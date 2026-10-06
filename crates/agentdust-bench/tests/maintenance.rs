mod common;

use std::fs::{self, File};
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Path, PathBuf};
use std::time::Duration;

use agentdust_bench::frame::{Framing, decode, encode};
use agentdust_bench::maintenance::{MaintenanceError, RetainReport, Retention, Rotation, generation_stamp};
use agentdust_bench::payload::{is_marker, marker};
use agentdust_bench::{Candidate, Journal, Options};
use agentdust_core::journal::Record;
use common::{Scratch, count_of, data_files, named, names_in, plant, plant_bytes};

const T: u64 = 1_800_000_000_000;

fn open(candidate: Candidate, dir: &Path) -> Box<dyn Journal> {
    candidate.open_with(
        dir,
        Options {
            maintenance_budget: Duration::ZERO,
            ..Options::default()
        },
    )
}

fn expired(n: u64) -> Record {
    marker(n, T, 100 + n)
}

fn retain(journal: &dyn Journal, now_ms: u64, grace_ms: u64) -> RetainReport {
    let keep = |record: &Record| !is_marker(record);
    journal
        .retain(&Retention {
            now_ms,
            grace_ms,
            keep: &keep,
        })
        .unwrap()
}

fn bytes_of(candidate: Candidate, records: &[Record]) -> Vec<u8> {
    records
        .iter()
        .flat_map(|record| encode(record, candidate.framing()).unwrap())
        .collect()
}

fn ino(path: &Path) -> u64 {
    fs::metadata(path).unwrap().ino()
}

fn newer_version_line(pad: usize) -> Vec<u8> {
    format!(
        "{{\"v\":3,\"kind\":\"future\",\"pad\":\"{}\"}}\n",
        "a".repeat(pad)
    )
    .into_bytes()
}

fn each_candidate(test: impl Fn(Candidate, &Path)) {
    for candidate in Candidate::ALL {
        let scratch = Scratch::new(candidate.label());
        test(candidate, scratch.path());
    }
}

fn generation(dir: &Path, stamp: u64) -> PathBuf {
    dir.join(format!("journal.{stamp}.jsonl"))
}

#[test]
fn generation_names_are_canonical_decimal_stamps_and_nothing_else() {
    assert_eq!(generation_stamp("journal.5.jsonl"), Some(5));
    assert_eq!(generation_stamp("journal.0.jsonl"), Some(0));
    assert_eq!(
        generation_stamp("journal.18446744073709551615.jsonl"),
        Some(u64::MAX)
    );
    for name in [
        "journal.jsonl",
        "journal..jsonl",
        "journal.007.jsonl",
        "journal.+7.jsonl",
        "journal.-7.jsonl",
        "journal.5.jsonl.tmp",
        "journal.jsonl.corrupt-5",
        "journal.18446744073709551616.jsonl",
        "xjournal.5.jsonl",
        "journal.5.json",
        "journal.lock",
        "journal.maint",
        "journal.compact.tmp",
    ] {
        assert_eq!(generation_stamp(name), None, "{name}");
    }
}

#[test]
fn rotation_renames_the_active_file_and_never_truncates_it() {
    each_candidate(|candidate, dir| {
        let journal = open(candidate, dir);
        for n in 1..=3 {
            journal.append(&named(&format!("s{n}"), n)).unwrap();
        }
        let before = fs::read(dir.join("journal.jsonl")).unwrap();
        let inode = ino(&dir.join("journal.jsonl"));
        assert!(
            matches!(journal.rotate(T), Ok(Rotation::Rotated { stamp: T })),
            "{}",
            candidate.label()
        );
        assert!(!dir.join("journal.jsonl").exists(), "{}", candidate.label());
        assert_eq!(
            fs::read(generation(dir, T)).unwrap(),
            before,
            "{}",
            candidate.label()
        );
        assert_eq!(ino(&generation(dir, T)), inode, "{}", candidate.label());
    });
}

