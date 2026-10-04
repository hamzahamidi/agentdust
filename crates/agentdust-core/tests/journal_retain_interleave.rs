mod journal_support;
mod maintenance_support;
mod scratch;

use std::path::Path;
use std::thread;

use agentdust_core::journal::{
    MaintenanceError, MaintenancePoint, PruneReport, ReadPoint, ReadReport, Record,
};
use journal_support::{
    Gate, WritePoint, copies_on_disk, count_of, frames, generation_name, journal, paused, plant,
};
use maintenance_support::{
    BOOT, MaintenanceGate, T, attempts, expired, keep_everything, numbered, with_paused_writer,
};
use scratch::TempDir;

const SEED: &str = "seed";
const EXPIRED: &str = "expired";
const KEEPER: &str = "keeper";
const VICTIM: &str = "victim";
const HOUR_MS: u64 = 3_600_000;

fn started(name: &str, records: Vec<Record>) -> TempDir {
    let dir = TempDir::private(name);
    plant(&dir, "journal.jsonl", &frames(&records));
    dir
}

fn three(name: &str) -> TempDir {
    started(
        name,
        vec![numbered(SEED, 1), expired(EXPIRED, 2), numbered(KEEPER, 3)],
    )
}

fn victim() -> Record {
    numbered(VICTIM, 50)
}

fn read(dir: &Path) -> ReadReport {
    journal(dir).read().unwrap()
}

fn prune(dir: &Path, now_ms: u64) -> PruneReport {
    journal(dir).prune(&keep_everything(), now_ms, BOOT).unwrap()
}

fn each_once(report: &ReadReport, sessions: &[&str]) {
    for session in sessions {
        assert_eq!(count_of(&report.records, session), 1, "{session}");
    }
}

#[test]
fn variant_one_the_writer_resumes_after_a_rotation_and_retention_then_prunes_it_down_to_one_copy() {
    let dir = three("ri-one");
    let (written, during) = with_paused_writer(&dir, &victim(), WritePoint::BeforeWrite, 1, || {
        journal(&dir).rotate(T).unwrap();
        read(&dir)
    });
    each_once(&during, &[SEED, KEEPER]);
    assert_eq!(count_of(&during.records, VICTIM), 0);
    assert_eq!(attempts(&written), 2);

    let after = read(&dir);
    assert_eq!(count_of(&after.records, VICTIM), 1);
    assert_eq!(after.duplicates_removed, 1);
    assert_eq!(copies_on_disk(&dir, VICTIM), 2);

    let report = journal(&dir).retain(&keep_everything(), T + 1, BOOT).unwrap();
    assert!(report.rewritten + report.deleted >= 1, "{report:?}");
    let retained = read(&dir);
    each_once(&retained, &[SEED, KEEPER, VICTIM]);
    assert_eq!(count_of(&retained.records, EXPIRED), 0);

    prune(&dir, T + 2);
    let last = read(&dir);
    each_once(&last, &[SEED, KEEPER, VICTIM]);
    assert_eq!(last.duplicates_removed, 0);
    assert_eq!(copies_on_disk(&dir, VICTIM), 1);
}

#[test]
fn variant_two_a_pause_that_outlasts_a_retention_replacing_the_generation_loses_nothing() {
    let dir = three("ri-two-replaced");
    let (written, ()) = with_paused_writer(&dir, &victim(), WritePoint::BeforeWrite, 1, || {
        let report = prune(&dir, T);
        assert_eq!(report.retained.rewritten, 1);
        let later = prune(&dir, T + HOUR_MS);
        assert_eq!(later.retained.rewritten + later.retained.deleted, 0);
    });
    assert_eq!(attempts(&written), 2);
    assert_eq!(copies_on_disk(&dir, VICTIM), 1);
    let after = read(&dir);
    each_once(&after, &[SEED, KEEPER, VICTIM]);
    assert_eq!(count_of(&after.records, EXPIRED), 0);
    assert_eq!(after.duplicates_removed, 0);
}

