mod journal_support;
mod scratch;

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::thread;

use agentdust_core::journal::{JournalError, MAX_FRAME_LEN, Record, SCHEMA_VERSION};
use agentdust_core::safe_open::SafeOpenError;
use journal_support::{
    Scripted, bare_line, frame, frames, join, journal, named, names_in, padded_to_frame_len, plant, record,
    sessions,
};
use scratch::{TempDir, make_fifo, returns_promptly};

fn append(dir: &Path, record: &Record) -> Result<(), JournalError> {
    journal(dir).append(record).map(drop)
}

fn refused(result: Result<(), JournalError>) -> SafeOpenError {
    match result {
        Err(JournalError::Refused(refusal)) => refusal,
        other => panic!("{other:?}"),
    }
}

fn mode_of(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

#[test]
fn an_append_is_one_write_call_of_one_whole_frame() {
    let dir = TempDir::absent("append-one-call");
    let mut writer = Scripted::new([]);
    let appended = journal(&dir).append_with(&named("a"), &mut writer).unwrap();
    assert_eq!(appended.attempts, 1);
    assert_eq!(writer.calls, [frame(&named("a")).len()]);
    assert_eq!(fs::read(dir.join("journal.jsonl")).unwrap(), frame(&named("a")));
}

#[test]
fn appended_frames_read_back_in_order() {
    let dir = TempDir::absent("append-roundtrip");
    let (a, b, c) = (
        record("a", "b", 1, 1),
        record("b", "b", 2, 2),
        record("c", "b", 3, 3),
    );
    for r in [&a, &b, &c] {
        append(&dir, r).unwrap();
    }
    assert_eq!(
        fs::read(dir.join("journal.jsonl")).unwrap(),
        frames(&[a.clone(), b.clone(), c.clone()])
    );
    let report = journal(&dir).read().unwrap();
    assert_eq!(report.records, vec![a, b, c]);
    assert_eq!(report.skipped_lines(), 0);
}

#[test]
fn the_first_append_creates_the_directory_and_the_journal_with_private_modes() {
    let root = TempDir::private("append-first");
    let dir = root.join("nested/data");
    append(&dir, &named("a")).unwrap();
    assert_eq!(mode_of(&dir), 0o700);
    assert_eq!(mode_of(&root.join("nested")), 0o700);
    assert_eq!(mode_of(&dir.join("journal.jsonl")), 0o600);
}

#[test]
fn an_append_creates_no_lock_file() {
    let dir = TempDir::absent("append-no-lock");
    append(&dir, &named("a")).unwrap();
    append(&dir, &named("b")).unwrap();
    journal(&dir).read().unwrap();
    assert_eq!(names_in(&dir), ["journal.jsonl"]);
}

#[test]
fn a_held_journal_lock_blocks_neither_an_append_nor_a_read() {
    let dir = TempDir::absent("append-lock-held");
    append(&dir, &named("a")).unwrap();
    let lock = File::create(dir.join("journal.lock")).unwrap();
    lock.lock().unwrap();
    let target = dir.to_path_buf();
    let report = returns_promptly(move || {
        append(&target, &named("b")).unwrap();
        journal(&target).read().unwrap()
    });
    assert_eq!(report.records.len(), 2);
    drop(lock);
}

#[test]
fn an_append_adds_to_a_journal_that_already_holds_frames() {
    let dir = TempDir::absent("append-existing");
    for session in ["a", "b", "c"] {
        append(&dir, &named(session)).unwrap();
    }
    let expected: Vec<u8> = ["a", "b", "c"].iter().flat_map(|s| frame(&named(s))).collect();
    assert_eq!(fs::read(dir.join("journal.jsonl")).unwrap(), expected);
}

#[test]
fn an_append_after_a_torn_tail_keeps_the_new_record_readable() {
    let dir = TempDir::private("append-after-torn");
    let bytes = join(&[
        &frame(&record("old", "b", 1, 1)),
        b"\x1e{\"v\":1,\"kind\":\"shell_st",
    ]);
    plant(&dir, "journal.jsonl", &bytes);
    append(&dir, &record("new", "b", 2, 2)).unwrap();
    let report = journal(&dir).read().unwrap();
    assert_eq!(sessions(&report.records), ["old", "new"]);
    assert_eq!((report.torn_frames, report.malformed_lines), (1, 0));
    let stored = fs::read(dir.join("journal.jsonl")).unwrap();
    assert!(stored.starts_with(&bytes));
}

#[test]
fn an_append_after_bare_lines_of_an_older_writer_keeps_both_readable() {
    let dir = TempDir::private("append-after-bare");
    plant(&dir, "journal.jsonl", &bare_line(&record("old", "b", 1, 1)));
    append(&dir, &record("new", "b", 2, 2)).unwrap();
    assert_eq!(sessions(&journal(&dir).read().unwrap().records), ["old", "new"]);
}

#[test]
fn an_append_never_rewrites_a_line_of_a_newer_version_even_over_the_cap() {
    let dir = TempDir::private("append-newer");
    let huge = format!(
        "\u{1e}{{\"v\":3,\"pad\":\"{}\"}}\n",
        "z".repeat(3 * MAX_FRAME_LEN)
    )
    .into_bytes();
    let before = join(&[&frame(&record("old", "b", 1, 1)), &huge]);
    plant(&dir, "journal.jsonl", &before);
    append(&dir, &record("new", "b", 2, 2)).unwrap();
    let stored = fs::read(dir.join("journal.jsonl")).unwrap();
    assert!(stored.starts_with(&before));
    assert_eq!(&stored[before.len()..], &frame(&record("new", "b", 2, 2))[..]);
    let report = journal(&dir).read().unwrap();
    assert_eq!(sessions(&report.records), ["old", "new"]);
    assert_eq!(report.newer_version_lines, 1);
    assert!(report.unsupported_version);
}

#[test]
fn a_frame_over_the_cap_is_refused_before_anything_is_created() {
    let dir = TempDir::absent("append-oversized");
    match append(&dir, &padded_to_frame_len(MAX_FRAME_LEN + 1, "big")) {
        Err(JournalError::TooLarge { len, max }) => {
            assert_eq!((len, max), (MAX_FRAME_LEN + 1, MAX_FRAME_LEN))
        }
        other => panic!("{other:?}"),
    }
    assert!(!dir.exists());
}

#[test]
fn a_frame_over_the_cap_leaves_an_existing_journal_untouched() {
    let dir = TempDir::absent("append-oversized-existing");
    append(&dir, &named("kept")).unwrap();
    let before = fs::read(dir.join("journal.jsonl")).unwrap();
    assert!(append(&dir, &padded_to_frame_len(MAX_FRAME_LEN + 1, "big")).is_err());
    assert_eq!(fs::read(dir.join("journal.jsonl")).unwrap(), before);
}

#[test]
fn a_frame_of_exactly_the_cap_is_written_whole() {
    let dir = TempDir::absent("append-edge");
    let mut writer = Scripted::new([]);
    journal(&dir)
        .append_with(&padded_to_frame_len(MAX_FRAME_LEN, "edge"), &mut writer)
        .unwrap();
    assert_eq!(writer.calls, [MAX_FRAME_LEN]);
    assert_eq!(journal(&dir).read().unwrap().records.len(), 1);
}

#[test]
fn a_record_of_another_schema_version_is_refused_and_writes_nothing() {
    let dir = TempDir::absent("append-version");
    for v in [0, SCHEMA_VERSION - 1, SCHEMA_VERSION + 1, u32::MAX] {
        let mut wrong = named("a");
        wrong.v = v;
        match append(&dir, &wrong) {
            Err(JournalError::WrongVersion { found }) => assert_eq!(found, v),
            other => panic!("{other:?}"),
        }
    }
    assert!(!dir.exists());
}

#[test]
fn a_symlinked_journal_is_refused_and_the_target_is_untouched() {
    let dir = TempDir::private("append-symlink");
    let target = dir.join("elsewhere");
    fs::write(&target, b"keep").unwrap();
    symlink(&target, dir.join("journal.jsonl")).unwrap();
    assert!(matches!(
        refused(append(&dir, &named("a"))),
        SafeOpenError::Symlink
    ));
    assert_eq!(fs::read(&target).unwrap(), b"keep");
}

#[test]
fn a_dangling_symlink_in_place_of_the_journal_is_not_created_through() {
    let dir = TempDir::private("append-dangling");
    let target = dir.join("not-yet");
    symlink(&target, dir.join("journal.jsonl")).unwrap();
    assert!(append(&dir, &named("a")).is_err());
    assert!(!target.exists());
}

#[test]
fn a_hard_linked_journal_is_refused_and_not_written() {
    let dir = TempDir::absent("append-hardlink");
    append(&dir, &named("a")).unwrap();
    fs::hard_link(dir.join("journal.jsonl"), dir.join("copy")).unwrap();
    let before = fs::read(dir.join("copy")).unwrap();
    assert!(matches!(
        refused(append(&dir, &named("b"))),
        SafeOpenError::HardLinked { links: 2 }
    ));
    assert_eq!(fs::read(dir.join("copy")).unwrap(), before);
}

#[test]
fn a_journal_with_a_loose_mode_is_refused_and_not_written() {
    let dir = TempDir::absent("append-loose-file");
    append(&dir, &named("a")).unwrap();
    fs::set_permissions(dir.join("journal.jsonl"), fs::Permissions::from_mode(0o666)).unwrap();
    let before = fs::read(dir.join("journal.jsonl")).unwrap();
    assert!(matches!(
        refused(append(&dir, &named("b"))),
        SafeOpenError::LooseMode { mode: 0o666, .. }
    ));
    assert_eq!(fs::read(dir.join("journal.jsonl")).unwrap(), before);
}

#[test]
fn a_fifo_in_place_of_the_journal_is_refused_without_blocking() {
    let dir = TempDir::private("append-fifo");
    make_fifo(&dir.join("journal.jsonl"));
    let target = dir.to_path_buf();
    let result = returns_promptly(move || append(&target, &named("a")));
    assert!(matches!(refused(result), SafeOpenError::NotRegular));
}

#[test]
fn a_directory_in_place_of_the_journal_is_refused() {
    let dir = TempDir::private("append-dir-as-file");
    fs::create_dir(dir.join("journal.jsonl")).unwrap();
    assert!(matches!(
        refused(append(&dir, &named("a"))),
        SafeOpenError::NotRegular
    ));
}

#[test]
fn a_data_directory_with_a_loose_mode_is_refused_and_keeps_its_mode() {
    let dir = TempDir::private("append-loose-dir");
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        refused(append(&dir, &named("a"))),
        SafeOpenError::LooseMode { mode: 0o755, .. }
    ));
    assert_eq!(mode_of(&dir), 0o755);
    assert!(!dir.join("journal.jsonl").exists());
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn a_symlinked_data_directory_is_refused() {
    let real = TempDir::private("append-real-dir");
    let link = TempDir::absent("append-link-dir");
    symlink(real.path(), link.path()).unwrap();
    assert!(matches!(
        refused(append(&link, &named("a"))),
        SafeOpenError::Symlink
    ));
    assert!(!real.join("journal.jsonl").exists());
}

