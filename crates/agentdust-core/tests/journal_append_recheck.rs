mod journal_support;
mod scratch;

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;

use agentdust_core::journal::{Appended, FrameWriter, JournalError, ReadReport, Record};
use journal_support::{
    Pause, Paused, RemoveAfterWrite, RotateAfterEachWrite, WritePoint, compact, copies_on_disk, count_of,
    frames, generation_name, journal, named, paused, plant, rotate_by_rename,
};
use scratch::TempDir;

const SEED: &str = "seed";
const EXPIRED: &str = "expired";
const KEEPER: &str = "keeper";
const VICTIM: &str = "victim";

type Written = Result<Appended, JournalError>;

fn not_expired(record: &Record) -> bool {
    record.session_id != EXPIRED
}

fn started(name: &str, records: &[&str]) -> TempDir {
    let dir = TempDir::private(name);
    let records: Vec<Record> = records
        .iter()
        .enumerate()
        .map(|(i, session)| {
            let mut record = named(session);
            record.mono_ts = i as u64 + 1;
            record
        })
        .collect();
    plant(&dir, "journal.jsonl", &frames(&records));
    dir
}

fn victim() -> Record {
    let mut victim = named(VICTIM);
    victim.mono_ts = 50;
    victim
}

fn with_paused_writer<R>(
    dir: &Path,
    point: WritePoint,
    attempt: u32,
    during: impl FnOnce() -> R,
) -> (Written, R) {
    let (mut writer, pause): (Paused, Pause) = paused(point, attempt);
    let record = victim();
    thread::scope(|scope| {
        let appending = scope.spawn(|| journal(dir).append_with(&record, &mut writer));
        pause.wait_reached();
        let observed = during();
        pause.release();
        (appending.join().unwrap(), observed)
    })
}

fn read(dir: &Path) -> ReadReport {
    journal(dir).read().unwrap()
}

fn attempts(written: &Written) -> u32 {
    written.as_ref().expect("the append was acknowledged").attempts
}

#[test]
fn variant_one_the_writer_resumes_after_the_rotation_and_the_record_is_present_exactly_once() {
    let dir = started("recheck-one", &[SEED, EXPIRED, KEEPER]);
    let (written, during) = with_paused_writer(&dir, WritePoint::BeforeWrite, 1, || {
        rotate_by_rename(&dir, 100);
        read(&dir)
    });
    assert_eq!(count_of(&during.records, SEED), 1);
    assert_eq!(count_of(&during.records, VICTIM), 0);
    assert_eq!(attempts(&written), 2);

    let after = read(&dir);
    assert_eq!(count_of(&after.records, VICTIM), 1);
    assert_eq!(after.duplicates_removed, 1);
    assert_eq!(copies_on_disk(&dir, VICTIM), 2);

    compact(&dir, 100, not_expired);
    let retained = read(&dir);
    assert_eq!(count_of(&retained.records, VICTIM), 1);
    assert_eq!(count_of(&retained.records, SEED), 1);
    assert_eq!(count_of(&retained.records, KEEPER), 1);
    assert_eq!(count_of(&retained.records, EXPIRED), 0);

    rotate_by_rename(&dir, 200);
    compact(&dir, 200, not_expired);
    let last = read(&dir);
    assert_eq!(count_of(&last.records, VICTIM), 1);
    assert_eq!(count_of(&last.records, SEED), 1);
}

#[test]
fn variant_two_a_pause_past_the_retention_that_replaced_the_generation_loses_nothing() {
    let dir = started("recheck-two-replaced", &[SEED, EXPIRED, KEEPER]);
    let (written, ()) = with_paused_writer(&dir, WritePoint::BeforeWrite, 1, || {
        rotate_by_rename(&dir, 100);
        compact(&dir, 100, not_expired);
    });
    assert_eq!(attempts(&written), 2);
    assert_eq!(copies_on_disk(&dir, VICTIM), 1);
    let after = read(&dir);
    assert_eq!(count_of(&after.records, VICTIM), 1);
    assert_eq!(after.duplicates_removed, 0);
    assert_eq!(count_of(&after.records, SEED), 1);
    assert_eq!(count_of(&after.records, EXPIRED), 0);
}

#[test]
fn variant_two_a_pause_past_the_retention_that_deleted_the_generation_loses_nothing() {
    let dir = started("recheck-two-deleted", &[EXPIRED]);
    let (written, ()) = with_paused_writer(&dir, WritePoint::BeforeWrite, 1, || {
        rotate_by_rename(&dir, 100);
        compact(&dir, 100, not_expired);
    });
    assert!(!dir.join(generation_name(100)).exists());
    assert_eq!(attempts(&written), 2);
    assert_eq!(copies_on_disk(&dir, VICTIM), 1);
    assert_eq!(count_of(&read(&dir).records, VICTIM), 1);
}