#[test]
fn variant_two_a_pause_that_outlasts_a_retention_deleting_the_generation_loses_nothing() {
    let dir = started(
        "ri-two-deleted",
        vec![expired(EXPIRED, 1), expired("expired-too", 2)],
    );
    let (written, ()) = with_paused_writer(&dir, &victim(), WritePoint::BeforeWrite, 1, || {
        let report = prune(&dir, T + HOUR_MS);
        assert_eq!(report.retained.deleted, 1);
    });
    assert!(!dir.join(generation_name(T + HOUR_MS)).exists());
    assert_eq!(attempts(&written), 2);
    assert_eq!(copies_on_disk(&dir, VICTIM), 1);
    assert_eq!(count_of(&read(&dir).records, VICTIM), 1);
}

#[test]
fn a_pause_between_the_write_and_the_recheck_across_a_replacement_keeps_one_copy_after_the_next_run() {
    let dir = three("ri-after-write");
    let (written, ()) = with_paused_writer(&dir, &victim(), WritePoint::AfterWrite, 1, || {
        let report = prune(&dir, T);
        assert_eq!(report.retained.rewritten, 1);
    });
    assert_eq!(attempts(&written), 2);
    assert_eq!(copies_on_disk(&dir, VICTIM), 2);
    let after = read(&dir);
    assert_eq!(count_of(&after.records, VICTIM), 1);
    assert_eq!(after.duplicates_removed, 1);

    prune(&dir, T + 1);
    assert_eq!(copies_on_disk(&dir, VICTIM), 1);
    assert_eq!(count_of(&read(&dir).records, VICTIM), 1);
}

#[test]
fn a_write_that_lands_in_a_generation_after_it_was_scanned_and_before_it_is_replaced_is_written_again() {
    let dir = three("ri-planned");
    let (mut writer, pause) = paused(WritePoint::BeforeWrite, 1);
    let gate = MaintenanceGate::at(MaintenancePoint::Planned);
    let record = victim();

    let (written, pruned) = thread::scope(|scope| {
        let appending = scope.spawn(|| journal(&dir).append_with(&record, &mut writer));
        pause.wait_reached();
        let pruning = scope.spawn(|| journal(&dir).with_probe(&gate).prune(&keep_everything(), T, BOOT));
        gate.wait_reached();

        pause.release();
        let written = appending.join().unwrap();

        gate.release();
        (written, pruning.join().unwrap().unwrap())
    });

    assert_eq!(attempts(&written), 2);
    assert_eq!(pruned.retained.rewritten, 1);
    assert_eq!(copies_on_disk(&dir, VICTIM), 1);
    let after = read(&dir);
    each_once(&after, &[SEED, KEEPER, VICTIM]);
    assert_eq!(count_of(&after.records, EXPIRED), 0);
    assert_eq!(after.duplicates_removed, 0);
}

