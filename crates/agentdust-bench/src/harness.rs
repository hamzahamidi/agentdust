use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use agentdust_core::clock;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::candidates::Candidate;
use crate::integrity::{Integrity, check};
use crate::payload::record_with;
use crate::stats::LatencySummary;
use crate::worker::{ReaderOutcome, RotatorOutcome, WriterOutcome};

pub const READER_INTERVAL_MS: u64 = 20;
pub const ROTATOR_BUDGET_MS: u64 = 50;
pub const ROTATOR_FINAL_BUDGET_MS: u64 = 10_000;
const COST_SEGMENTS: u64 = 4;

#[derive(Debug, Clone)]
pub struct Cell {
    pub candidate: Candidate,
    pub writers: u32,
    pub records_per_writer: u64,
    pub size: usize,
    pub reader: bool,
    pub rotator: bool,
    pub pace_us: u64,
    pub rotate_period_ms: u64,
    pub grace_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellOutcome {
    pub attempted: u64,
    pub acked: u64,
    pub dropped: u64,
    pub stale: u64,
    pub errors: u64,
    pub retried: u64,
    pub latency: LatencySummary,
    pub integrity: Integrity,
    pub files: u64,
    pub wall_ns: u64,
    pub reader: Option<ReaderOutcome>,
    pub rotator: Option<RotatorOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderCost {
    pub candidate: String,
    pub size: usize,
    pub records: u64,
    pub records_read: u64,
    pub files: u64,
    pub bytes: u64,
    pub read_ns: Vec<u64>,
}

struct Proc {
    child: Child,
    stdout: BufReader<ChildStdout>,
}

impl Proc {
    fn spawn(exe: &Path, args: &[OsString]) -> io::Result<Self> {
        let mut child = Command::new(exe)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let stdout = BufReader::new(child.stdout.take().ok_or_else(|| io::Error::other("no stdout"))?);
        Ok(Self { child, stdout })
    }

    fn wait_ready(&mut self) -> io::Result<()> {
        let mut line = String::new();
        self.stdout.read_line(&mut line)?;
        if line.trim() == "ready" {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "a child said {line:?} instead of ready"
            )))
        }
    }

    fn go(&mut self) -> io::Result<()> {
        let mut stdin = self
            .child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("no stdin"))?;
        stdin.write_all(b"go\n")
    }

    fn finish<T: DeserializeOwned>(&mut self) -> io::Result<T> {
        let mut text = String::new();
        self.stdout.read_to_string(&mut text)?;
        let status = self.child.wait()?;
        if !status.success() {
            return Err(io::Error::other(format!("a child exited with {status}")));
        }
        serde_json::from_str(text.trim()).map_err(io::Error::other)
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn unique_dir(root: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    root.join(format!(
        "run-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

fn command(name: &str, flags: &[(&str, OsString)]) -> Vec<OsString> {
    let mut all = vec![OsString::from(name)];
    for (flag, value) in flags {
        all.push(OsString::from(flag));
        all.push(value.clone());
    }
    all
}

fn text(value: impl ToString) -> OsString {
    OsString::from(value.to_string())
}

pub fn run_cell(exe: &Path, root: &Path, cell: &Cell) -> io::Result<CellOutcome> {
    let base = unique_dir(root);
    let data = base.join("data");
    let stop = base.join("stop");
    fs::create_dir_all(&base)?;
    let result = run_cell_in(exe, &data, &stop, cell);
    let _ = fs::remove_dir_all(&base);
    result
}

fn run_cell_in(exe: &Path, data: &Path, stop: &Path, cell: &Cell) -> io::Result<CellOutcome> {
    let label = cell.candidate.label();
    let mut writers = Vec::new();
    for writer in 0..cell.writers {
        writers.push(Proc::spawn(
            exe,
            &command(
                "writer",
                &[
                    ("--candidate", text(label)),
                    ("--dir", data.into()),
                    ("--writer", text(writer)),
                    ("--records", text(cell.records_per_writer)),
                    ("--size", text(cell.size)),
                    ("--pace-us", text(cell.pace_us)),
                ],
            ),
        )?);
    }
    let mut reader = if cell.reader {
        Some(Proc::spawn(
            exe,
            &command(
                "reader",
                &[
                    ("--candidate", text(label)),
                    ("--dir", data.into()),
                    ("--size", text(cell.size)),
                    ("--stop-file", stop.into()),
                    ("--interval-ms", text(READER_INTERVAL_MS)),
                ],
            ),
        )?)
    } else {
        None
    };
    let mut rotator = if cell.rotator {
        Some(Proc::spawn(
            exe,
            &command(
                "rotator",
                &[
                    ("--candidate", text(label)),
                    ("--dir", data.into()),
                    ("--stop-file", stop.into()),
                    ("--period-ms", text(cell.rotate_period_ms)),
                    ("--grace-ms", text(cell.grace_ms)),
                    ("--budget-ms", text(ROTATOR_BUDGET_MS)),
                    ("--final-budget-ms", text(ROTATOR_FINAL_BUDGET_MS)),
                ],
            ),
        )?)
    } else {
        None
    };
    for process in writers
        .iter_mut()
        .chain(reader.iter_mut())
        .chain(rotator.iter_mut())
    {
        process.wait_ready()?;
    }
    let started = Instant::now();
    for process in writers
        .iter_mut()
        .chain(reader.iter_mut())
        .chain(rotator.iter_mut())
    {
        process.go()?;
    }
    let outcomes = writers
        .iter_mut()
        .map(Proc::finish::<WriterOutcome>)
        .collect::<io::Result<Vec<_>>>()?;
    let wall_ns = started.elapsed().as_nanos() as u64;
    fs::write(stop, b"")?;
    let rotator = match rotator.as_mut() {
        Some(process) => Some(process.finish::<RotatorOutcome>()?),
        None => None,
    };
    let reader = match reader.as_mut() {
        Some(process) => Some(process.finish::<ReaderOutcome>()?),
        None => None,
    };
    summarise(cell, data, outcomes, wall_ns, reader, rotator)
}

fn summarise(
    cell: &Cell,
    data: &Path,
    outcomes: Vec<WriterOutcome>,
    wall_ns: u64,
    reader: Option<ReaderOutcome>,
    rotator: Option<RotatorOutcome>,
) -> io::Result<CellOutcome> {
    let journal = cell.candidate.open(data);
    let report = journal.read_all()?;
    let files = journal.file_count()? as u64;
    let mut acked = BTreeSet::new();
    let mut samples = Vec::new();
    let (mut dropped, mut stale, mut errors, mut retried) = (0, 0, 0, 0);
    for outcome in outcomes {
        let failed: BTreeSet<u64> = outcome.failed.iter().copied().collect();
        acked.extend(
            (0..cell.records_per_writer)
                .filter(|seq| !failed.contains(seq))
                .map(|seq| (outcome.writer, seq)),
        );
        dropped += outcome.busy;
        stale += outcome.stale;
        errors += outcome.other_errors;
        retried += outcome.retried;
        samples.extend(outcome.latencies_ns);
    }
    Ok(CellOutcome {
        attempted: u64::from(cell.writers) * cell.records_per_writer,
        acked: acked.len() as u64,
        dropped,
        stale,
        errors,
        retried,
        latency: LatencySummary::from_samples(samples).ok_or_else(|| io::Error::other("no appends ran"))?,
        integrity: check(&report, &acked, cell.size),
        files,
        wall_ns,
        reader,
        rotator,
    })
}

pub fn measure_reader_cost(
    candidate: Candidate,
    root: &Path,
    records: u64,
    size: usize,
    repeats: u32,
) -> io::Result<ReaderCost> {
    let base = unique_dir(root);
    let data = base.join("data");
    let result = measure_in(candidate, &data, records, size, repeats);
    let _ = fs::remove_dir_all(&base);
    result
}

fn measure_in(
    candidate: Candidate,
    data: &Path,
    records: u64,
    size: usize,
    repeats: u32,
) -> io::Result<ReaderCost> {
    let journal = candidate.open(data);
    let per_segment = records.div_ceil(COST_SEGMENTS).max(1);
    for seq in 0..records {
        let record = record_with(0, seq, size, clock::wall_ms(), clock::monotonic_ns());
        journal.append(&record).map_err(io::Error::other)?;
        if (seq + 1) % per_segment == 0 && seq + 1 < records {
            journal.rotate(clock::wall_ms()).map_err(io::Error::other)?;
        }
    }
    let mut read_ns = Vec::new();
    let mut records_read = 0;
    for _ in 0..repeats {
        let started = Instant::now();
        let report = journal.read_all()?;
        read_ns.push((started.elapsed().as_nanos() as u64).max(1));
        records_read = report.records.len() as u64;
    }
    Ok(ReaderCost {
        candidate: candidate.label().to_owned(),
        size,
        records,
        records_read,
        files: journal.file_count()? as u64,
        bytes: bytes_under(data)?,
        read_ns,
    })
}

fn bytes_under(dir: &Path) -> io::Result<u64> {
    let mut total = 0;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            total += bytes_under(&entry.path())?;
        } else {
            total += entry.metadata()?.len();
        }
    }
    Ok(total)
}
