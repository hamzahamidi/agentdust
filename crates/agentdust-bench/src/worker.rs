use std::path::PathBuf;
use std::time::{Duration, Instant};

use agentdust_core::clock;
use agentdust_core::journal::Record;
use serde::{Deserialize, Serialize};

use crate::candidates::{AppendError, Candidate, Options};
use crate::maintenance::{MaintenanceError, Retention, Rotation};
use crate::payload::{identify, is_marker, marker, record_with};
use crate::stats::percentile;

#[derive(Debug, Clone)]
pub struct WriterPlan {
    pub candidate: Candidate,
    pub dir: PathBuf,
    pub writer: u32,
    pub records: u64,
    pub size: usize,
    pub pace_us: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriterOutcome {
    pub writer: u32,
    pub latencies_ns: Vec<u64>,
    pub failed: Vec<u64>,
    pub busy: u64,
    pub stale: u64,
    pub other_errors: u64,
    pub retried: u64,
}

pub fn run_writer(plan: &WriterPlan) -> WriterOutcome {
    let mut outcome = WriterOutcome {
        writer: plan.writer,
        latencies_ns: Vec::with_capacity(plan.records as usize),
        failed: Vec::new(),
        busy: 0,
        stale: 0,
        other_errors: 0,
        retried: 0,
    };
    let journal = plan.candidate.open(&plan.dir);
    for seq in 0..plan.records {
        let record = record_with(
            plan.writer,
            seq,
            plan.size,
            clock::wall_ms(),
            clock::monotonic_ns(),
        );
        let started = Instant::now();
        let result = journal.append(&record);
        outcome
            .latencies_ns
            .push((started.elapsed().as_nanos() as u64).max(1));
        match result {
            Ok(appended) => outcome.retried += u64::from(appended.attempts > 1),
            Err(AppendError::Busy) => {
                outcome.busy += 1;
                outcome.failed.push(seq);
            }
            Err(AppendError::Stale { .. }) => {
                outcome.stale += 1;
                outcome.failed.push(seq);
            }
            Err(_) => {
                outcome.other_errors += 1;
                outcome.failed.push(seq);
            }
        }
        if plan.pace_us > 0 {
            std::thread::sleep(Duration::from_micros(plan.pace_us));
        }
    }
    outcome
}

#[derive(Debug, Clone)]
pub struct ReaderPlan {
    pub candidate: Candidate,
    pub dir: PathBuf,
    pub size: usize,
    pub interval_ms: u64,
    pub stop_file: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderOutcome {
    pub reads: u64,
    pub median_ns: u64,
    pub max_ns: u64,
    pub max_records: u64,
    pub max_skipped: u64,
}

pub fn run_reader(plan: &ReaderPlan) -> ReaderOutcome {
    run_reader_with(plan, |_| {})
}

pub fn run_reader_with(plan: &ReaderPlan, mut on_read: impl FnMut(u64)) -> ReaderOutcome {
    let journal = plan.candidate.open(&plan.dir);
    let parent = std::os::unix::process::parent_id();
    let mut read_ns = Vec::new();
    let (mut max_records, mut max_skipped) = (0, 0);
    loop {
        let started = Instant::now();
        if let Ok(outcome) = journal.read_all() {
            read_ns.push((started.elapsed().as_nanos() as u64).max(1));
            let unidentified = outcome
                .records
                .iter()
                .filter(|record| !is_marker(record) && identify(record, plan.size).is_none())
                .count();
            max_records = max_records.max(outcome.records.len() as u64);
            max_skipped = max_skipped.max((outcome.skipped_lines() + unidentified) as u64);
            on_read(read_ns.len() as u64);
        }
        if plan.stop_file.exists() || std::os::unix::process::parent_id() != parent {
            break;
        }
        std::thread::sleep(Duration::from_millis(plan.interval_ms));
    }
    read_ns.sort_unstable();
    ReaderOutcome {
        reads: read_ns.len() as u64,
        median_ns: percentile(&read_ns, 50.0).unwrap_or(0),
        max_ns: read_ns.last().copied().unwrap_or(0),
        max_records,
        max_skipped,
    }
}

#[derive(Debug, Clone)]
pub struct RotatorPlan {
    pub candidate: Candidate,
    pub dir: PathBuf,
    pub period_ms: u64,
    pub grace_ms: u64,
    pub budget_ms: u64,
    pub final_budget_ms: u64,
    pub stop_file: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RotatorOutcome {
    pub cycles: u64,
    pub rotations: u64,
    pub rotate_busy: u64,
    pub retains: u64,
    pub retain_busy: u64,
    pub errors: u64,
    pub rewritten: u64,
    pub deleted: u64,
    pub final_cycle_ok: bool,
    pub wait_median_ns: u64,
    pub wait_max_ns: u64,
}

pub fn run_rotator(plan: &RotatorPlan) -> RotatorOutcome {
    run_rotator_with(plan, |_| {})
}

pub fn run_rotator_with(plan: &RotatorPlan, mut on_cycle: impl FnMut(u64)) -> RotatorOutcome {
    let parent = std::os::unix::process::parent_id();
    let mut outcome = RotatorOutcome {
        cycles: 0,
        rotations: 0,
        rotate_busy: 0,
        retains: 0,
        retain_busy: 0,
        errors: 0,
        rewritten: 0,
        deleted: 0,
        final_cycle_ok: false,
        wait_median_ns: 0,
        wait_max_ns: 0,
    };
    let mut waits = Vec::new();
    loop {
        let stopping = plan.stop_file.exists() || std::os::unix::process::parent_id() != parent;
        outcome.cycles += 1;
        let budget = if stopping {
            plan.final_budget_ms
        } else {
            plan.budget_ms
        };
        let journal = plan.candidate.open_with(
            &plan.dir,
            Options {
                maintenance_budget: Duration::from_millis(budget),
            },
        );
        let _ = journal.append(&marker(outcome.cycles, clock::wall_ms(), clock::monotonic_ns()));
        let started = Instant::now();
        let rotation = journal.rotate(clock::wall_ms());
        waits.push((started.elapsed().as_nanos() as u64).max(1));
        let keep = |record: &Record| !is_marker(record);
        let retained = journal.retain(&Retention {
            now_ms: clock::wall_ms(),
            grace_ms: if stopping { 0 } else { plan.grace_ms },
            keep: &keep,
        });
        match &rotation {
            Ok(Rotation::Rotated { .. }) => outcome.rotations += 1,
            Ok(Rotation::Empty) => {}
            Err(MaintenanceError::Busy) => outcome.rotate_busy += 1,
            Err(MaintenanceError::Io(_)) => outcome.errors += 1,
        }
        match &retained {
            Ok(report) => {
                outcome.retains += 1;
                outcome.rewritten += report.rewritten as u64;
                outcome.deleted += report.deleted as u64;
            }
            Err(MaintenanceError::Busy) => outcome.retain_busy += 1,
            Err(MaintenanceError::Io(_)) => outcome.errors += 1,
        }
        on_cycle(outcome.cycles);
        if stopping {
            outcome.final_cycle_ok = rotation.is_ok() && retained.is_ok();
            break;
        }
        std::thread::sleep(Duration::from_millis(plan.period_ms));
    }
    waits.sort_unstable();
    outcome.wait_median_ns = percentile(&waits, 50.0).unwrap_or(0);
    outcome.wait_max_ns = waits.last().copied().unwrap_or(0);
    outcome
}
