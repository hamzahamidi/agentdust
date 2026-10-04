mod journal_support;
mod scratch;

use std::path::Path;
use std::thread;

use agentdust_core::journal::{ReadPoint, ReadReport};
use journal_support::{
    Gate, compact, count_of, frame, frames, generation_name, journal, named, plant, rotate_by_rename,
};
use scratch::TempDir;

const SEED: &str = "seed";
const KEEPER: &str = "keeper";
const EXPIRED: &str = "expired";

fn not_expired(record: &agentdust_core::journal::Record) -> bool {
    record.session_id != EXPIRED
}

fn start(name: &str) -> TempDir {
    let dir = TempDir::private(name);
    let mut seed = named(SEED);
    seed.mono_ts = 1;
    let mut expired = named(EXPIRED);
    expired.mono_ts = 2;
    let mut keeper = named(KEEPER);
    keeper.mono_ts = 3;
    plant(&dir, "journal.jsonl", &frames(&[seed, expired, keeper]));
    dir
}

fn read_paused_at<R>(dir: &Path, point: ReadPoint, during: impl FnOnce() -> R) -> (ReadReport, R) {
    let gate = Gate::at(point);
    thread::scope(|scope| {
        let reader = scope.spawn(|| journal(dir).read_observed(&gate).unwrap());
        gate.wait_reached();
        let observed = during();
        gate.release();
        (reader.join().unwrap(), observed)
    })
}

#[test]
fn a_rotation_after_the_active_file_was_opened_leaves_every_record_present_once() {
    let dir = start("snapshot-rotate-after-open");
    let (report, ()) = read_paused_at(&dir, ReadPoint::ActiveOpened, || {
        rotate_by_rename(&dir, 100);
        plant(&dir, "journal.jsonl", &frame(&named("after")));
    });
    for session in [SEED, EXPIRED, KEEPER] {
        assert_eq!(count_of(&report.records, session), 1, "{session}");
    }
    assert_eq!(report.duplicates_removed, 0);
    assert_eq!(report.unsafe_files, 0);
}

#[test]
fn a_compaction_after_the_listing_is_read_through_the_replacement() {
    let dir = start("snapshot-compact-after-list");
    rotate_by_rename(&dir, 100);
    let (report, ()) = read_paused_at(&dir, ReadPoint::GenerationsListed, || {
        compact(&dir, 100, not_expired);
    });
    assert_eq!(count_of(&report.records, SEED), 1);
    assert_eq!(count_of(&report.records, KEEPER), 1);
    assert_eq!(count_of(&report.records, EXPIRED), 0);
    assert_eq!(report.duplicates_removed, 0);
}

#[test]
fn a_deletion_after_the_listing_skips_the_vanished_generation_and_reads_the_rest() {
    let dir = start("snapshot-delete-after-list");
    rotate_by_rename(&dir, 100);
    let mut only_expired = named(EXPIRED);
    only_expired.mono_ts = 9;
    plant(&dir, &generation_name(50), &frame(&only_expired));
    let (report, ()) = read_paused_at(&dir, ReadPoint::GenerationsListed, || {
        compact(&dir, 50, not_expired);
    });
    assert!(!dir.join(generation_name(50)).exists());
    assert_eq!(count_of(&report.records, SEED), 1);
    assert_eq!(count_of(&report.records, KEEPER), 1);
    assert_eq!(count_of(&report.records, EXPIRED), 1);
    assert_eq!(report.unsafe_files, 0);
    assert_eq!(report.skipped_lines(), 0);
}

#[test]
fn a_rotation_after_the_listing_is_covered_by_the_active_file_already_held() {
    let dir = start("snapshot-rotate-after-list");
    let (report, ()) = read_paused_at(&dir, ReadPoint::GenerationsListed, || {
        rotate_by_rename(&dir, 100);
    });
    for session in [SEED, EXPIRED, KEEPER] {
        assert_eq!(count_of(&report.records, session), 1, "{session}");
    }
    assert_eq!(report.duplicates_removed, 0);
}

#[test]
fn a_compaction_between_the_open_and_the_listing_reads_both_copies_and_loses_nothing() {
    let dir = start("snapshot-compact-after-open");
    let (report, ()) = read_paused_at(&dir, ReadPoint::ActiveOpened, || {
        rotate_by_rename(&dir, 100);
        compact(&dir, 100, not_expired);
    });
    assert_eq!(count_of(&report.records, SEED), 1);
    assert_eq!(count_of(&report.records, KEEPER), 1);
    assert_eq!(count_of(&report.records, EXPIRED), 1);
    assert_eq!(report.duplicates_removed, 2);
}

#[test]
fn a_whole_generation_deleted_between_the_open_and_the_listing_still_leaves_the_held_records() {
    let dir = start("snapshot-delete-after-open");
    let (report, ()) = read_paused_at(&dir, ReadPoint::ActiveOpened, || {
        rotate_by_rename(&dir, 100);
        compact(&dir, 100, |_| false);
    });
    for session in [SEED, EXPIRED, KEEPER] {
        assert_eq!(count_of(&report.records, session), 1, "{session}");
    }
    assert_eq!(report.duplicates_removed, 0);
}