#[test]
fn a_missing_or_empty_active_file_is_not_rotated_and_a_missing_directory_stays_missing() {
    each_candidate(|candidate, dir| {
        let journal = open(candidate, dir);
        assert!(
            matches!(journal.rotate(T), Ok(Rotation::Empty)),
            "{}",
            candidate.label()
        );
        assert!(!dir.exists(), "{}", candidate.label());
        plant_bytes(dir, "journal.jsonl", b"");
        assert!(
            matches!(journal.rotate(T), Ok(Rotation::Empty)),
            "{}",
            candidate.label()
        );
        assert!(dir.join("journal.jsonl").exists(), "{}", candidate.label());
        assert!(!generation(dir, T).exists(), "{}", candidate.label());
    });
}

#[test]
fn a_taken_stamp_moves_to_the_next_free_one() {
    each_candidate(|candidate, dir| {
        let journal = open(candidate, dir);
        journal.append(&named("one", 1)).unwrap();
        assert!(matches!(journal.rotate(T), Ok(Rotation::Rotated { stamp: T })));
        journal.append(&named("two", 2)).unwrap();
        assert!(
            matches!(journal.rotate(T), Ok(Rotation::Rotated { stamp }) if stamp == T + 1),
            "{}",
            candidate.label()
        );
        assert!(generation(dir, T).exists() && generation(dir, T + 1).exists());
    });
}

#[test]
fn the_next_append_after_a_rotation_creates_a_new_active_file() {
    each_candidate(|candidate, dir| {
        let journal = open(candidate, dir);
        journal.append(&named("one", 1)).unwrap();
        journal.rotate(T).unwrap();
        journal.append(&named("two", 2)).unwrap();
        let active = decode(File::open(dir.join("journal.jsonl")).unwrap()).unwrap();
        assert_eq!(count_of(&active.records, "two"), 1, "{}", candidate.label());
        assert_eq!(active.records.len(), 1, "{}", candidate.label());
    });
}

#[test]
fn a_symlinked_active_file_is_not_rotated() {
    each_candidate(|candidate, dir| {
        fs::create_dir_all(dir).unwrap();
        let target = dir.join("elsewhere");
        fs::write(&target, b"data\n").unwrap();
        symlink(&target, dir.join("journal.jsonl")).unwrap();
        let journal = open(candidate, dir);
        assert!(
            matches!(journal.rotate(T), Err(MaintenanceError::Io(_))),
            "{}",
            candidate.label()
        );
        assert!(!generation(dir, T).exists(), "{}", candidate.label());
    });
}

#[test]
fn a_held_maintenance_lock_makes_rotation_and_retention_busy_for_every_candidate() {
    each_candidate(|candidate, dir| {
        let journal = open(candidate, dir);
        journal.append(&named("one", 1)).unwrap();
        let name = if candidate == Candidate::Flock {
            "journal.lock"
        } else {
            "journal.maint"
        };
        let holder = File::options()
            .create(true)
            .append(true)
            .open(dir.join(name))
            .unwrap();
        holder.lock().unwrap();
        assert!(
            matches!(journal.rotate(T), Err(MaintenanceError::Busy)),
            "{}",
            candidate.label()
        );
        let keep = |_: &Record| true;
        let retained = journal.retain(&Retention {
            now_ms: T,
            grace_ms: 0,
            keep: &keep,
        });
        assert!(
            matches!(retained, Err(MaintenanceError::Busy)),
            "{}",
            candidate.label()
        );
        drop(holder);
        assert!(
            matches!(journal.rotate(T), Ok(Rotation::Rotated { .. })),
            "{}",
            candidate.label()
        );
    });
}

#[test]
fn a_generation_with_nothing_to_drop_is_left_alone() {
    each_candidate(|candidate, dir| {
        plant(
            dir,
            "journal.10.jsonl",
            candidate,
            &[named("a", 1), named("b", 2)],
        );
        let before = fs::read(generation(dir, 10)).unwrap();
        let inode = ino(&generation(dir, 10));
        let report = retain(&*open(candidate, dir), T, 0);
        assert_eq!(
            (
                report.eligible,
                report.untouched,
                report.rewritten,
                report.deleted
            ),
            (1, 1, 0, 0)
        );
        assert_eq!(fs::read(generation(dir, 10)).unwrap(), before);
        assert_eq!(ino(&generation(dir, 10)), inode);
    });
}

