mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use agentdust_bench::Candidate;
use agentdust_bench::harness::{Cell, CellOutcome, measure_reader_cost, run_cell};
use agentdust_bench::worker::{RotatorOutcome, WriterOutcome};
use common::{EXE, Scratch};

fn cell(candidate: Candidate, writers: u32, records_per_writer: u64, size: usize) -> Cell {
    Cell {
        candidate,
        writers,
        records_per_writer,
        size,
        reader: true,
        rotator: true,
        pace_us: 0,
        rotate_period_ms: 2,
        grace_ms: 0,
    }
}

fn assert_consistent(candidate: Candidate, outcome: &CellOutcome, attempted: u64) {
    let label = candidate.label();
    assert_eq!(outcome.attempted, attempted, "{label}");
    assert_eq!(
        outcome.acked + outcome.dropped + outcome.stale + outcome.errors,
        attempted,
        "{label}"
    );
    assert_eq!(outcome.errors, 0, "{label}");
    assert_eq!(outcome.latency.count, attempted, "{label}");
    assert!(outcome.wall_ns > 0, "{label}");
    let integrity = &outcome.integrity;
    assert_eq!(
        (
            integrity.torn,
            integrity.interleaved,
            integrity.markers,
            integrity.out_of_order
        ),
        (0, 0, 0, 0),
        "{label}: {integrity:?}"
    );
    assert!(integrity.unacked <= outcome.stale, "{label}: {integrity:?}");
    if candidate != Candidate::Append {
        assert_eq!(integrity.lost, 0, "{label}: {integrity:?}");
    }
    if matches!(candidate, Candidate::Append | Candidate::Recheck) {
        assert_eq!(outcome.dropped, 0, "{label}");
    }
    if candidate != Candidate::Recheck {
        assert_eq!((outcome.retried, outcome.stale), (0, 0), "{label}");
    }
    let rotator = outcome.rotator.as_ref().expect("the cell ran a rotator");
    assert!(rotator.final_cycle_ok, "{label}: {rotator:?}");
    assert!(rotator.rotations >= 1, "{label}: {rotator:?}");
    assert_eq!(rotator.errors, 0, "{label}: {rotator:?}");
    let reader = outcome.reader.as_ref().expect("the cell ran a reader");
    assert!(reader.reads >= 1, "{label}");
}

#[test]
fn three_writer_processes_with_a_rotator_and_a_reader_lose_and_tear_nothing() {
    for candidate in Candidate::ALL {
        let scratch = Scratch::new("three");
        let outcome = run_cell(Path::new(EXE), scratch.path(), &cell(candidate, 3, 40, 150)).unwrap();
        assert_consistent(candidate, &outcome, 120);
        assert!(outcome.files >= 1, "{}", candidate.label());
    }
}

#[test]
fn sixteen_writer_processes_with_4000_byte_records_tear_nothing() {
    for candidate in Candidate::ALL {
        let scratch = Scratch::new("sixteen");
        let outcome = run_cell(Path::new(EXE), scratch.path(), &cell(candidate, 16, 12, 4000)).unwrap();
        assert_consistent(candidate, &outcome, 192);
    }
}

#[test]
fn a_cell_without_a_reader_or_a_rotator_reports_none_for_each() {
    let scratch = Scratch::new("quiet");
    let mut quiet = cell(Candidate::Append, 3, 10, 150);
    quiet.reader = false;
    quiet.rotator = false;
    let outcome = run_cell(Path::new(EXE), scratch.path(), &quiet).unwrap();
    assert!(outcome.reader.is_none() && outcome.rotator.is_none());
    assert_eq!((outcome.integrity.lost, outcome.integrity.torn), (0, 0));
    assert_eq!(outcome.files, 1);
}