#[test]
fn a_pause_between_the_write_and_the_recheck_across_a_rotation_stores_the_record_once_when_read() {
    let dir = started("recheck-after-write-rotated", &[SEED, EXPIRED]);
    let (written, ()) = with_paused_writer(&dir, WritePoint::AfterWrite, 1, || {
        rotate_by_rename(&dir, 100);
    });
    assert_eq!(attempts(&written), 2);
    assert_eq!(copies_on_disk(&dir, VICTIM), 2);
    let after = read(&dir);
    assert_eq!(count_of(&after.records, VICTIM), 1);
    assert_eq!(after.duplicates_removed, 1);
}

#[test]
fn a_pause_between_the_write_and_the_recheck_across_a_compaction_stores_the_record_once_when_read() {
    let dir = started("recheck-after-write-compacted", &[SEED, EXPIRED]);
    let (written, ()) = with_paused_writer(&dir, WritePoint::AfterWrite, 1, || {
        rotate_by_rename(&dir, 100);
        compact(&dir, 100, not_expired);
    });
    assert_eq!(attempts(&written), 2);
    assert_eq!(copies_on_disk(&dir, VICTIM), 2);
    let after = read(&dir);
    assert_eq!(count_of(&after.records, VICTIM), 1);
    assert_eq!(count_of(&after.records, SEED), 1);
    assert_eq!(count_of(&after.records, EXPIRED), 0);
}

#[test]
fn a_pause_between_the_write_and_the_recheck_without_any_rotation_needs_one_attempt() {
    let dir = started("recheck-after-write-quiet", &[SEED]);
    let (written, ()) = with_paused_writer(&dir, WritePoint::AfterWrite, 1, || {});
    assert_eq!(attempts(&written), 1);
    assert_eq!(copies_on_disk(&dir, VICTIM), 1);
}

struct StaleThenPaused {
    inner: Paused,
    dir: PathBuf,
    seen: u32,
}

impl FrameWriter for StaleThenPaused {
    fn write(&mut self, file: &File, frame: &[u8]) -> io::Result<usize> {
        self.seen += 1;
        let written = self.inner.write(file, frame)?;
        if self.seen == 1 {
            rotate_by_rename(&self.dir, 10);
        }
        Ok(written)
    }
}

#[test]
fn a_rotation_during_the_second_attempt_costs_a_third_attempt_and_every_copy_collapses() {
    let dir = started("recheck-third-attempt", &[SEED]);
    let (inner, pause) = paused(WritePoint::BeforeWrite, 2);
    let mut writer = StaleThenPaused {
        inner,
        dir: dir.to_path_buf(),
        seen: 0,
    };
    let written = thread::scope(|scope| {
        let appending = scope.spawn(|| journal(&dir).append_with(&victim(), &mut writer));
        pause.wait_reached();
        rotate_by_rename(&dir, 20);
        pause.release();
        appending.join().unwrap()
    });
    assert_eq!(attempts(&written), 3);
    assert_eq!(copies_on_disk(&dir, VICTIM), 3);
    let report = read(&dir);
    assert_eq!(count_of(&report.records, VICTIM), 1);
    assert_eq!(report.duplicates_removed, 2);
    assert_eq!(count_of(&report.records, SEED), 1);
}

#[test]
fn an_active_file_replaced_under_every_attempt_fails_with_stale_and_the_copies_are_still_read() {
    let dir = started("recheck-stale", &[SEED]);
    let mut writer = RotateAfterEachWrite {
        dir: &dir,
        next_stamp: 10,
    };
    let outcome = journal(&dir).append_with(&victim(), &mut writer);
    match outcome {
        Err(JournalError::Stale { attempts }) => assert_eq!(attempts, 3),
        other => panic!("{other:?}"),
    }
    assert_eq!(copies_on_disk(&dir, VICTIM), 3);
    let report = read(&dir);
    assert_eq!(count_of(&report.records, VICTIM), 1);
    assert_eq!(report.duplicates_removed, 2);
}

#[test]
fn an_active_file_unlinked_after_the_write_is_created_again_and_the_record_is_written_again() {
    let dir = started("recheck-unlinked", &[SEED]);
    let mut writer = RemoveAfterWrite { dir: &dir, left: 1 };
    let written = journal(&dir).append_with(&victim(), &mut writer);
    assert_eq!(attempts(&written), 2);
    assert_eq!(copies_on_disk(&dir, VICTIM), 1);
    assert_eq!(count_of(&read(&dir).records, VICTIM), 1);
}

#[test]
fn an_active_file_unlinked_under_every_attempt_fails_with_stale_and_leaves_no_copy() {
    let dir = started("recheck-always-unlinked", &[SEED]);
    let mut writer = RemoveAfterWrite { dir: &dir, left: 3 };
    let outcome = journal(&dir).append_with(&victim(), &mut writer);
    assert!(matches!(outcome, Err(JournalError::Stale { attempts: 3 })));
    assert_eq!(copies_on_disk(&dir, VICTIM), 0);
}
