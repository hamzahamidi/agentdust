mod common;

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use agentdust_bench::frame::{EncodeError, Framing, MAX_FRAME_LEN, encode};
use agentdust_bench::maintenance::Retention;
use agentdust_bench::payload::{line_len, record_with};
use agentdust_bench::{AppendError, Candidate, NoProbe, Options, Point};
use agentdust_core::journal::Record;
use common::{Gate, HANG_GUARD, Scratch, data_files, files_under, named, names_in, plant, stamped};

fn each_candidate(test: impl Fn(Candidate, &Path)) {
    for candidate in Candidate::ALL {
        let scratch = Scratch::new(candidate.label());
        test(candidate, scratch.path());
    }
}

fn sequence(count: u64) -> Vec<Record> {
    (0..count)
        .map(|seq| record_with(0, seq, 150, 1_800_000_000_000 + seq, 1_000 + seq))
        .collect()
}

fn lock_file(dir: &Path) -> File {
    OpenOptions::new()
        .write(true)
        .open(dir.join("journal.lock"))
        .unwrap()
}

fn immediate() -> Options {
    Options {
        maintenance_budget: Duration::ZERO,
        ..Options::default()
    }
}

fn keep_all(now_ms: u64) -> Retention<'static> {
    Retention {
        now_ms,
        grace_ms: 0,
        keep: &|_| true,
    }
}

fn junk_prefix(candidate: Candidate) -> &'static [u8] {
    match candidate.framing() {
        Framing::Record => b"\x1e",
        Framing::Line => b"",
    }
}

#[test]
fn the_four_candidates_are_labelled_a_c_c2_and_d() {
    let labels: Vec<_> = Candidate::ALL.iter().map(|c| c.label()).collect();
    assert_eq!(labels, ["A", "C", "C2", "D"]);
    for candidate in Candidate::ALL {
        assert_eq!(Candidate::from_label(candidate.label()), Some(candidate));
        assert_eq!(
            Candidate::from_label(&candidate.label().to_lowercase()),
            Some(candidate)
        );
    }
    assert_eq!(Candidate::from_label("B"), None);
    assert_eq!(Candidate::from_label("E"), None);
}

#[test]
fn only_candidate_a_keeps_the_m0_line_framing() {
    for candidate in Candidate::ALL {
        let expected = if candidate == Candidate::Flock {
            Framing::Line
        } else {
            Framing::Record
        };
        assert_eq!(candidate.framing(), expected, "{}", candidate.label());
    }
}

#[test]
fn appended_records_read_back_whole_and_in_order() {
    each_candidate(|candidate, dir| {
        let journal = candidate.open(dir);
        let records = sequence(5);
        for record in &records {
            assert_eq!(journal.append(record).unwrap().attempts, 1);
        }
        let report = journal.read_all().unwrap();
        assert_eq!(report.records, records, "{}", candidate.label());
        assert_eq!(report.skipped_lines(), 0, "{}", candidate.label());
        assert_eq!(report.duplicates_removed, 0, "{}", candidate.label());
    });
}

#[test]
fn a_new_handle_reads_what_another_handle_appended() {
    each_candidate(|candidate, dir| {
        let records = sequence(3);
        for record in &records {
            candidate.open(dir).append(record).unwrap();
        }
        assert_eq!(candidate.open(dir).read_all().unwrap().records, records);
    });
}

#[test]
fn a_4000_byte_record_round_trips() {
    each_candidate(|candidate, dir| {
        let record = record_with(1, 7, 4000, 1_800_000_000_000, 123);
        assert_eq!(line_len(&record), 4000);
        let journal = candidate.open(dir);
        journal.append(&record).unwrap();
        assert_eq!(
            journal.read_all().unwrap().records,
            vec![record],
            "{}",
            candidate.label()
        );
        let written = fs::metadata(dir.join("journal.jsonl")).unwrap().len();
        let framing_bytes = if candidate.framing() == Framing::Record {
            1
        } else {
            0
        };
        assert_eq!(written, 4000 + framing_bytes, "{}", candidate.label());
    });
}

