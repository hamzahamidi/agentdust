mod common;

use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::thread;
use std::time::Duration;

use agentdust_bench::frame::decode;
use agentdust_bench::maintenance::{MaintenanceError, RetainReport, Retention, Rotation, generation_stamp};
use agentdust_bench::payload::{is_marker, marker};
use agentdust_bench::{AppendError, Appended, Candidate, Journal, Options, Point, ReadOutcome};
use agentdust_core::journal::Record;
use common::{Gate, OnPoint, Scratch, count_of, named};

const T: u64 = 1_800_000_000_000;
const GRACE: u64 = 10_000;
const VICTIM: &str = "victim";
const SEED: &str = "seed";

type Writer = Result<Appended, AppendError>;

struct Run {
    journal: Box<dyn Journal>,
    scratch: Scratch,
}

fn expired() -> Record {
    marker(0, T, 2)
}

fn victim() -> Record {
    named(VICTIM, 50)
}

fn unexpired(record: &Record) -> bool {
    !is_marker(record)
}

fn retention(now_ms: u64, grace_ms: u64) -> Retention<'static> {
    Retention {
        now_ms,
        grace_ms,
        keep: &unexpired,
    }
}

fn open(candidate: Candidate, scratch: &Scratch) -> Box<dyn Journal> {
    candidate.open_with(
        scratch.path(),
        Options {
            maintenance_budget: Duration::ZERO,
        },
    )
}

fn paused<R>(
    candidate: Candidate,
    point: Point,
    seed: &[Record],
    during: impl FnOnce(&dyn Journal) -> R,
) -> (Run, Writer, R) {
    let scratch = Scratch::new(candidate.label());
    let journal = open(candidate, &scratch);
    for record in seed {
        journal.append(record).unwrap();
    }
    let gate = Gate::at(point);
    let record = victim();
    let (written, observed) = thread::scope(|scope| {
        let writer = scope.spawn(|| journal.append_probed(&record, &gate));
        gate.wait_reached();
        let observed = during(&*journal);
        gate.release();
        (writer.join().unwrap(), observed)
    });
    (Run { journal, scratch }, written, observed)
}

fn on_disk(dir: &Path, session: &str) -> usize {
    fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name == "journal.jsonl" || generation_stamp(&name).is_some()
        })
        .map(|entry| {
            let decoded = decode(File::open(entry.path()).unwrap()).unwrap();
            count_of(&decoded.records, session)
        })
        .sum()
}

fn read(run: &Run) -> ReadOutcome {
    run.journal.read_all().unwrap()
}

fn attempts(written: &Writer) -> u32 {
    written.as_ref().expect("the append was acknowledged").attempts
}

fn rotated(rotation: &Result<Rotation, MaintenanceError>) -> bool {
    matches!(rotation, Ok(Rotation::Rotated { .. }))
}

fn busy<T>(result: &Result<T, MaintenanceError>) -> bool {
    matches!(result, Err(MaintenanceError::Busy))
}

fn lock_free(candidate: Candidate) -> bool {
    matches!(candidate, Candidate::Append | Candidate::Recheck)
}

#[test]
fn variant_one_the_writer_resumes_after_the_rotation_and_the_record_is_present_exactly_once() {
    for candidate in Candidate::ALL {
        let label = candidate.label();
        let (run, written, (rotation, during)) = paused(
            candidate,
            Point::AfterOpen,
            &[named(SEED, 1), expired()],
            |journal| {
                let rotation = journal.rotate(T);
                let during = (candidate != Candidate::Flock).then(|| journal.read_all().unwrap());
                (rotation, during)
            },
        );
        assert_eq!(rotated(&rotation), lock_free(candidate), "{label}: {rotation:?}");
        assert_eq!(busy(&rotation), !lock_free(candidate), "{label}");
        if let Some(during) = during {
            assert_eq!(count_of(&during.records, SEED), 1, "{label}");
            assert_eq!(count_of(&during.records, VICTIM), 0, "{label}");
        }
        let expected_attempts = if candidate == Candidate::Recheck { 2 } else { 1 };
        assert_eq!(attempts(&written), expected_attempts, "{label}");

        let after = read(&run);
        assert_eq!(count_of(&after.records, VICTIM), 1, "{label}");
        assert_eq!(
            after.duplicates_removed,
            usize::from(candidate == Candidate::Recheck),
            "{label}"
        );

        run.journal.rotate(T + 1).unwrap();
        let report = run.journal.retain(&retention(T + 1 + GRACE, GRACE)).unwrap();
        assert!(report.rewritten + report.deleted >= 1, "{label}: {report:?}");
        let last = read(&run);
        assert_eq!(count_of(&last.records, VICTIM), 1, "{label}");
        assert_eq!(count_of(&last.records, SEED), 1, "{label}");
        assert_eq!(last.records.iter().filter(|r| is_marker(r)).count(), 0, "{label}");
        assert_eq!(on_disk(run.scratch.path(), VICTIM), 1, "{label}");
    }
}