#[test]
fn a_generation_with_a_droppable_record_is_replaced_by_the_kept_records_verbatim_and_in_order() {
    each_candidate(|candidate, dir| {
        let kept = [named("a", 1), named("b", 3)];
        plant(
            dir,
            "journal.10.jsonl",
            candidate,
            &[kept[0].clone(), expired(1), kept[1].clone()],
        );
        let inode = ino(&generation(dir, 10));
        let report = retain(&*open(candidate, dir), T, 0);
        assert_eq!(
            (report.rewritten, report.deleted, report.dropped_records),
            (1, 0, 1),
            "{}",
            candidate.label()
        );
        assert_eq!(fs::read(generation(dir, 10)).unwrap(), bytes_of(candidate, &kept));
        assert_ne!(ino(&generation(dir, 10)), inode);
        let mode = fs::metadata(generation(dir, 10)).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(!dir.join("journal.compact.tmp").exists());
    });
}

#[test]
fn a_generation_with_no_kept_record_is_deleted() {
    each_candidate(|candidate, dir| {
        plant(dir, "journal.10.jsonl", candidate, &[expired(1), expired(2)]);
        let report = retain(&*open(candidate, dir), T, 0);
        assert_eq!(
            (report.rewritten, report.deleted, report.dropped_records),
            (0, 1, 2)
        );
        assert!(!generation(dir, 10).exists());
    });
}

#[test]
fn the_active_file_is_never_rewritten_or_deleted() {
    each_candidate(|candidate, dir| {
        plant(dir, "journal.jsonl", candidate, &[named("a", 1), expired(1)]);
        plant(dir, "journal.10.jsonl", candidate, &[expired(2)]);
        let before = fs::read(dir.join("journal.jsonl")).unwrap();
        let inode = ino(&dir.join("journal.jsonl"));
        let report = retain(&*open(candidate, dir), T, 0);
        assert_eq!(report.deleted, 1, "{}", candidate.label());
        assert_eq!(fs::read(dir.join("journal.jsonl")).unwrap(), before);
        assert_eq!(ino(&dir.join("journal.jsonl")), inode);
    });
}

#[test]
fn a_generation_becomes_eligible_at_exactly_its_grace_and_waits_before_that() {
    each_candidate(|candidate, dir| {
        plant(dir, "journal.100.jsonl", candidate, &[expired(1)]);
        let journal = open(candidate, dir);
        let early = retain(&*journal, 100 + 999, 1_000);
        assert_eq!(
            (early.eligible, early.waiting, early.deleted),
            (0, 1, 0),
            "{}",
            candidate.label()
        );
        assert!(generation(dir, 100).exists());
        let on_time = retain(&*journal, 100 + 1_000, 1_000);
        assert_eq!((on_time.eligible, on_time.waiting, on_time.deleted), (1, 0, 1));
    });
}

#[test]
fn a_generation_stamped_after_now_waits_for_the_clock_to_catch_up() {
    each_candidate(|candidate, dir| {
        plant(dir, "journal.5000.jsonl", candidate, &[expired(1)]);
        let report = retain(&*open(candidate, dir), 4_000, 0);
        assert_eq!((report.eligible, report.waiting, report.deleted), (0, 1, 0));
        assert!(generation(dir, 5_000).exists());
    });
}

#[test]
fn a_generation_with_a_newer_version_line_is_never_shrunk_or_deleted() {
    each_candidate(|candidate, dir| {
        let mut bytes = bytes_of(candidate, &[named("a", 1), expired(1)]);
        bytes.extend(newer_version_line(10));
        plant_bytes(dir, "journal.10.jsonl", &bytes);
        let mut only_droppable = bytes_of(candidate, &[expired(2)]);
        only_droppable.extend(newer_version_line(10));
        plant_bytes(dir, "journal.20.jsonl", &only_droppable);
        plant(dir, "journal.30.jsonl", candidate, &[expired(3)]);
        let inode = ino(&generation(dir, 10));
        let report = retain(&*open(candidate, dir), T, 0);
        assert_eq!(report.held_newer_version, 2, "{}", candidate.label());
        assert_eq!(
            (report.deleted, report.rewritten),
            (1, 0),
            "{}",
            candidate.label()
        );
        assert_eq!(fs::read(generation(dir, 10)).unwrap(), bytes);
        assert_eq!(ino(&generation(dir, 10)), inode);
        assert_eq!(fs::read(generation(dir, 20)).unwrap(), only_droppable);
        assert!(!generation(dir, 30).exists());
    });
}