#[test]
fn reading_a_directory_that_does_not_exist_is_empty_and_creates_nothing() {
    each_candidate(|candidate, dir| {
        let journal = candidate.open(dir);
        let report = journal.read_all().unwrap();
        assert!(report.records.is_empty(), "{}", candidate.label());
        assert_eq!(report.skipped_lines(), 0, "{}", candidate.label());
        assert!(!dir.exists(), "{}", candidate.label());
        assert_eq!(journal.file_count().unwrap(), 0, "{}", candidate.label());
    });
}

#[test]
fn the_directory_is_private_and_every_file_in_it_is_0600_after_maintenance_too() {
    each_candidate(|candidate, dir| {
        let journal = candidate.open(dir);
        journal.append(&sequence(1)[0]).unwrap();
        journal.rotate(1_800_000_000_000).unwrap();
        journal.append(&sequence(2)[1]).unwrap();
        journal.retain(&keep_all(1_800_000_000_000)).unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(dir), 0o700, "{}", candidate.label());
        for path in files_under(dir) {
            assert_eq!(mode(&path), 0o600, "{} {path:?}", candidate.label());
        }
    });
}

#[test]
fn the_file_layout_is_one_file_for_c_and_c2_and_two_for_a_and_d() {
    let expected = [
        (Candidate::Flock, vec!["journal.jsonl", "journal.lock"]),
        (Candidate::Append, vec!["journal.jsonl"]),
        (Candidate::Recheck, vec!["journal.jsonl"]),
        (Candidate::Shared, vec!["journal.jsonl", "journal.lock"]),
    ];
    for (candidate, names) in expected {
        let scratch = Scratch::new("layout");
        let journal = candidate.open(scratch.path());
        for record in sequence(3) {
            journal.append(&record).unwrap();
        }
        assert_eq!(names_in(scratch.path()), names, "{}", candidate.label());
        assert_eq!(
            journal.file_count().unwrap(),
            names.len(),
            "{}",
            candidate.label()
        );
    }
}

#[test]
fn a_truncated_last_line_is_skipped_and_counted() {
    each_candidate(|candidate, dir| {
        let journal = candidate.open(dir);
        let records = sequence(2);
        for record in &records {
            journal.append(record).unwrap();
        }
        let mut junk = junk_prefix(candidate).to_vec();
        junk.extend(b"{\"v\":1,\"kind\":\"shell_st");
        OpenOptions::new()
            .append(true)
            .open(dir.join("journal.jsonl"))
            .unwrap()
            .write_all(&junk)
            .unwrap();
        let report = journal.read_all().unwrap();
        assert_eq!(report.records, records, "{}", candidate.label());
        assert_eq!(report.skipped_lines(), 1, "{}", candidate.label());
    });
}

#[test]
fn a_line_from_a_future_schema_version_is_skipped_and_counted() {
    each_candidate(|candidate, dir| {
        let journal = candidate.open(dir);
        let records = sequence(2);
        for record in &records {
            journal.append(record).unwrap();
        }
        let mut future = serde_json::to_value(&records[0]).unwrap();
        future["v"] = serde_json::json!(3);
        let mut line = junk_prefix(candidate).to_vec();
        line.extend(format!("{future}\n").bytes());
        OpenOptions::new()
            .append(true)
            .open(dir.join("journal.jsonl"))
            .unwrap()
            .write_all(&line)
            .unwrap();
        let report = journal.read_all().unwrap();
        assert_eq!(report.records, records, "{}", candidate.label());
        assert_eq!(report.newer_version_lines, 1, "{}", candidate.label());
        assert_eq!(report.skipped_lines(), 1, "{}", candidate.label());
    });
}

