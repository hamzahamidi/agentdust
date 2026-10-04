mod journal_support;
mod scratch;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

use agentdust_core::journal::volume::{FixedVolume, MNT_LOCAL, classify};
use agentdust_core::journal::{Agent, Journal, JournalError, Kind, ReadReport, generation_stamp};
use agentdust_core::safe_open::SafeOpenError;
use journal_support::{
    append_raw, bare_line, edited_frame, frame, frames, generation_name, join, journal, names_in, plant,
    record, sessions,
};
use scratch::{TempDir, make_fifo, returns_promptly};
use serde_json::json;

fn read(dir: &Path) -> ReadReport {
    journal(dir).read().unwrap()
}

fn with_journal(name: &str, bytes: &[u8]) -> TempDir {
    let dir = TempDir::private(name);
    plant(&dir, "journal.jsonl", bytes);
    dir
}

fn refused(result: Result<ReadReport, JournalError>) -> SafeOpenError {
    match result {
        Err(JournalError::Refused(refusal)) => refusal,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_missing_journal_directory_reads_as_empty_and_is_not_created() {
    let dir = TempDir::absent("read-missing-dir");
    assert_eq!(read(&dir), ReadReport::default());
    assert!(!dir.exists());
}

#[test]
fn a_directory_without_a_journal_reads_as_empty_and_creates_nothing() {
    let dir = TempDir::private("read-missing-file");
    let report = read(&dir);
    assert!(report.records.is_empty());
    assert_eq!(report.skipped_lines(), 0);
    assert!(names_in(&dir).is_empty());
}

#[test]
fn a_read_never_changes_a_file_or_adds_one() {
    let newer = edited_frame(&record("future", "b", 1, 1), |v| v["v"] = json!(2));
    let bytes = join(&[&frame(&record("a", "b", 1, 1)), &newer, b"\x1e{\"v\":1,\"kin"]);
    let dir = with_journal("read-readonly", &bytes);
    plant(&dir, &generation_name(5), &newer);
    let before: Vec<(String, Vec<u8>)> = names_in(&dir)
        .into_iter()
        .map(|name| {
            let content = fs::read(dir.join(&name)).unwrap();
            (name, content)
        })
        .collect();
    let report = read(&dir);
    assert_eq!(report.newer_version_lines, 2);
    let after: Vec<(String, Vec<u8>)> = names_in(&dir)
        .into_iter()
        .map(|name| {
            let content = fs::read(dir.join(&name)).unwrap();
            (name, content)
        })
        .collect();
    assert_eq!(before, after);
}

#[test]
fn bare_lines_without_the_separator_are_read_as_records() {
    let (a, b) = (record("a", "b", 1, 1), record("b", "b", 2, 2));
    let dir = with_journal("read-bare", &join(&[&bare_line(&a), &frame(&b)]));
    assert_eq!(sessions(&read(&dir).records), ["a", "b"]);
}

#[test]
fn a_non_utf8_line_in_the_file_does_not_abort_the_read() {
    let (a, b) = (record("a", "b", 1, 1), record("b", "b", 2, 2));
    let dir = with_journal(
        "read-non-utf8",
        &join(&[&frame(&a), b"\x1e\xff\xfe\xfd\n", &frame(&b)]),
    );
    let report = read(&dir);
    assert_eq!(sessions(&report.records), ["a", "b"]);
    assert_eq!(report.malformed_lines, 1);
}

#[test]
fn a_symlinked_journal_is_refused_and_not_followed() {
    let dir = TempDir::private("read-symlink");
    let target = dir.join("elsewhere");
    fs::write(&target, frame(&record("a", "b", 1, 1))).unwrap();
    symlink(&target, dir.join("journal.jsonl")).unwrap();
    assert!(matches!(refused(journal(&dir).read()), SafeOpenError::Symlink));
}

#[test]
fn a_hard_linked_journal_is_refused() {
    let dir = with_journal("read-hardlink", &frame(&record("a", "b", 1, 1)));
    fs::hard_link(dir.join("journal.jsonl"), dir.join("copy")).unwrap();
    assert!(matches!(
        refused(journal(&dir).read()),
        SafeOpenError::HardLinked { links: 2 }
    ));
}

#[test]
fn a_journal_with_a_loose_mode_is_refused() {
    let dir = with_journal("read-loose-file", &frame(&record("a", "b", 1, 1)));
    fs::set_permissions(dir.join("journal.jsonl"), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        refused(journal(&dir).read()),
        SafeOpenError::LooseMode { mode: 0o644, .. }
    ));
}

#[test]
fn a_fifo_in_place_of_the_journal_is_refused_without_blocking() {
    let dir = TempDir::private("read-fifo");
    make_fifo(&dir.join("journal.jsonl"));
    let target = dir.to_path_buf();
    let result = returns_promptly(move || journal(&target).read());
    assert!(matches!(refused(result), SafeOpenError::NotRegular));
}

#[test]
fn a_directory_in_place_of_the_journal_is_refused() {
    let dir = TempDir::private("read-dir-as-file");
    fs::create_dir(dir.join("journal.jsonl")).unwrap();
    assert!(matches!(refused(journal(&dir).read()), SafeOpenError::NotRegular));
}

#[test]
fn a_data_directory_with_a_loose_mode_is_refused() {
    let dir = with_journal("read-loose-dir", &frame(&record("a", "b", 1, 1)));
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        refused(journal(&dir).read()),
        SafeOpenError::LooseMode { mode: 0o755, .. }
    ));
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn a_symlinked_data_directory_is_refused() {
    let real = with_journal("read-real-dir", &frame(&record("a", "b", 1, 1)));
    let link = TempDir::absent("read-link-dir");
    symlink(real.path(), link.path()).unwrap();
    assert!(matches!(refused(journal(&link).read()), SafeOpenError::Symlink));
}

