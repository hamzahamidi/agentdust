mod common;

use std::fs;
use std::time::Instant;

use common::{pre_tool_use, run_hook, scratch_dir};

#[test]
#[ignore = "measures the real binary: cargo test --release -p agentdust --test hook_latency -- --ignored"]
fn hook_latency_is_within_budget() {
    let dir = scratch_dir("latency");
    let input = pre_tool_use("latency", "toolu_latency");
    for _ in 0..20 {
        assert!(run_hook(&dir, &input).status.success());
    }
    let mut samples: Vec<f64> = (0..200)
        .map(|_| {
            let start = Instant::now();
            assert!(run_hook(&dir, &input).status.success());
            start.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    samples.sort_by(f64::total_cmp);
    let p50 = samples[samples.len() / 2];
    let p95 = samples[samples.len() * 95 / 100];
    println!("hook latency p50 {p50:.2} ms, p95 {p95:.2} ms");
    fs::remove_dir_all(&dir).unwrap();
    assert!(p50 < 10.0, "p50 {p50:.2} ms");
    assert!(p95 < 20.0, "p95 {p95:.2} ms");
}