#[test]
fn after_a_torn_write_the_next_record_survives_with_rs_framing_and_is_lost_without_it() {
    each_candidate(|candidate, dir| {
        let journal = candidate.open(dir);
        let (first, second, third) = (named("one", 1), named("two", 2), named("three", 3));
        journal.append(&first).unwrap();
        let torn = encode(&second, candidate.framing()).unwrap();
        OpenOptions::new()
            .append(true)
            .open(dir.join("journal.jsonl"))
            .unwrap()
            .write_all(&torn[..torn.len() - 7])
            .unwrap();
        journal.append(&third).unwrap();
        let report = journal.read_all().unwrap();
        match candidate.framing() {
            Framing::Record => {
                assert_eq!(report.records, vec![first, third], "{}", candidate.label());
                assert_eq!(report.skipped_lines(), 1, "{}", candidate.label());
            }
            Framing::Line => {
                assert_eq!(report.records, vec![first], "{}", candidate.label());
                assert_eq!(report.malformed_lines, 1, "{}", candidate.label());
            }
        }
    });
}

#[test]
fn a_held_exclusive_lock_drops_the_a_and_d_appends() {
    for candidate in [Candidate::Flock, Candidate::Shared] {
        let scratch = Scratch::new("busy");
        let journal = candidate.open(scratch.path());
        journal.append(&sequence(1)[0]).unwrap();
        let holder = lock_file(scratch.path());
        holder.lock().unwrap();
        let result = journal.append(&sequence(2)[1]);
        assert!(matches!(result, Err(AppendError::Busy)), "{}", candidate.label());
        drop(holder);
        assert_eq!(
            journal.read_all().unwrap().records.len(),
            1,
            "{}",
            candidate.label()
        );
        journal.append(&sequence(2)[1]).unwrap();
    }
}

#[test]
fn a_held_shared_lock_drops_the_a_append_and_not_the_d_append() {
    let scratch = Scratch::new("shared-a");
    let journal = Candidate::Flock.open(scratch.path());
    journal.append(&sequence(1)[0]).unwrap();
    let reader = lock_file(scratch.path());
    reader.lock_shared().unwrap();
    assert!(matches!(journal.append(&sequence(2)[1]), Err(AppendError::Busy)));
    drop(reader);
    journal.append(&sequence(2)[1]).unwrap();

    let scratch = Scratch::new("shared-d");
    let journal = Candidate::Shared.open(scratch.path());
    journal.append(&sequence(1)[0]).unwrap();
    let reader = lock_file(scratch.path());
    reader.lock_shared().unwrap();
    journal.append(&sequence(2)[1]).unwrap();
    assert_eq!(journal.read_all().unwrap().records.len(), 2);
}

#[test]
fn shared_locks_do_not_serialise_d_appenders_and_an_exclusive_lock_waits_for_them() {
    let scratch = Scratch::new("d-shared");
    let journal = Candidate::Shared.open(scratch.path());
    journal.append(&sequence(1)[0]).unwrap();
    let gate = Gate::at(Point::AfterOpen);
    thread::scope(|scope| {
        let paused = scope.spawn(|| journal.append_probed(&sequence(2)[1], &gate));
        gate.wait_reached();
        journal.append(&sequence(3)[2]).unwrap();
        let rotator = lock_file(scratch.path());
        assert!(matches!(rotator.try_lock(), Err(fs::TryLockError::WouldBlock)));
        gate.release();
        paused.join().unwrap().unwrap();
        assert!(rotator.try_lock().is_ok());
    });
    assert_eq!(journal.read_all().unwrap().records.len(), 3);
}

#[test]
fn an_a_appender_holds_the_lock_exclusively_from_before_the_open_until_after_the_write() {
    let scratch = Scratch::new("a-excl");
    let journal = Candidate::Flock.open(scratch.path());
    journal.append(&sequence(1)[0]).unwrap();
    let gate = Gate::at(Point::AfterWrite);
    thread::scope(|scope| {
        let paused = scope.spawn(|| journal.append_probed(&sequence(2)[1], &gate));
        gate.wait_reached();
        assert!(matches!(journal.append(&sequence(3)[2]), Err(AppendError::Busy)));
        gate.release();
        paused.join().unwrap().unwrap();
    });
}