#[test]
fn a_file_in_place_of_the_data_directory_is_refused() {
    let dir = TempDir::absent("read-file-as-dir");
    fs::write(dir.path(), b"x").unwrap();
    assert!(matches!(
        refused(journal(&dir).read()),
        SafeOpenError::NotDirectory
    ));
    fs::remove_file(dir.path()).unwrap();
}

#[test]
fn a_journal_from_a_newer_version_reads_what_it_can_and_raises_the_flag() {
    let a = record("a", "b", 1, 1);
    let future = edited_frame(&record("future", "b", 2, 2), |v| v["v"] = json!(2));
    let dir = with_journal(
        "read-future",
        &join(&[&frame(&a), &future, b"\x1e{\"v\":1,\"kind\":\"sam"]),
    );
    let report = read(&dir);
    assert_eq!(sessions(&report.records), ["a"]);
    assert_eq!(report.newer_version_lines, 1);
    assert!(report.unsupported_version);
    assert!(report.truncated_last_line);
}

#[test]
fn a_newer_version_line_over_the_cap_in_the_journal_is_counted_and_left_in_place() {
    let huge = format!("\u{1e}{{\"v\":2,\"pad\":\"{}\"}}\n", "z".repeat(200_000)).into_bytes();
    let bytes = join(&[&frame(&record("a", "b", 1, 1)), &huge]);
    let dir = with_journal("read-huge-future", &bytes);
    let report = read(&dir);
    assert_eq!(sessions(&report.records), ["a"]);
    assert_eq!((report.newer_version_lines, report.malformed_lines), (1, 0));
    assert!(report.unsupported_version);
    assert_eq!(fs::read(dir.join("journal.jsonl")).unwrap(), bytes);
}

#[test]
fn records_are_ordered_by_monotonic_time_and_not_by_file_position() {
    let dir = with_journal(
        "read-order-mono",
        &frames(&[
            record("late", "b", 100, 30),
            record("early", "b", 100, 10),
            record("middle", "b", 100, 20),
        ]),
    );
    assert_eq!(sessions(&read(&dir).records), ["early", "middle", "late"]);
}

#[test]
fn a_boot_that_first_appears_earlier_comes_first_whatever_its_monotonic_values() {
    let dir = with_journal(
        "read-order-boot",
        &frames(&[
            record("new-1", "boot-new", 2_000, 5),
            record("old-1", "boot-old", 1_000, 900),
            record("new-2", "boot-new", 2_001, 6),
            record("old-2", "boot-old", 1_001, 901),
        ]),
    );
    assert_eq!(
        sessions(&read(&dir).records),
        ["old-1", "old-2", "new-1", "new-2"]
    );
}