#[test]
fn a_run_leaves_nothing_behind_in_its_root() {
    let scratch = Scratch::new("cleanup");
    run_cell(
        Path::new(EXE),
        scratch.path(),
        &cell(Candidate::Shared, 3, 10, 150),
    )
    .unwrap();
    assert_eq!(
        common::files_under(scratch.path()),
        Vec::<std::path::PathBuf>::new()
    );
}

#[test]
fn the_reader_cost_is_measured_on_a_prepared_store_of_three_generations_and_an_active_file() {
    for candidate in Candidate::ALL {
        let scratch = Scratch::new("reader-cost");
        let cost = measure_reader_cost(candidate, scratch.path(), 300, 150, 3).unwrap();
        let label = candidate.label();
        assert_eq!(cost.candidate, label);
        assert_eq!((cost.records, cost.records_read), (300, 300), "{label}");
        assert_eq!(cost.read_ns.len(), 3, "{label}");
        assert!(cost.read_ns.iter().all(|ns| *ns > 0), "{label}");
        assert!(cost.bytes >= 300 * 140, "{label}");
        let files = match candidate {
            Candidate::Shared => 6,
            _ => 5,
        };
        assert_eq!(cost.files, files, "{label}");
    }
}

fn spawn_child(args: &[&str]) -> std::process::Child {
    Command::new(EXE)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap()
}

#[test]
fn a_writer_process_waits_for_go_before_it_touches_the_journal() {
    let scratch = Scratch::new("protocol");
    let dir = scratch.path().to_str().unwrap();
    let mut child = spawn_child(&[
        "writer",
        "--candidate",
        "C",
        "--dir",
        dir,
        "--writer",
        "1",
        "--records",
        "10",
        "--size",
        "150",
    ]);
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "ready");
    assert!(!scratch.path().join("journal.jsonl").exists());
    child.stdin.take().unwrap().write_all(b"go\n").unwrap();
    let mut rest = String::new();
    stdout.read_to_string(&mut rest).unwrap();
    assert!(child.wait().unwrap().success());
    let outcome: WriterOutcome = serde_json::from_str(rest.trim()).unwrap();
    assert_eq!(outcome.writer, 1);
    assert_eq!(outcome.latencies_ns.len(), 10);
    assert!(scratch.path().join("journal.jsonl").exists());
}

#[test]
fn a_writer_process_whose_stdin_closes_without_go_writes_nothing() {
    let scratch = Scratch::new("orphaned-writer");
    let dir = scratch.path().to_str().unwrap();
    let mut child = spawn_child(&[
        "writer",
        "--candidate",
        "C2",
        "--dir",
        dir,
        "--writer",
        "1",
        "--records",
        "10",
        "--size",
        "150",
    ]);
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    drop(child.stdin.take());
    assert!(child.wait().unwrap().code().is_some());
    assert!(!scratch.path().join("journal.jsonl").exists());
}

#[test]
fn a_rotator_process_waits_for_go_and_runs_its_final_cycle_when_the_stop_file_exists() {
    let scratch = Scratch::new("rotator-protocol");
    scratch.create();
    let dir = scratch.path().join("data");
    let stop = scratch.path().join("stop");
    std::fs::write(&stop, b"").unwrap();
    let mut child = spawn_child(&[
        "rotator",
        "--candidate",
        "D",
        "--dir",
        dir.to_str().unwrap(),
        "--stop-file",
        stop.to_str().unwrap(),
        "--period-ms",
        "5",
        "--grace-ms",
        "0",
        "--budget-ms",
        "50",
        "--final-budget-ms",
        "5000",
    ]);
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "ready");
    assert!(!dir.exists());
    child.stdin.take().unwrap().write_all(b"go\n").unwrap();
    let mut rest = String::new();
    stdout.read_to_string(&mut rest).unwrap();
    assert!(child.wait().unwrap().success());
    let outcome: RotatorOutcome = serde_json::from_str(rest.trim()).unwrap();
    assert_eq!(outcome.cycles, 1);
    assert!(outcome.final_cycle_ok);
}