#[test]
fn a_reader_of_a_and_d_holds_the_shared_lock_until_it_has_opened_every_file() {
    for candidate in [Candidate::Flock, Candidate::Shared] {
        let scratch = Scratch::new("reader-lock");
        let journal = candidate.open(scratch.path());
        journal.append(&sequence(1)[0]).unwrap();
        let gate = Gate::at(Point::ReaderAfterList);
        thread::scope(|scope| {
            let reader = scope.spawn(|| journal.read_probed(&gate));
            gate.wait_reached();
            let rotator = lock_file(scratch.path());
            assert!(
                matches!(rotator.try_lock(), Err(fs::TryLockError::WouldBlock)),
                "{}",
                candidate.label()
            );
            gate.release();
            assert_eq!(reader.join().unwrap().unwrap().records.len(), 1);
            assert!(rotator.try_lock().is_ok(), "{}", candidate.label());
        });
    }
}

#[test]
fn a_reader_of_c_and_c2_takes_no_lock_and_creates_none() {
    for candidate in [Candidate::Append, Candidate::Recheck] {
        let scratch = Scratch::new("reader-free");
        let journal = candidate.open(scratch.path());
        journal.append(&sequence(1)[0]).unwrap();
        let gate = Gate::at(Point::ReaderAfterList);
        thread::scope(|scope| {
            let reader = scope.spawn(|| journal.read_probed(&gate));
            gate.wait_reached();
            let rotated = journal.rotate(1_800_000_000_000).unwrap();
            assert!(matches!(
                rotated,
                agentdust_bench::maintenance::Rotation::Rotated { .. }
            ));
            gate.release();
            assert_eq!(reader.join().unwrap().unwrap().records.len(), 1);
        });
        assert!(!scratch.path().join("journal.lock").exists());
    }
}

#[test]
fn an_a_reader_waits_for_a_writer_that_holds_the_lock() {
    let scratch = Scratch::new("a-reader-waits");
    let journal = Candidate::Flock.open(scratch.path());
    journal.append(&sequence(1)[0]).unwrap();
    let writer = lock_file(scratch.path());
    writer.lock().unwrap();
    let (started, ready) = mpsc::channel();
    let (finished, result) = mpsc::channel();
    let dir = scratch.path().to_path_buf();
    thread::spawn(move || {
        started.send(()).unwrap();
        finished
            .send(Candidate::Flock.open(&dir).read_all().unwrap().records.len())
            .unwrap();
    });
    ready.recv_timeout(HANG_GUARD).unwrap();
    thread::sleep(Duration::from_millis(100));
    assert!(result.try_recv().is_err());
    drop(writer);
    assert_eq!(result.recv_timeout(HANG_GUARD).unwrap(), 1);
}

#[test]
fn two_a_readers_do_not_block_each_other() {
    let scratch = Scratch::new("a-two-readers");
    let journal = Candidate::Flock.open(scratch.path());
    journal.append(&sequence(1)[0]).unwrap();
    let first = lock_file(scratch.path());
    first.lock_shared().unwrap();
    let (sender, receiver) = mpsc::channel();
    let dir = scratch.path().to_path_buf();
    thread::spawn(move || {
        sender
            .send(Candidate::Flock.open(&dir).read_all().unwrap().records.len())
            .unwrap();
    });
    assert_eq!(receiver.recv_timeout(HANG_GUARD).unwrap(), 1);
}