#[test]
fn a_boot_is_placed_by_its_earliest_wall_time_and_not_by_its_first_line() {
    let dir = with_journal(
        "read-order-first-wall",
        &frames(&[
            record("b2-late-line", "boot-2", 100, 1),
            record("b1", "boot-1", 50, 1),
            record("b2-early-stamp", "boot-2", 5, 2),
        ]),
    );
    assert_eq!(
        sessions(&read(&dir).records),
        ["b2-late-line", "b2-early-stamp", "b1"]
    );
}

#[test]
fn equal_first_wall_times_are_split_by_boot_name_so_the_order_is_stable() {
    let from_b = record("from-b", "boot-b", 100, 1);
    let from_a = record("from-a", "boot-a", 100, 1);
    let first = with_journal("read-boot-tie-1", &frames(&[from_b.clone(), from_a.clone()]));
    let second = with_journal("read-boot-tie-2", &frames(&[from_a, from_b]));
    assert_eq!(sessions(&read(&first).records), ["from-a", "from-b"]);
    assert_eq!(read(&first).records, read(&second).records);
}

#[test]
fn records_equal_in_boot_and_stamps_get_a_fixed_order_that_does_not_depend_on_the_files() {
    let (a, b, c) = (
        record("a", "b", 100, 7),
        record("b", "b", 100, 7),
        record("c", "b", 100, 7),
    );
    let one_file = with_journal("read-tie-one", &frames(&[c.clone(), a.clone(), b.clone()]));
    let spread = with_journal("read-tie-spread", &frame(&b));
    plant(&spread, &generation_name(9), &frame(&c));
    plant(&spread, &generation_name(3), &frame(&a));
    let reversed = with_journal("read-tie-reversed", &frames(&[b.clone(), a.clone(), c.clone()]));
    let expected = vec![a, b, c];
    assert_eq!(read(&one_file).records, expected);
    assert_eq!(read(&spread).records, expected);
    assert_eq!(read(&reversed).records, expected);
}

#[test]
fn the_final_key_compares_every_field_in_declaration_order_starting_with_the_kind() {
    let mut ended = record("a", "b", 100, 7);
    ended.kind = Kind::ShellEnd;
    let started = record("z", "b", 100, 7);
    let mut codex = record("a", "b", 100, 7);
    codex.agent = Agent::Codex;
    let claude = record("a", "b", 100, 7);
    let dir = with_journal("read-final-key", &frames(&[ended, codex, claude, started]));
    let order: Vec<(Kind, Agent, String)> = read(&dir)
        .records
        .into_iter()
        .map(|r| (r.kind, r.agent, r.session_id))
        .collect();
    assert_eq!(
        order,
        [
            (Kind::ShellStart, Agent::Claude, "a".to_owned()),
            (Kind::ShellStart, Agent::Claude, "z".to_owned()),
            (Kind::ShellStart, Agent::Codex, "a".to_owned()),
            (Kind::ShellEnd, Agent::Claude, "a".to_owned()),
        ]
    );
}

#[test]
fn records_that_differ_in_an_optional_field_are_not_collapsed() {
    let plain = record("a", "b", 1, 1);
    let mut keyed = plain.clone();
    keyed.cwd_key = Some("abc123".try_into().unwrap());
    let mut named = plain.clone();
    named.exe_base = Some("node".try_into().unwrap());
    let dir = with_journal("read-optional-differs", &frames(&[plain, keyed, named]));
    let report = read(&dir);
    assert_eq!(report.records.len(), 3);
    assert_eq!(report.duplicates_removed, 0);
}

#[test]
fn exact_duplicates_in_one_file_and_across_files_collapse_and_are_counted() {
    let a = record("a", "b", 1, 1);
    let b = record("b", "b", 2, 2);
    let dir = with_journal("read-dupes", &frames(&[a.clone(), a.clone(), b.clone()]));
    plant(&dir, &generation_name(4), &frames(&[a.clone(), b.clone()]));
    let report = read(&dir);
    assert_eq!(report.records, vec![a, b]);
    assert_eq!(report.duplicates_removed, 3);
}

#[test]
fn a_record_that_differs_in_one_field_is_not_a_duplicate() {
    let a = record("a", "b", 1, 1);
    let mut later = a.clone();
    later.wall_ts += 1;
    let mut other_tool = a.clone();
    other_tool.tool_use_id = Some("toolu_2".to_owned());
    let dir = with_journal("read-not-dupes", &frames(&[a, later, other_tool]));
    assert_eq!(read(&dir).records.len(), 3);
}

