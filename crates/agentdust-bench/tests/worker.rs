mod common;

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

use agentdust_bench::Candidate;
use agentdust_bench::integrity::{Integrity, check};
use agentdust_bench::payload::is_marker;
use agentdust_bench::worker::{
    ReaderPlan, RotatorPlan, WriterPlan, run_reader, run_reader_with, run_rotator, run_writer,
};
use common::{Scratch, data_files};

fn plan(candidate: Candidate, scratch: &Scratch, records: u64) -> WriterPlan {
    WriterPlan {
        candidate,
        dir: scratch.path().to_path_buf(),
        writer: 3,
        records,
        size: 150,
        pace_us: 0,
    }
}

fn reader_plan(candidate: Candidate, scratch: &Scratch, stop_file: &Path) -> ReaderPlan {
    ReaderPlan {
        candidate,
        dir: scratch.path().to_path_buf(),
        size: 150,
        interval_ms: 5,
        stop_file: stop_file.to_path_buf(),
    }
}

fn rotator_plan(candidate: Candidate, scratch: &Scratch, stop_file: &Path) -> RotatorPlan {
    RotatorPlan {
        candidate,
        dir: scratch.path().to_path_buf(),
        period_ms: 1,
        grace_ms: 0,
        budget_ms: 0,
        final_budget_ms: 0,
        stop_file: stop_file.to_path_buf(),
    }
}

fn acked(writer: u32, count: u64) -> BTreeSet<(u32, u64)> {
    (0..count).map(|seq| (writer, seq)).collect()
}

fn hold_exclusive(scratch: &Scratch) -> File {
    scratch.create();
    let holder = OpenOptions::new()
        .create(true)
        .append(true)
        .open(scratch.path().join("journal.lock"))
        .unwrap();
    holder.lock().unwrap();
    holder
}

#[test]
fn a_writer_numbers_its_records_from_zero_and_times_every_append() {
    for candidate in Candidate::ALL {
        let scratch = Scratch::new("writer");
        let outcome = run_writer(&plan(candidate, &scratch, 20));
        let label = candidate.label();
        assert_eq!(outcome.writer, 3);
        assert_eq!(outcome.latencies_ns.len(), 20, "{label}");
        assert!(outcome.latencies_ns.iter().all(|ns| *ns > 0), "{label}");
        assert!(outcome.failed.is_empty(), "{label}");
        assert_eq!(
            (outcome.busy, outcome.stale, outcome.other_errors, outcome.retried),
            (0, 0, 0, 0),
            "{label}"
        );
        let report = candidate.open(scratch.path()).read_all().unwrap();
        assert_eq!(
            check(&report, &acked(3, 20), 150),
            Integrity::default(),
            "{label}"
        );
    }
}

#[test]
fn a_dropped_record_is_listed_and_never_retried_by_a_and_d() {
    for candidate in [Candidate::Flock, Candidate::Shared] {
        let scratch = Scratch::new("dropped");
        let holder = hold_exclusive(&scratch);
        let outcome = run_writer(&plan(candidate, &scratch, 3));
        assert_eq!(outcome.failed, [0, 1, 2], "{}", candidate.label());
        assert_eq!(
            (outcome.busy, outcome.stale, outcome.other_errors),
            (3, 0, 0),
            "{}",
            candidate.label()
        );
        drop(holder);
        assert!(
            candidate
                .open(scratch.path())
                .read_all()
                .unwrap()
                .records
                .is_empty()
        );
    }
}

#[test]
fn a_record_that_the_writer_could_not_append_for_another_reason_is_an_error_and_not_a_drop() {
    let scratch = Scratch::new("other-error");
    scratch.create();
    let target = scratch.path().join("elsewhere");
    std::fs::write(&target, b"").unwrap();
    std::os::unix::fs::symlink(&target, scratch.path().join("journal.jsonl")).unwrap();
    let outcome = run_writer(&plan(Candidate::Append, &scratch, 2));
    assert_eq!(outcome.failed, [0, 1]);
    assert_eq!((outcome.busy, outcome.stale, outcome.other_errors), (0, 0, 2));
}

#[test]
fn a_reader_reads_once_when_the_stop_file_already_exists() {
    for candidate in Candidate::ALL {
        let scratch = Scratch::new("reader-once");
        run_writer(&plan(candidate, &scratch, 5));
        let stop = scratch.path().join("stop");
        File::create(&stop).unwrap();
        let outcome = run_reader(&reader_plan(candidate, &scratch, &stop));
        assert_eq!(outcome.reads, 1, "{}", candidate.label());
        assert_eq!(outcome.max_records, 5, "{}", candidate.label());
        assert_eq!(outcome.max_skipped, 0, "{}", candidate.label());
        assert!(outcome.median_ns > 0 && outcome.max_ns >= outcome.median_ns);
    }
}