#[test]
fn variant_two_a_pause_past_the_grace_window_and_a_compaction_lose_the_record_only_under_c() {
    for point in [Point::AfterOpen, Point::BeforeWrite] {
        for candidate in Candidate::ALL {
            let label = candidate.label();
            let (run, written, (rotation, retained)) =
                paused(candidate, point, &[named(SEED, 1), expired()], |journal| {
                    (journal.rotate(T), journal.retain(&retention(T + GRACE, GRACE)))
                });
            assert_eq!(rotated(&rotation), lock_free(candidate), "{label} {point:?}");
            match candidate {
                Candidate::Flock => assert!(busy(&retained), "{label}"),
                Candidate::Shared => assert_eq!(retained.unwrap().eligible, 0, "{label}"),
                _ => {
                    let report = retained.unwrap();
                    assert_eq!(
                        (report.eligible, report.rewritten, report.deleted),
                        (1, 1, 0),
                        "{label}"
                    );
                }
            }
            let expected_attempts = if candidate == Candidate::Recheck { 2 } else { 1 };
            assert_eq!(attempts(&written), expected_attempts, "{label} {point:?}");

            let found = count_of(&read(&run).records, VICTIM);
            if candidate == Candidate::Append {
                assert_eq!(
                    found, 0,
                    "C loses an acknowledged record when the pause outlasts the grace window"
                );
            } else {
                assert_eq!(found, 1, "{label} {point:?}");
            }

            run.journal.rotate(T + GRACE + 1).unwrap();
            run.journal.retain(&retention(T + 2 * GRACE, GRACE)).unwrap();
            let last = count_of(&read(&run).records, VICTIM);
            assert_eq!(
                last,
                if candidate == Candidate::Append { 0 } else { 1 },
                "{label} {point:?}"
            );
            assert_eq!(on_disk(run.scratch.path(), VICTIM), last, "{label} {point:?}");
        }
    }
}

#[test]
fn variant_two_with_a_generation_that_retention_deletes_outright() {
    for candidate in Candidate::ALL {
        let label = candidate.label();
        let (run, written, (rotation, retained)) =
            paused(candidate, Point::AfterOpen, &[expired()], |journal| {
                (journal.rotate(T), journal.retain(&retention(T + GRACE, GRACE)))
            });
        assert_eq!(rotated(&rotation), lock_free(candidate), "{label}");
        if lock_free(candidate) {
            let report = retained.unwrap();
            assert_eq!((report.deleted, report.rewritten), (1, 0), "{label}");
        }
        assert!(written.is_ok(), "{label}");
        let found = count_of(&read(&run).records, VICTIM);
        assert_eq!(found, usize::from(candidate != Candidate::Append), "{label}");
    }
}