#[test]
fn every_generation_is_read_with_the_active_file() {
    let dir = with_journal("read-generations", &frame(&record("active", "b", 3, 3)));
    plant(
        &dir,
        &generation_name(20),
        &frame(&record("newer-gen", "b", 2, 2)),
    );
    plant(
        &dir,
        &generation_name(10),
        &frame(&record("older-gen", "b", 1, 1)),
    );
    let report = read(&dir);
    assert_eq!(sessions(&report.records), ["older-gen", "newer-gen", "active"]);
    assert_eq!(report.unsafe_files, 0);
}

#[test]
fn generations_are_read_when_there_is_no_active_file() {
    let dir = TempDir::private("read-no-active");
    plant(&dir, &generation_name(10), &frame(&record("only", "b", 1, 1)));
    assert_eq!(sessions(&read(&dir).records), ["only"]);
}

#[test]
fn only_canonical_generation_names_are_generations() {
    assert_eq!(generation_stamp("journal.0.jsonl"), Some(0));
    assert_eq!(
        generation_stamp("journal.1800000000000.jsonl"),
        Some(1_800_000_000_000)
    );
    assert_eq!(
        generation_stamp("journal.18446744073709551615.jsonl"),
        Some(u64::MAX)
    );
    for name in [
        "journal.jsonl",
        "journal..jsonl",
        "journal.01.jsonl",
        "journal.00.jsonl",
        "journal.+1.jsonl",
        "journal.-1.jsonl",
        "journal.1.jsonl.bak",
        "journal.1.json",
        "journal.18446744073709551616.jsonl",
        "journal.1a.jsonl",
        "journal.compact.tmp",
        "journal.maint",
        "xjournal.1.jsonl",
        "journal.1.jsonl ",
    ] {
        assert_eq!(generation_stamp(name), None, "{name}");
    }
}

#[test]
fn files_that_are_not_generations_are_not_read() {
    let dir = with_journal("read-not-generations", &frame(&record("active", "b", 3, 3)));
    for name in [
        "journal.01.jsonl",
        "journal.jsonl.bak",
        "journal.compact.tmp",
        "journal.maint",
        "other.jsonl",
    ] {
        plant(&dir, name, &frame(&record(name, "b", 1, 1)));
    }
    let report = read(&dir);
    assert_eq!(sessions(&report.records), ["active"]);
    assert_eq!(report.unsafe_files, 0);
}

#[test]
fn a_symlinked_generation_is_not_followed_and_is_counted() {
    let dir = with_journal("read-gen-symlink", &frame(&record("active", "b", 3, 3)));
    let target = dir.join("elsewhere");
    fs::write(&target, frame(&record("hidden", "b", 1, 1))).unwrap();
    symlink(&target, dir.join(generation_name(5))).unwrap();
    let report = read(&dir);
    assert_eq!(sessions(&report.records), ["active"]);
    assert_eq!(report.unsafe_files, 1);
    assert_eq!(fs::read(&target).unwrap(), frame(&record("hidden", "b", 1, 1)));
}

#[test]
fn a_dangling_symlink_named_as_a_generation_is_counted() {
    let dir = with_journal("read-gen-dangling", &frame(&record("active", "b", 3, 3)));
    symlink(dir.join("nowhere"), dir.join(generation_name(5))).unwrap();
    let report = read(&dir);
    assert_eq!(sessions(&report.records), ["active"]);
    assert_eq!(report.unsafe_files, 1);
}

#[test]
fn a_hard_linked_a_loose_and_a_directory_generation_are_counted_and_skipped() {
    let dir = with_journal("read-gen-unsafe", &frame(&record("active", "b", 3, 3)));
    plant(&dir, &generation_name(1), &frame(&record("linked", "b", 1, 1)));
    fs::hard_link(dir.join(generation_name(1)), dir.join("alias")).unwrap();
    plant(&dir, &generation_name(2), &frame(&record("loose", "b", 1, 1)));
    fs::set_permissions(dir.join(generation_name(2)), fs::Permissions::from_mode(0o644)).unwrap();
    fs::create_dir(dir.join(generation_name(3))).unwrap();
    plant(&dir, &generation_name(4), &frame(&record("fine", "b", 2, 2)));
    let report = read(&dir);
    assert_eq!(sessions(&report.records), ["fine", "active"]);
    assert_eq!(report.unsafe_files, 3);
}