#[test]
fn a_retention_holding_the_lock_makes_a_second_run_busy_while_appends_and_reads_go_on() {
    let dir = three("ri-locked");
    let gate = MaintenanceGate::at(MaintenancePoint::Locked);

    let pruned = thread::scope(|scope| {
        let first = scope.spawn(|| journal(&dir).with_probe(&gate).prune(&keep_everything(), T, BOOT));
        gate.wait_reached();

        let retained = journal(&dir).retain(&keep_everything(), T, BOOT);
        let pruned = journal(&dir).prune(&keep_everything(), T, BOOT);
        assert!(matches!(retained, Err(MaintenanceError::Busy)), "{retained:?}");
        assert!(matches!(pruned, Err(MaintenanceError::Busy)), "{pruned:?}");
        journal(&dir).append(&victim()).unwrap();
        each_once(&read(&dir), &[SEED, KEEPER, VICTIM]);

        gate.release();
        first.join().unwrap().unwrap()
    });

    assert_eq!(pruned.retained.rewritten, 1);
    each_once(&read(&dir), &[SEED, KEEPER, VICTIM]);
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
fn a_reader_paused_after_it_opened_the_active_file_sees_every_survivor_once_when_a_run_replaces_the_generation()
 {
    let dir = three("ri-reader-open");
    let (report, ()) = read_paused_at(&dir, ReadPoint::ActiveOpened, || {
        let pruned = prune(&dir, T);
        assert_eq!(pruned.retained.rewritten, 1);
    });
    each_once(&report, &[SEED, KEEPER]);
    assert!(count_of(&report.records, EXPIRED) <= 1);
    assert!(report.duplicates_removed >= 2);
}

#[test]
fn a_reader_paused_after_it_opened_the_active_file_still_holds_what_a_run_deleted() {
    let dir = started(
        "ri-reader-open-delete",
        vec![expired(EXPIRED, 1), expired("expired-too", 2)],
    );
    let (report, ()) = read_paused_at(&dir, ReadPoint::ActiveOpened, || {
        let pruned = prune(&dir, T);
        assert_eq!(pruned.retained.deleted, 1);
    });
    each_once(&report, &[EXPIRED, "expired-too"]);
    assert_eq!(report.duplicates_removed, 0);
}

#[test]
fn a_reader_paused_after_the_listing_reads_the_replacement_and_misses_no_survivor() {
    let dir = three("ri-reader-list");
    journal(&dir).rotate(T).unwrap();
    let (report, ()) = read_paused_at(&dir, ReadPoint::GenerationsListed, || {
        let retained = journal(&dir).retain(&keep_everything(), T + 1, BOOT).unwrap();
        assert_eq!(retained.rewritten, 1);
    });
    each_once(&report, &[SEED, KEEPER]);
    assert_eq!(count_of(&report.records, EXPIRED), 0);
    assert_eq!(report.duplicates_removed, 0);
}

#[test]
fn a_reader_paused_after_the_listing_skips_a_generation_a_run_deleted() {
    let dir = three("ri-reader-list-delete");
    journal(&dir).rotate(T).unwrap();
    plant(
        &dir,
        &generation_name(T - 10),
        &frames(&[expired("only-expired", 7)]),
    );
    let (report, ()) = read_paused_at(&dir, ReadPoint::GenerationsListed, || {
        let retained = journal(&dir).retain(&keep_everything(), T + 1, BOOT).unwrap();
        assert_eq!((retained.deleted, retained.rewritten), (1, 1));
    });
    each_once(&report, &[SEED, KEEPER]);
    assert_eq!(report.unsafe_files, 0);
    assert_eq!(report.skipped_lines(), 0);
}

#[derive(Debug, Clone, Copy)]
enum Step {
    Rotate,
    Retain,
    Prune,
}

fn run_steps(dir: &Path, steps: &[Step]) {
    for (n, step) in steps.iter().enumerate() {
        let now = T + n as u64 * HOUR_MS;
        match step {
            Step::Rotate => {
                journal(dir).rotate(now).unwrap();
            }
            Step::Retain => {
                journal(dir).retain(&keep_everything(), now, BOOT).unwrap();
            }
            Step::Prune => {
                prune(dir, now);
            }
        }
    }
}

#[test]
fn whatever_the_writer_sleeps_through_its_record_is_present_exactly_once_after_the_next_run() {
    use Step::{Prune, Retain, Rotate};
    let scripts: [&[Step]; 7] = [
        &[],
        &[Rotate],
        &[Prune],
        &[Rotate, Retain],
        &[Prune, Prune],
        &[Rotate, Retain, Rotate, Retain],
        &[Prune, Retain, Prune, Rotate, Prune],
    ];
    for point in [WritePoint::BeforeWrite, WritePoint::AfterWrite] {
        for (n, script) in scripts.iter().enumerate() {
            let label = format!("{point:?} script {n}");
            let dir = three(&format!("ri-matrix-{n}"));
            let (written, ()) = with_paused_writer(&dir, &victim(), point, 1, || run_steps(&dir, script));
            let written = written.unwrap_or_else(|err| panic!("{label}: {err}"));
            assert!(written.attempts >= 1, "{label}");

            let before = read(&dir);
            each_once(&before, &[SEED, KEEPER, VICTIM]);

            prune(&dir, T + 100 * HOUR_MS);
            let settled = read(&dir);
            each_once(&settled, &[SEED, KEEPER, VICTIM]);
            assert_eq!(count_of(&settled.records, EXPIRED), 0, "{label}");
            assert_eq!(settled.duplicates_removed, 0, "{label}");
            assert_eq!(copies_on_disk(&dir, VICTIM), 1, "{label}");
        }
    }
}