#[test]
fn a_newer_version_line_longer_than_the_cap_still_protects_its_generation() {
    each_candidate(|candidate, dir| {
        let mut bytes = bytes_of(candidate, &[expired(1)]);
        bytes.extend(newer_version_line(200_000));
        plant_bytes(dir, "journal.10.jsonl", &bytes);
        let report = retain(&*open(candidate, dir), T, 0);
        assert_eq!(report.held_newer_version, 1, "{}", candidate.label());
        assert_eq!(report.deleted + report.rewritten, 0, "{}", candidate.label());
        assert_eq!(fs::read(generation(dir, 10)).unwrap(), bytes);
    });
}

#[test]
fn lines_of_an_unknown_kind_survive_a_rewrite_byte_for_byte() {
    each_candidate(|candidate, dir| {
        let unknown = "{\"v\":1,\"kind\":\"future_kind\",\"agent\":\"claude\",\"session_id\":\"x\",\"wall_ts\":1,\"mono_ts\":2,\"boot\":\"b\"}\n";
        let mut bytes = bytes_of(candidate, &[named("a", 1), expired(1)]);
        bytes.extend(unknown.as_bytes());
        plant_bytes(dir, "journal.10.jsonl", &bytes);
        plant_bytes(
            dir,
            "journal.20.jsonl",
            &[bytes_of(candidate, &[expired(2)]), unknown.as_bytes().to_vec()].concat(),
        );
        let report = retain(&*open(candidate, dir), T, 0);
        assert_eq!(
            (report.rewritten, report.deleted),
            (2, 0),
            "{}",
            candidate.label()
        );
        for stamp in [10, 20] {
            let text = fs::read(generation(dir, stamp)).unwrap();
            assert!(
                text.windows(unknown.len())
                    .any(|window| window == unknown.as_bytes()),
                "{} {stamp}",
                candidate.label()
            );
            let decoded = decode(&text[..]).unwrap();
            assert_eq!(decoded.unknown_kind, 1);
            assert_eq!(decoded.records.iter().filter(|r| is_marker(r)).count(), 0);
        }
    });
}

#[test]
fn damaged_lines_go_when_a_generation_is_rewritten_and_never_cause_a_rewrite_alone() {
    each_candidate(|candidate, dir| {
        let mut alone = bytes_of(candidate, &[named("a", 1)]);
        alone.extend(b"\x1e{\"v\":1,\"kind\":\"shell_st");
        plant_bytes(dir, "journal.10.jsonl", &alone);
        let mut mixed = bytes_of(candidate, &[named("b", 2), expired(1)]);
        mixed.extend(b"not json at all\n");
        plant_bytes(dir, "journal.20.jsonl", &mixed);
        let report = retain(&*open(candidate, dir), T, 0);
        assert_eq!(
            (report.untouched, report.rewritten),
            (1, 1),
            "{}",
            candidate.label()
        );
        assert_eq!(report.dropped_damaged_lines, 1, "{}", candidate.label());
        assert_eq!(fs::read(generation(dir, 10)).unwrap(), alone);
        assert_eq!(
            fs::read(generation(dir, 20)).unwrap(),
            bytes_of(candidate, &[named("b", 2)])
        );
    });
}

