#![cfg(target_os = "macos")]

mod journal_support;
mod scratch;

use std::time::Instant;

use agentdust_core::journal::volume::locate;
use agentdust_core::journal::{Journal, SystemVolume, VolumeProbe};
use journal_support::{journal, named};
use scratch::TempDir;

const WARMUP: usize = 200;
const SAMPLES: usize = 2000;

fn measure(mut work: impl FnMut()) -> (f64, f64) {
    for _ in 0..WARMUP {
        work();
    }
    let mut samples: Vec<f64> = (0..SAMPLES)
        .map(|_| {
            let start = Instant::now();
            work();
            start.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    samples.sort_by(f64::total_cmp);
    (samples[SAMPLES / 2], samples[SAMPLES * 95 / 100])
}

#[test]
#[ignore = "measures only: cargo test --release -p agentdust-core --test journal_cost -- --ignored --nocapture"]
fn what_one_append_and_its_statfs_cost() {
    let dir = TempDir::private("cost");
    let record = named("cost");
    let rows = [
        (
            "statfs of the directory",
            measure(|| drop(SystemVolume.probe(&dir).unwrap())),
        ),
        (
            "locate (lstat and statfs)",
            measure(|| drop(locate(&SystemVolume, &dir).unwrap())),
        ),
        (
            "append with the system probe",
            measure(|| {
                let _ = Journal::new(&dir).append(&record).unwrap();
            }),
        ),
        (
            "append with a fixed probe",
            measure(|| {
                let _ = journal(&dir).append(&record).unwrap();
            }),
        ),
    ];
    for (label, (p50, p95)) in rows {
        println!("{label}: p50 {p50:.4} ms, p95 {p95:.4} ms");
    }
}