#[test]
fn a_fifo_named_as_a_generation_is_skipped_without_blocking() {
    let dir = with_journal("read-gen-fifo", &frame(&record("active", "b", 3, 3)));
    make_fifo(&dir.join(generation_name(5)));
    let target = dir.to_path_buf();
    let report = returns_promptly(move || journal(&target).read()).unwrap();
    assert_eq!(sessions(&report.records), ["active"]);
    assert_eq!(report.unsafe_files, 1);
}

#[test]
fn counters_and_flags_add_up_across_every_file() {
    let dir = with_journal(
        "read-aggregate",
        &join(&[&frame(&record("a", "b", 3, 3)), b"\x1ebroken\n", b"\x1e{\"v\":1"]),
    );
    plant(
        &dir,
        &generation_name(2),
        &join(&[
            &frame(&record("g", "b", 1, 1)),
            &edited_frame(&record("f", "b", 1, 1), |v| v["v"] = json!(2)),
            &edited_frame(&record("u", "b", 1, 1), |v| v["kind"] = json!("hologram")),
        ]),
    );
    let report = read(&dir);
    assert_eq!(sessions(&report.records), ["g", "a"]);
    assert_eq!(report.malformed_lines, 1);
    assert_eq!(report.torn_frames, 1);
    assert_eq!(report.newer_version_lines, 1);
    assert_eq!(report.unknown_kind_lines, 1);
    assert!(report.unsupported_version && report.truncated_last_line);
    assert_eq!(report.skipped_lines(), 4);
}

#[test]
fn a_torn_tail_in_a_closed_generation_counts_as_a_truncated_last_line() {
    let dir = with_journal("read-gen-torn", &frame(&record("a", "b", 3, 3)));
    plant(&dir, &generation_name(2), b"\x1e{\"v\":1,\"ki");
    let report = read(&dir);
    assert!(report.truncated_last_line);
    assert_eq!(report.torn_frames, 1);
}

#[test]
fn the_report_names_the_file_system_of_the_directory() {
    let dir = with_journal("read-fs", &frame(&record("a", "b", 1, 1)));
    let report = read(&dir);
    assert_eq!(report.filesystem, Some(classify("apfs", MNT_LOCAL)));
    assert!(!report.on_unsupported_filesystem());
}

#[test]
fn a_journal_on_an_unsupported_volume_is_still_read_and_the_report_says_so() {
    let dir = with_journal("read-nfs", &frame(&record("a", "b", 1, 1)));
    let nfs = FixedVolume::new(classify("nfs", 0));
    let report = Journal::with_volume(&dir, &nfs).read().unwrap();
    assert_eq!(sessions(&report.records), ["a"]);
    assert_eq!(report.filesystem, Some(classify("nfs", 0)));
    assert!(report.on_unsupported_filesystem());
}

#[test]
fn a_probe_that_fails_leaves_the_file_system_unknown_and_does_not_stop_the_read() {
    struct Failing;
    impl agentdust_core::journal::volume::VolumeProbe for Failing {
        fn probe(&self, _path: &Path) -> std::io::Result<agentdust_core::journal::volume::FsFacts> {
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        }
    }
    let dir = with_journal("read-fs-unknown", &frame(&record("a", "b", 1, 1)));
    let report = Journal::with_volume(&dir, &Failing).read().unwrap();
    assert_eq!(sessions(&report.records), ["a"]);
    assert_eq!(report.filesystem, None);
    assert!(!report.on_unsupported_filesystem());
}

#[test]
fn the_status_names_the_volume_even_when_the_directory_does_not_exist_yet() {
    let missing = TempDir::absent("status-missing");
    let nfs = FixedVolume::new(classify("nfs", 0));
    let facts = Journal::with_volume(&missing, &nfs).status().unwrap();
    assert_eq!(facts.describe(), "nfs, not local, not supported");
    assert!(!missing.exists());
}

#[test]
fn a_read_sees_records_appended_after_a_previous_read() {
    let dir = with_journal("read-twice", &frame(&record("a", "b", 1, 1)));
    assert_eq!(read(&dir).records.len(), 1);
    append_raw(&dir, "journal.jsonl", &frame(&record("b", "b", 2, 2)));
    assert_eq!(sessions(&read(&dir).records), ["a", "b"]);
}