#[test]
fn an_exact_duplicate_in_a_later_generation_is_dropped_and_counted() {
    each_candidate(|candidate, dir| {
        plant(dir, "journal.10.jsonl", candidate, &[named("a", 1)]);
        plant(
            dir,
            "journal.20.jsonl",
            candidate,
            &[named("a", 1), named("b", 2)],
        );
        plant(dir, "journal.30.jsonl", candidate, &[named("a", 1)]);
        let report = retain(&*open(candidate, dir), T, 0);
        assert_eq!(report.duplicates_removed, 2, "{}", candidate.label());
        assert_eq!(
            (report.rewritten, report.deleted),
            (1, 1),
            "{}",
            candidate.label()
        );
        assert_eq!(
            fs::read(generation(dir, 10)).unwrap(),
            bytes_of(candidate, &[named("a", 1)])
        );
        assert_eq!(
            fs::read(generation(dir, 20)).unwrap(),
            bytes_of(candidate, &[named("b", 2)])
        );
        assert!(!generation(dir, 30).exists());
    });
}

#[test]
fn a_duplicate_in_a_generation_that_is_still_waiting_is_left_for_the_reader() {
    each_candidate(|candidate, dir| {
        plant(dir, "journal.10.jsonl", candidate, &[named("a", 1)]);
        plant(dir, "journal.20.jsonl", candidate, &[named("a", 1)]);
        let report = retain(&*open(candidate, dir), 25, 10);
        assert_eq!(
            (report.eligible, report.waiting, report.duplicates_removed),
            (1, 1, 0)
        );
        assert!(generation(dir, 20).exists());
    });
}

#[test]
fn a_leftover_temporary_file_is_replaced_and_a_symlink_in_its_place_is_not_followed() {
    each_candidate(|candidate, dir| {
        plant(dir, "journal.10.jsonl", candidate, &[named("a", 1), expired(1)]);
        plant_bytes(dir, "journal.compact.tmp", b"half a compaction");
        retain(&*open(candidate, dir), T, 0);
        assert_eq!(
            fs::read(generation(dir, 10)).unwrap(),
            bytes_of(candidate, &[named("a", 1)])
        );
        assert!(!dir.join("journal.compact.tmp").exists());

        let outside = dir.join("outside");
        fs::write(&outside, b"keep me").unwrap();
        plant(dir, "journal.20.jsonl", candidate, &[named("b", 2), expired(2)]);
        symlink(&outside, dir.join("journal.compact.tmp")).unwrap();
        retain(&*open(candidate, dir), T, 0);
        assert_eq!(fs::read(&outside).unwrap(), b"keep me");
        assert_eq!(
            fs::read(generation(dir, 20)).unwrap(),
            bytes_of(candidate, &[named("b", 2)])
        );
    });
}

