mod common;

use std::fs;
use std::time::Instant;

use common::{pre_tool_use_with, run_hook, scratch_dir};

const WARM_UP: usize = 20;
const RUNS: usize = 200;
const FRESH_RUNS: usize = 30;

fn percentiles(mut samples: Vec<f64>) -> (f64, f64) {
    samples.sort_by(f64::total_cmp);
    (samples[samples.len() / 2], samples[samples.len() * 95 / 100])
}

fn milliseconds(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn measure(input: &[u8]) -> (f64, f64) {
    let dir = scratch_dir("latency");
    for _ in 0..WARM_UP {
        assert!(run_hook(&dir, input).status.success());
    }
    let samples = (0..RUNS)
        .map(|_| {
            let start = Instant::now();
            assert!(run_hook(&dir, input).status.success());
            milliseconds(start)
        })
        .collect();
    fs::remove_dir_all(&dir).unwrap();
    percentiles(samples)
}

#[test]
#[ignore = "measures the real binary: cargo test --release -p agentdust --test hook_latency -- --ignored --nocapture --test-threads=1"]
fn hook_latency_is_within_budget() {
    let cwd = format!(r#","cwd":"{}""#, env!("CARGO_MANIFEST_DIR"));
    let (p50, p95) = measure(&pre_tool_use_with("latency", "toolu_latency", &cwd));
    println!("hook latency with a cwd p50 {p50:.2} ms, p95 {p95:.2} ms");
    let (bare_p50, bare_p95) = measure(&pre_tool_use_with("latency", "toolu_latency", ""));
    println!("hook latency without a cwd p50 {bare_p50:.2} ms, p95 {bare_p95:.2} ms");
    assert!(p50 < 10.0, "p50 {p50:.2} ms");
    assert!(p95 < 20.0, "p95 {p95:.2} ms");
}

#[test]
#[ignore = "measures the real binary: cargo test --release -p agentdust --test hook_latency -- --ignored --nocapture --test-threads=1"]
fn the_first_hook_in_an_empty_data_directory_is_within_budget() {
    let cwd = format!(r#","cwd":"{}""#, env!("CARGO_MANIFEST_DIR"));
    let input = pre_tool_use_with("latency", "toolu_latency", &cwd);
    let warm = scratch_dir("latency-warm");
    for _ in 0..WARM_UP {
        assert!(run_hook(&warm, &input).status.success());
    }
    fs::remove_dir_all(&warm).unwrap();
    let samples = (0..FRESH_RUNS)
        .map(|_| {
            let dir = scratch_dir("latency-fresh");
            let start = Instant::now();
            assert!(run_hook(&dir, &input).status.success());
            let took = milliseconds(start);
            fs::remove_dir_all(&dir).unwrap();
            took
        })
        .collect();
    let (p50, p95) = percentiles(samples);
    println!("first hook in an empty data directory p50 {p50:.2} ms, p95 {p95:.2} ms");
    assert!(p50 < 50.0, "p50 {p50:.2} ms");
}