#[test]
fn a_writer_that_resumes_inside_the_grace_window_is_safe_even_under_c() {
    for candidate in Candidate::ALL {
        let label = candidate.label();
        let (run, written, retained) = paused(
            candidate,
            Point::AfterOpen,
            &[named(SEED, 1), expired()],
            |journal| {
                let _ = journal.rotate(T);
                journal.retain(&retention(T + GRACE - 1, GRACE))
            },
        );
        if lock_free(candidate) {
            let report: RetainReport = retained.unwrap();
            assert_eq!(
                (report.waiting, report.rewritten, report.deleted),
                (1, 0, 0),
                "{label}"
            );
        }
        assert!(written.is_ok(), "{label}");
        assert_eq!(count_of(&read(&run).records, VICTIM), 1, "{label}");
        run.journal.retain(&retention(T + GRACE, GRACE)).ok();
        run.journal.rotate(T + GRACE + 1).unwrap();
        run.journal.retain(&retention(T + 3 * GRACE, GRACE)).unwrap();
        assert_eq!(count_of(&read(&run).records, VICTIM), 1, "{label}");
        assert_eq!(on_disk(run.scratch.path(), VICTIM), 1, "{label}");
    }
}

#[test]
fn a_writer_paused_after_its_write_is_never_lost_and_a_recheck_writes_it_again() {
    for candidate in Candidate::ALL {
        let label = candidate.label();
        let (run, written, (rotation, _)) = paused(
            candidate,
            Point::AfterWrite,
            &[named(SEED, 1), expired()],
            |journal| (journal.rotate(T), journal.retain(&retention(T + GRACE, GRACE))),
        );
        assert_eq!(rotated(&rotation), lock_free(candidate), "{label}");
        let expected_attempts = if candidate == Candidate::Recheck { 2 } else { 1 };
        assert_eq!(attempts(&written), expected_attempts, "{label}");
        let after = read(&run);
        assert_eq!(count_of(&after.records, VICTIM), 1, "{label}");
        assert_eq!(
            after.duplicates_removed,
            usize::from(candidate == Candidate::Recheck),
            "{label}"
        );

        run.journal.rotate(T + GRACE + 1).unwrap();
        run.journal.retain(&retention(T + 3 * GRACE, GRACE)).unwrap();
        assert_eq!(count_of(&read(&run).records, VICTIM), 1, "{label}");
        assert_eq!(on_disk(run.scratch.path(), VICTIM), 1, "{label}");
    }
}

#[test]
fn a_recheck_gives_up_after_three_attempts_and_the_copies_collapse_on_read() {
    let scratch = Scratch::new("c2-three");
    let journal = open(Candidate::Recheck, &scratch);
    let probe = OnPoint::new(Point::AfterWrite, |hit| {
        journal.rotate(T + u64::from(hit)).unwrap();
    });
    let result = journal.append_probed(&victim(), &probe);
    assert!(
        matches!(result, Err(AppendError::Stale { attempts: 3 })),
        "{result:?}"
    );
    let report = journal.read_all().unwrap();
    assert_eq!(count_of(&report.records, VICTIM), 1);
    assert_eq!(report.duplicates_removed, 2);
    assert_eq!(on_disk(scratch.path(), VICTIM), 3);
}

#[test]
fn a_recheck_notices_that_the_file_it_wrote_to_was_unlinked() {
    let scratch = Scratch::new("c2-unlinked");
    let journal = open(Candidate::Recheck, &scratch);
    let active = scratch.path().join("journal.jsonl");
    let probe = OnPoint::new(Point::AfterWrite, |hit| {
        if hit == 1 {
            fs::remove_file(&active).unwrap();
        }
    });
    let written = journal.append_probed(&victim(), &probe).unwrap();
    assert_eq!(written.attempts, 2);
    assert_eq!(count_of(&journal.read_all().unwrap().records, VICTIM), 1);
    assert_eq!(on_disk(scratch.path(), VICTIM), 1);
}

#[test]
fn a_recheck_notices_that_another_file_now_has_the_name() {
    let scratch = Scratch::new("c2-replaced");
    let journal = open(Candidate::Recheck, &scratch);
    let active = scratch.path().join("journal.jsonl");
    let probe = OnPoint::new(Point::AfterWrite, |hit| {
        if hit == 1 {
            fs::remove_file(&active).unwrap();
            OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&active)
                .unwrap();
        }
    });
    let written = journal.append_probed(&victim(), &probe).unwrap();
    assert_eq!(written.attempts, 2);
    assert_eq!(on_disk(scratch.path(), VICTIM), 1);
}