#[test]
fn c_and_c2_ignore_a_held_journal_lock_and_create_none() {
    for candidate in [Candidate::Append, Candidate::Recheck] {
        let scratch = Scratch::new("no-lock");
        scratch.create();
        let held = File::create(scratch.path().join("journal.lock")).unwrap();
        held.lock().unwrap();
        let journal = candidate.open(scratch.path());
        journal.append(&sequence(1)[0]).unwrap();
        assert_eq!(journal.read_all().unwrap().records.len(), 1);
        drop(held);
        let fresh = Scratch::new("no-lock-fresh");
        candidate.open(fresh.path()).append(&sequence(1)[0]).unwrap();
        assert_eq!(names_in(fresh.path()), ["journal.jsonl"], "{}", candidate.label());
    }
}

#[test]
fn a_symlinked_journal_is_refused_by_every_candidate() {
    each_candidate(|candidate, dir| {
        fs::create_dir_all(dir).unwrap();
        let target = dir.join("elsewhere");
        fs::write(&target, b"").unwrap();
        symlink(&target, dir.join("journal.jsonl")).unwrap();
        let result = candidate.open(dir).append(&sequence(1)[0]);
        assert!(matches!(result, Err(AppendError::Io(_))), "{}", candidate.label());
        assert_eq!(fs::read(&target).unwrap(), b"", "{}", candidate.label());
    });
}

#[test]
fn a_symlinked_generation_is_not_followed_and_is_counted() {
    each_candidate(|candidate, dir| {
        let journal = candidate.open(dir);
        journal.append(&sequence(1)[0]).unwrap();
        let outside = dir.join("outside.jsonl");
        plant(dir, "outside.jsonl", candidate, &[named("foreign", 1)]);
        symlink(&outside, dir.join("journal.5.jsonl")).unwrap();
        let report = journal.read_all().unwrap();
        assert_eq!(report.records, sequence(1), "{}", candidate.label());
        assert_eq!(report.skipped_lines(), 1, "{}", candidate.label());
    });
}

#[test]
fn a_file_whose_name_is_not_a_canonical_generation_is_not_read() {
    each_candidate(|candidate, dir| {
        let journal = candidate.open(dir);
        journal.append(&sequence(1)[0]).unwrap();
        for name in [
            "journal.007.jsonl",
            "journal.5.jsonl.tmp",
            "journal.jsonl.corrupt-5",
            "journal..jsonl",
        ] {
            plant(dir, name, candidate, &[named(name, 1)]);
        }
        assert_eq!(
            journal.read_all().unwrap().records,
            sequence(1),
            "{}",
            candidate.label()
        );
    });
}

#[test]
fn a_record_over_the_frame_cap_is_refused_by_every_candidate_and_creates_nothing() {
    each_candidate(|candidate, dir| {
        let journal = candidate.open(dir);
        let overhead = if candidate.framing() == Framing::Record {
            1
        } else {
            0
        };
        let too_big = record_with(0, 0, MAX_FRAME_LEN - overhead + 1, 1_800_000_000_000, 1);
        assert!(
            matches!(
                journal.append(&too_big),
                Err(AppendError::Encode(EncodeError::TooLarge { .. }))
            ),
            "{}",
            candidate.label()
        );
        assert!(!dir.exists(), "{}", candidate.label());
        let fits = record_with(0, 1, MAX_FRAME_LEN - overhead, 1_800_000_000_000, 2);
        journal.append(&fits).unwrap();
        assert_eq!(journal.read_all().unwrap().records, vec![fits]);
    });
}

#[test]
fn reading_orders_by_boot_then_monotonic_stamp_whatever_the_file_order() {
    each_candidate(|candidate, dir| {
        let journal = candidate.open(dir);
        journal.append(&stamped("new-1", "boot-new", 2_000, 5)).unwrap();
        journal.append(&stamped("old-1", "boot-old", 1_000, 900)).unwrap();
        journal.append(&stamped("old-2", "boot-old", 1_001, 901)).unwrap();
        journal.append(&stamped("new-2", "boot-new", 2_001, 6)).unwrap();
        let ids: Vec<_> = journal
            .read_all()
            .unwrap()
            .records
            .into_iter()
            .map(|r| r.session_id)
            .collect();
        assert_eq!(ids, ["old-1", "old-2", "new-1", "new-2"], "{}", candidate.label());
    });
}

