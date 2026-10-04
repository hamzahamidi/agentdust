mod journal_support;
mod maintenance_support;
mod scratch;

use std::path::Path;
use std::thread;

use agentdust_core::journal::{ReadPoint, ReadReport, Record, Rotation};
use journal_support::{Gate, WritePoint, copies_on_disk, count_of, frame, frames, journal, plant};
use maintenance_support::{T, attempts, numbered, with_paused_writer};
use scratch::TempDir;

const SEED: &str = "seed";
const MIDDLE: &str = "middle";
const KEEPER: &str = "keeper";
const VICTIM: &str = "victim";

fn started(name: &str) -> TempDir {
    let dir = TempDir::private(name);
    plant(
        &dir,
        "journal.jsonl",
        &frames(&[numbered(SEED, 1), numbered(MIDDLE, 2), numbered(KEEPER, 3)]),
    );
    dir
}

fn victim() -> Record {
    numbered(VICTIM, 50)
}

fn read(dir: &Path) -> ReadReport {
    journal(dir).read().unwrap()
}

fn rotate(dir: &Path, now_ms: u64) -> Rotation {
    journal(dir).rotate(now_ms).unwrap()
}

#[test]
fn a_writer_paused_before_its_write_across_a_rotation_stores_its_record_once() {
    let dir = started("rot-before-write");
    let (written, during) = with_paused_writer(&dir, &victim(), WritePoint::BeforeWrite, 1, || {
        assert_eq!(rotate(&dir, T), Rotation::Rotated { stamp: T });
        read(&dir)
    });
    assert_eq!(count_of(&during.records, SEED), 1);
    assert_eq!(count_of(&during.records, VICTIM), 0);
    assert_eq!(attempts(&written), 2);

    let after = read(&dir);
    assert_eq!(count_of(&after.records, VICTIM), 1);
    assert_eq!(after.duplicates_removed, 1);
    assert_eq!(copies_on_disk(&dir, VICTIM), 2);
    for session in [SEED, MIDDLE, KEEPER] {
        assert_eq!(count_of(&after.records, session), 1, "{session}");
    }
}

#[test]
fn a_writer_paused_after_its_write_across_a_rotation_stores_its_record_once() {
    let dir = started("rot-after-write");
    let (written, ()) = with_paused_writer(&dir, &victim(), WritePoint::AfterWrite, 1, || {
        rotate(&dir, T);
    });
    assert_eq!(attempts(&written), 2);
    assert_eq!(copies_on_disk(&dir, VICTIM), 2);
    let after = read(&dir);
    assert_eq!(count_of(&after.records, VICTIM), 1);
    assert_eq!(after.duplicates_removed, 1);
}

#[test]
fn a_writer_paused_without_any_rotation_needs_one_attempt() {
    let dir = started("rot-quiet");
    let (written, ()) = with_paused_writer(&dir, &victim(), WritePoint::BeforeWrite, 1, || {});
    assert_eq!(attempts(&written), 1);
    assert_eq!(copies_on_disk(&dir, VICTIM), 1);
}

#[test]
fn however_many_rotations_a_paused_writer_sleeps_through_its_record_is_present_once() {
    for point in [WritePoint::BeforeWrite, WritePoint::AfterWrite] {
        for rotations in 1..=4u64 {
            let name = format!("rot-sleeps-{point:?}-{rotations}");
            let dir = started(&name);
            let (written, ()) = with_paused_writer(&dir, &victim(), point, 1, || {
                for n in 0..rotations {
                    journal(&dir)
                        .append(&numbered(&format!("filler-{n}"), 60 + n))
                        .unwrap();
                    rotate(&dir, T + n);
                }
            });
            assert_eq!(attempts(&written), 2, "{point:?} {rotations}");
            let after = read(&dir);
            assert_eq!(count_of(&after.records, VICTIM), 1, "{point:?} {rotations}");
            for session in [SEED, MIDDLE, KEEPER] {
                assert_eq!(
                    count_of(&after.records, session),
                    1,
                    "{session} {point:?} {rotations}"
                );
            }
        }
    }
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
fn a_reader_paused_after_it_opened_the_active_file_misses_nothing_when_a_rotation_follows() {
    let dir = started("rot-reader-after-open");
    let (report, ()) = read_paused_at(&dir, ReadPoint::ActiveOpened, || {
        rotate(&dir, T);
        plant(&dir, "journal.jsonl", &frame(&numbered("after", 9)));
    });
    for session in [SEED, MIDDLE, KEEPER] {
        assert_eq!(count_of(&report.records, session), 1, "{session}");
    }
    assert_eq!(report.duplicates_removed, 0);
    assert_eq!(report.unsafe_files, 0);
}

#[test]
fn a_reader_paused_after_it_listed_the_generations_misses_nothing_when_a_rotation_follows() {
    let dir = started("rot-reader-after-list");
    let (report, ()) = read_paused_at(&dir, ReadPoint::GenerationsListed, || {
        rotate(&dir, T);
    });
    for session in [SEED, MIDDLE, KEEPER] {
        assert_eq!(count_of(&report.records, session), 1, "{session}");
    }
    assert_eq!(report.duplicates_removed, 0);
}

#[test]
fn a_rotation_does_not_change_what_a_read_returns() {
    let dir = started("rot-reader-agree");
    let before = read(&dir);
    rotate(&dir, T);
    let after = read(&dir);
    assert_eq!(before.records, after.records);
}
