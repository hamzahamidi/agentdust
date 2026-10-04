use std::fs::File;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::Instant;

use agentdust_core::clock;
use serde::{Deserialize, Serialize};

use crate::frame::{Framing, encode};
use crate::payload::record_with;
use crate::stats::LatencySummary;
use crate::store::{create_private_dir, open_active};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    None,
    Fsync,
    Full,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::None, Mode::Fsync, Mode::Full];

    pub fn label(self) -> &'static str {
        match self {
            Mode::None => "none",
            Mode::Fsync => "fsync",
            Mode::Full => "full",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.label() == label)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurabilityRow {
    pub mode: String,
    pub samples: u64,
    pub size: usize,
    pub p50: u64,
    pub p95: u64,
    pub p99: u64,
    pub max: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DurabilityReport {
    pub load_before: String,
    pub load_after: String,
    pub filesystem: String,
    pub elapsed_secs: f64,
    pub rows: Vec<DurabilityRow>,
}

pub fn measure(dir: &Path, mode: Mode, samples: u64, size: usize) -> io::Result<DurabilityRow> {
    if samples == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a measurement needs at least one sample",
        ));
    }
    create_private_dir(dir)?;
    let mut file = open_active(dir)?;
    let mut latencies = Vec::with_capacity(samples as usize);
    for seq in 0..samples {
        let record = record_with(0, seq, size, clock::wall_ms(), clock::monotonic_ns());
        let frame = encode(&record, Framing::Record).map_err(io::Error::other)?;
        let started = Instant::now();
        file.write_all(&frame)?;
        match mode {
            Mode::None => {}
            Mode::Fsync => fsync(&file)?,
            Mode::Full => file.sync_all()?,
        }
        latencies.push((started.elapsed().as_nanos() as u64).max(1));
    }
    let summary =
        LatencySummary::from_samples(latencies).ok_or_else(|| io::Error::other("no sample was taken"))?;
    Ok(DurabilityRow {
        mode: mode.label().to_owned(),
        samples,
        size,
        p50: summary.p50,
        p95: summary.p95,
        p99: summary.p99,
        max: summary.max,
    })
}

fn fsync(file: &File) -> io::Result<()> {
    // SAFETY: the descriptor stays open for as long as `file` is borrowed.
    if unsafe { libc::fsync(file.as_raw_fd()) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub fn render_durability(report: &DurabilityReport) -> String {
    let mut out = format!(
        "- Load averages before the run: {}\n- Load averages after the run: {}\n- Data directory file system: {}\n- Run time: {} s.\n\n",
        report.load_before,
        report.load_after,
        report.filesystem,
        plain(report.elapsed_secs)
    );
    out.push_str("| Mode | Samples | Record bytes | p50 ms | p95 ms | p99 ms | max ms |\n");
    out.push_str("| --- | ---: | ---: | ---: | ---: | ---: | ---: |\n");
    for row in &report.rows {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            row.mode,
            row.samples,
            row.size,
            ms(row.p50),
            ms(row.p95),
            ms(row.p99),
            ms(row.max)
        ));
    }
    out
}

fn ms(ns: u64) -> String {
    format!("{:.3}", ns as f64 / 1_000_000.0)
}

fn plain(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}