#[test]
fn records_with_equal_stamps_are_ordered_by_content_and_not_by_file_position() {
    for candidate in Candidate::ALL {
        let forward = Scratch::new("ties-forward");
        let backward = Scratch::new("ties-backward");
        let ids = ["a", "b", "c"];
        for id in ids {
            candidate
                .open(forward.path())
                .append(&stamped(id, "boot", 1_000, 5))
                .unwrap();
        }
        for id in ids.iter().rev() {
            candidate
                .open(backward.path())
                .append(&stamped(id, "boot", 1_000, 5))
                .unwrap();
        }
        let order = |dir: &Path| -> Vec<String> {
            candidate
                .open(dir)
                .read_all()
                .unwrap()
                .records
                .into_iter()
                .map(|r| r.session_id)
                .collect()
        };
        assert_eq!(order(forward.path()), ids, "{}", candidate.label());
        assert_eq!(order(backward.path()), ids, "{}", candidate.label());
    }
}

#[test]
fn the_order_does_not_depend_on_which_generation_holds_a_record() {
    for candidate in Candidate::ALL {
        let one = Scratch::new("gen-one");
        let two = Scratch::new("gen-two");
        let (a, b) = (stamped("a", "boot", 1_000, 5), stamped("b", "boot", 1_000, 5));
        plant(
            one.path(),
            "journal.10.jsonl",
            candidate,
            std::slice::from_ref(&a),
        );
        plant(one.path(), "journal.jsonl", candidate, std::slice::from_ref(&b));
        plant(
            two.path(),
            "journal.10.jsonl",
            candidate,
            std::slice::from_ref(&b),
        );
        plant(two.path(), "journal.jsonl", candidate, std::slice::from_ref(&a));
        let read = |dir: &Path| candidate.open(dir).read_all().unwrap().records;
        assert_eq!(
            read(one.path()),
            vec![a.clone(), b.clone()],
            "{}",
            candidate.label()
        );
        assert_eq!(read(two.path()), vec![a, b], "{}", candidate.label());
    }
}

#[test]
fn exact_duplicates_collapse_and_are_counted_and_near_duplicates_do_not() {
    each_candidate(|candidate, dir| {
        let journal = candidate.open(dir);
        let record = stamped("dup", "boot", 1_000, 5);
        journal.append(&record).unwrap();
        journal.append(&record).unwrap();
        journal.append(&stamped("dup", "boot", 1_000, 6)).unwrap();
        let report = journal.read_all().unwrap();
        assert_eq!(report.records.len(), 2, "{}", candidate.label());
        assert_eq!(report.duplicates_removed, 1, "{}", candidate.label());
    });
}

#[test]
fn the_active_file_and_the_generations_are_all_read() {
    each_candidate(|candidate, dir| {
        let journal = candidate.open(dir);
        let records = sequence(6);
        for (index, record) in records.iter().enumerate() {
            journal.append(record).unwrap();
            if index % 2 == 1 {
                journal.rotate(1_800_000_000_000 + index as u64).unwrap();
            }
        }
        assert_eq!(
            journal.read_all().unwrap().records,
            records,
            "{}",
            candidate.label()
        );
        assert_eq!(
            data_files(dir).len(),
            3,
            "{}: three generations and no active file",
            candidate.label()
        );
    });
}

#[test]
fn append_without_a_probe_equals_append_with_the_no_op_probe() {
    each_candidate(|candidate, dir| {
        let journal = candidate.open_with(dir, immediate());
        journal.append_probed(&sequence(1)[0], &NoProbe).unwrap();
        journal.append(&sequence(2)[1]).unwrap();
        assert_eq!(journal.read_probed(&NoProbe).unwrap().records, sequence(2));
    });
}