#[test]
fn c_does_not_notice_and_reports_success_for_a_record_in_an_unlinked_file() {
    let scratch = Scratch::new("c-unlinked");
    let journal = open(Candidate::Append, &scratch);
    let active = scratch.path().join("journal.jsonl");
    let probe = OnPoint::new(Point::AfterWrite, |hit| {
        if hit == 1 {
            fs::remove_file(&active).unwrap();
        }
    });
    let written = journal.append_probed(&victim(), &probe).unwrap();
    assert_eq!(written.attempts, 1);
    assert_eq!(count_of(&journal.read_all().unwrap().records, VICTIM), 0);
}

#[test]
fn a_reader_that_started_before_a_rotation_and_a_compaction_returns_every_acknowledged_record_once() {
    for point in [Point::ReaderAfterActive, Point::ReaderAfterList] {
        for candidate in Candidate::ALL {
            let label = candidate.label();
            let scratch = Scratch::new(label);
            let journal = open(candidate, &scratch);
            let seed = [named("r1", 1), named("r2", 2), named("r3", 3), expired()];
            for record in &seed {
                journal.append(record).unwrap();
            }
            let gate = Gate::at(point);
            let (outcome, rotation, retained) = thread::scope(|scope| {
                let reader = scope.spawn(|| journal.read_probed(&gate));
                gate.wait_reached();
                let rotation = journal.rotate(T);
                let retained = journal.retain(&retention(T + GRACE, GRACE));
                gate.release();
                (reader.join().unwrap().unwrap(), rotation, retained)
            });
            assert_eq!(rotated(&rotation), lock_free(candidate), "{label} {point:?}");
            if lock_free(candidate) {
                assert_eq!(retained.unwrap().rewritten, 1, "{label} {point:?}");
            }
            for id in ["r1", "r2", "r3"] {
                assert_eq!(count_of(&outcome.records, id), 1, "{label} {point:?} {id}");
            }
        }
    }
}

#[test]
fn a_generation_that_retention_deletes_between_the_listing_and_the_open_is_skipped_without_error() {
    for candidate in Candidate::ALL {
        let label = candidate.label();
        let scratch = Scratch::new(label);
        common::plant(scratch.path(), "journal.5.jsonl", candidate, &[expired()]);
        common::plant(
            scratch.path(),
            "journal.jsonl",
            candidate,
            &[named("r1", 1), named("r2", 2)],
        );
        let journal = open(candidate, &scratch);
        let gate = Gate::at(Point::ReaderAfterList);
        let (outcome, retained) = thread::scope(|scope| {
            let reader = scope.spawn(|| journal.read_probed(&gate));
            gate.wait_reached();
            let retained = journal.retain(&retention(T, 0));
            gate.release();
            (reader.join().unwrap().unwrap(), retained)
        });
        if lock_free(candidate) {
            assert_eq!(retained.unwrap().deleted, 1, "{label}");
        } else {
            assert!(
                busy(&retained),
                "{label}: the reader's lock holds the generation set still"
            );
        }
        assert_eq!(count_of(&outcome.records, "r1"), 1, "{label}");
        assert_eq!(count_of(&outcome.records, "r2"), 1, "{label}");
        assert_eq!(outcome.skipped_lines(), 0, "{label}");
    }
}

#[test]
fn a_rotator_that_cannot_get_the_lock_leaves_the_active_file_where_it_is() {
    for candidate in [Candidate::Flock, Candidate::Shared] {
        let label = candidate.label();
        let (run, _, rotation) = paused(candidate, Point::AfterOpen, &[named(SEED, 1)], |journal| {
            journal.rotate(T)
        });
        assert!(busy(&rotation), "{label}");
        assert!(run.scratch.path().join("journal.jsonl").exists(), "{label}");
        assert!(
            matches!(run.journal.rotate(T), Ok(Rotation::Rotated { stamp: T })),
            "{label}"
        );
    }
}