#[test]
fn a_file_in_place_of_the_data_directory_is_refused() {
    let path = TempDir::absent("append-file-as-dir");
    fs::write(path.path(), b"keep").unwrap();
    assert!(append(&path, &named("a")).is_err());
    assert_eq!(fs::read(path.path()).unwrap(), b"keep");
    fs::remove_file(path.path()).unwrap();
}

#[test]
fn sixteen_threads_appending_4000_byte_records_tear_and_lose_nothing() {
    const WRITERS: usize = 16;
    const PER_WRITER: usize = 150;
    let dir = TempDir::absent("append-threads");
    thread::scope(|scope| {
        for writer in 0..WRITERS {
            let dir = &dir;
            scope.spawn(move || {
                for seq in 0..PER_WRITER {
                    let mut rec = padded_to_frame_len(4000, &format!("w{writer}-s{seq}-"));
                    rec.mono_ts = seq as u64;
                    journal(dir).append(&rec).unwrap();
                }
            });
        }
    });
    let report = journal(&dir).read().unwrap();
    assert_eq!(report.skipped_lines(), 0);
    assert!(!report.truncated_last_line);
    assert_eq!(report.records.len(), WRITERS * PER_WRITER);
    let seen: BTreeSet<String> = report
        .records
        .iter()
        .map(|r| r.session_id.split('-').take(2).collect::<Vec<_>>().join("-"))
        .collect();
    assert_eq!(seen.len(), WRITERS * PER_WRITER);
    assert_eq!(report.duplicates_removed, 0);
}