#[test]
fn a_reader_counts_the_torn_tail_it_sees() {
    let scratch = Scratch::new("reader-torn");
    run_writer(&plan(Candidate::Append, &scratch, 3));
    OpenOptions::new()
        .append(true)
        .open(scratch.path().join("journal.jsonl"))
        .unwrap()
        .write_all(b"\x1e{\"v\":1,\"kind\":")
        .unwrap();
    let stop = scratch.path().join("stop");
    File::create(&stop).unwrap();
    let outcome = run_reader(&reader_plan(Candidate::Append, &scratch, &stop));
    assert_eq!((outcome.max_records, outcome.max_skipped), (3, 1));
}

#[test]
fn a_reader_keeps_reading_until_the_stop_file_appears() {
    let scratch = Scratch::new("reader-loop");
    run_writer(&plan(Candidate::Append, &scratch, 3));
    let stop = scratch.path().join("stop");
    let marker = stop.clone();
    let outcome = run_reader_with(&reader_plan(Candidate::Append, &scratch, &stop), |reads| {
        if reads == 3 {
            File::create(&marker).unwrap();
        }
    });
    assert_eq!(outcome.reads, 3);
}

#[test]
fn a_rotator_with_the_stop_file_in_place_runs_exactly_one_final_cycle_per_candidate() {
    for candidate in Candidate::ALL {
        let scratch = Scratch::new("rotator-final");
        run_writer(&plan(candidate, &scratch, 12));
        let stop = scratch.path().join("stop");
        File::create(&stop).unwrap();
        let outcome = run_rotator(&rotator_plan(candidate, &scratch, &stop));
        let label = candidate.label();
        assert_eq!(outcome.cycles, 1, "{label}");
        assert_eq!(
            (outcome.rotations, outcome.rotate_busy, outcome.errors),
            (1, 0, 0),
            "{label}"
        );
        assert_eq!((outcome.retains, outcome.retain_busy), (1, 0), "{label}");
        assert!(outcome.final_cycle_ok, "{label}");
        assert_eq!(
            outcome.rewritten, 1,
            "{label}: the marker is the one record that goes"
        );
        let report = candidate.open(scratch.path()).read_all().unwrap();
        assert_eq!(
            check(&report, &acked(3, 12), 150),
            Integrity::default(),
            "{label}"
        );
        assert!(report.records.iter().all(|record| !is_marker(record)), "{label}");
        assert_eq!(
            data_files(scratch.path()).len(),
            1,
            "{label}: one compacted generation and no active file"
        );
    }
}

#[test]
fn a_rotator_that_cannot_get_a_lock_counts_it_and_reports_the_final_cycle_as_failed() {
    let scratch = Scratch::new("rotator-busy");
    run_writer(&plan(Candidate::Flock, &scratch, 3));
    let holder = hold_exclusive(&scratch);
    let stop = scratch.path().join("stop");
    File::create(&stop).unwrap();
    let outcome = run_rotator(&rotator_plan(Candidate::Flock, &scratch, &stop));
    assert_eq!(
        (outcome.rotate_busy, outcome.retain_busy, outcome.rotations),
        (1, 1, 0)
    );
    assert!(!outcome.final_cycle_ok);
    drop(holder);
}

#[test]
fn a_rotator_loop_cycles_until_the_stop_file_appears_and_then_runs_the_final_cycle() {
    let scratch = Scratch::new("rotator-loop");
    run_writer(&plan(Candidate::Recheck, &scratch, 6));
    let stop = scratch.path().join("stop");
    let mut loop_plan = rotator_plan(Candidate::Recheck, &scratch, &stop);
    loop_plan.budget_ms = 50;
    loop_plan.final_budget_ms = 5_000;
    let outcome = agentdust_bench::worker::run_rotator_with(&loop_plan, |cycle| {
        if cycle == 3 {
            File::create(&stop).unwrap();
        }
    });
    assert_eq!(outcome.cycles, 4);
    assert!(outcome.final_cycle_ok);
    assert!(outcome.rotations >= 1);
    let report = Candidate::Recheck.open(scratch.path()).read_all().unwrap();
    assert_eq!(check(&report, &acked(3, 6), 150), Integrity::default());
}