#[test]
fn files_that_are_not_canonical_generations_are_never_touched() {
    each_candidate(|candidate, dir| {
        for name in [
            "journal.007.jsonl",
            "journal.5.jsonl.tmp",
            "journal.jsonl.corrupt-5",
        ] {
            plant(dir, name, candidate, &[expired(1)]);
        }
        let data = |dir: &Path| -> Vec<String> {
            data_files(dir)
                .iter()
                .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        let before = data(dir);
        let report = retain(&*open(candidate, dir), T, 0);
        assert_eq!(report.eligible + report.waiting, 0, "{}", candidate.label());
        assert_eq!(data(dir), before, "{}", candidate.label());
        for name in &before {
            assert_eq!(
                fs::read(dir.join(name)).unwrap(),
                bytes_of(candidate, &[expired(1)]),
                "{} {name}",
                candidate.label()
            );
        }
    });
}

#[test]
fn retention_of_a_missing_directory_does_nothing_and_creates_nothing() {
    each_candidate(|candidate, dir| {
        let report = retain(&*open(candidate, dir), T, 0);
        assert_eq!(report, RetainReport::default(), "{}", candidate.label());
        assert!(!dir.exists(), "{}", candidate.label());
    });
}

#[test]
fn a_second_retention_run_changes_nothing() {
    each_candidate(|candidate, dir| {
        plant(dir, "journal.10.jsonl", candidate, &[named("a", 1), expired(1)]);
        plant(
            dir,
            "journal.20.jsonl",
            candidate,
            &[named("a", 1), named("b", 2)],
        );
        let journal = open(candidate, dir);
        retain(&*journal, T, 0);
        let names = names_in(dir);
        let bytes: Vec<_> = names
            .iter()
            .map(|name| fs::read(dir.join(name)).unwrap())
            .collect();
        let again = retain(&*journal, T, 0);
        assert_eq!(
            (again.rewritten, again.deleted, again.duplicates_removed),
            (0, 0, 0)
        );
        assert_eq!(names_in(dir), names);
        let after: Vec<_> = names
            .iter()
            .map(|name| fs::read(dir.join(name)).unwrap())
            .collect();
        assert_eq!(after, bytes);
    });
}

#[test]
fn candidate_d_swaps_a_generation_only_when_no_appender_holds_the_shared_lock() {
    let scratch = Scratch::new("d-swap");
    let dir = scratch.path();
    let journal = open(Candidate::Shared, dir);
    journal.append(&named("a", 1)).unwrap();
    plant(dir, "journal.10.jsonl", Candidate::Shared, &[expired(1)]);
    let appender = File::options()
        .write(true)
        .open(dir.join("journal.lock"))
        .unwrap();
    appender.lock_shared().unwrap();
    let keep = |record: &Record| !is_marker(record);
    let busy = journal.retain(&Retention {
        now_ms: T,
        grace_ms: 0,
        keep: &keep,
    });
    assert!(matches!(busy, Err(MaintenanceError::Busy)));
    assert!(generation(dir, 10).exists());
    drop(appender);
    assert_eq!(retain(&*journal, T, 0).deleted, 1);
    assert!(!generation(dir, 10).exists());
}

#[test]
fn candidates_c_and_c2_retain_while_a_journal_lock_is_held_because_they_never_take_it() {
    for candidate in [Candidate::Append, Candidate::Recheck] {
        let scratch = Scratch::new("free-swap");
        let dir = scratch.path();
        plant(dir, "journal.10.jsonl", candidate, &[expired(1)]);
        let holder = File::options()
            .create(true)
            .append(true)
            .open(dir.join("journal.lock"))
            .unwrap();
        holder.lock().unwrap();
        assert_eq!(
            retain(&*open(candidate, dir), T, 0).deleted,
            1,
            "{}",
            candidate.label()
        );
    }
}

#[test]
fn the_kept_records_keep_the_framing_their_candidate_writes() {
    for candidate in Candidate::ALL {
        let scratch = Scratch::new("framing");
        let dir = scratch.path();
        plant(dir, "journal.10.jsonl", candidate, &[named("a", 1), expired(1)]);
        retain(&*open(candidate, dir), T, 0);
        let bytes = fs::read(generation(dir, 10)).unwrap();
        let expected = match candidate.framing() {
            Framing::Record => b'\x1e',
            Framing::Line => b'{',
        };
        assert_eq!(bytes[0], expected, "{}", candidate.label());
    }
}

#[test]
fn turning_the_syncs_off_changes_no_file_the_maintenance_leaves_behind() {
    for candidate in Candidate::ALL {
        let state = |sync: bool| {
            let scratch = Scratch::new("sync");
            let dir = scratch.path();
            plant(
                dir,
                "journal.10.jsonl",
                candidate,
                &[named("a", 1), expired(1), named("a2", 4), named("a3", 5)],
            );
            plant(
                dir,
                "journal.20.jsonl",
                candidate,
                &[named("a", 1), named("b", 2)],
            );
            plant(dir, "journal.30.jsonl", candidate, &[expired(2)]);
            plant(dir, "journal.jsonl", candidate, &[named("c", 3)]);
            let journal = candidate.open_with(
                dir,
                Options {
                    maintenance_budget: Duration::ZERO,
                    sync,
                },
            );
            assert!(matches!(journal.rotate(T), Ok(Rotation::Rotated { .. })));
            let report = retain(&*journal, T, 0);
            let mut files: Vec<_> = names_in(dir)
                .into_iter()
                .map(|name| (fs::read(dir.join(&name)).unwrap(), name))
                .collect();
            files.sort();
            (files, report)
        };
        assert_eq!(state(true), state(false), "{}", candidate.label());
    }
}

#[test]
fn syncing_is_on_unless_a_caller_turns_it_off() {
    assert!(Options::default().sync);
}
