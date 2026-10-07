mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use common::{pre_tool_use_with, private_dir, run_hook, run_hook_with, scratch_dir};
use serde_json::Value;

const WARM_UP: usize = 20;
const RUNS: usize = 200;
const FRESH_RUNS: usize = 30;
const CAPTURED_WORKLOAD_RUNS: usize = 3;

fn percentiles(mut samples: Vec<f64>) -> (f64, f64) {
    samples.sort_by(f64::total_cmp);
    (samples[samples.len() / 2], samples[samples.len() * 95 / 100])
}

fn milliseconds(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn captured_m6_fixtures() -> Vec<(PathBuf, Vec<Vec<u8>>)> {
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/m6");
    let mut paths: Vec<_> = fs::read_dir(fixture_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("claude-2.1.292-") && name.ends_with(".jsonl"))
        })
        .collect();
    paths.sort();

    paths
        .into_iter()
        .map(|path| {
            let fixture_name = path.file_stem().unwrap().to_str().unwrap();
            let contents = fs::read_to_string(&path).unwrap();
            let events = contents
                .lines()
                .map(|line| {
                    let mut event: Value = serde_json::from_str(line).unwrap();
                    let object = event.as_object_mut().unwrap();
                    for key in ["session_id", "agent_id", "tool_use_id"] {
                        if let Some(Value::String(value)) = object.get_mut(key) {
                            *value = format!("{value}-{fixture_name}");
                        }
                    }
                    object.insert(
                        "cwd".to_owned(),
                        Value::String(env!("CARGO_MANIFEST_DIR").to_owned()),
                    );
                    serde_json::to_vec(&event).unwrap()
                })
                .collect();
            (path, events)
        })
        .collect()
}

fn captured_session_streams(events: &[Vec<u8>]) -> Vec<Vec<Vec<u8>>> {
    let mut sessions = Vec::<(String, Vec<Vec<u8>>)>::new();
    for event in events {
        let value: Value = serde_json::from_slice(event).unwrap();
        let session = value["session_id"].as_str().unwrap().to_owned();
        if let Some((_, stream)) = sessions.iter_mut().find(|(id, _)| id == &session) {
            stream.push(event.clone());
        } else {
            sessions.push((session, vec![event.clone()]));
        }
    }
    sessions.into_iter().map(|(_, events)| events).collect()
}

fn captured_subagent_streams(events: &[Vec<u8>]) -> (Vec<Vec<u8>>, Vec<Vec<Vec<u8>>>) {
    let mut parent = Vec::new();
    let mut agents = Vec::<(String, Vec<Vec<u8>>)>::new();
    for event in events {
        let value: Value = serde_json::from_slice(event).unwrap();
        if let Some(agent) = value.get("agent_id").and_then(Value::as_str) {
            if let Some((_, stream)) = agents.iter_mut().find(|(id, _)| id == agent) {
                stream.push(event.clone());
            } else {
                agents.push((agent.to_owned(), vec![event.clone()]));
            }
        } else {
            parent.push(event.clone());
        }
    }
    (parent, agents.into_iter().map(|(_, events)| events).collect())
}

fn measure_concurrent_streams(dir: &Path, streams: &[Vec<Vec<u8>>]) -> Vec<f64> {
    std::thread::scope(|scope| {
        let workers = streams
            .iter()
            .map(|stream| {
                scope.spawn(move || {
                    stream
                        .iter()
                        .map(|event| {
                            let start = Instant::now();
                            let output = run_hook(dir, event);
                            assert!(output.status.success());
                            milliseconds(start)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap())
            .collect()
    })
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

#[test]
#[ignore = "measures the real binary: cargo test --release -p agentdust --test hook_latency -- --ignored --nocapture --test-threads=1"]
fn session_start_latency_is_within_budget() {
    let cwd = env!("CARGO_MANIFEST_DIR");
    let input = format!(
        r#"{{"session_id":"latency","hook_event_name":"SessionStart","source":"startup","cwd":"{cwd}"}}"#
    );
    let work = private_dir("latency-env");
    let env_file = work.join("env.sh");
    let dir = scratch_dir("latency-session-start");
    let run = || {
        let envs = [
            ("AGENTDUST_DATA_DIR", dir.as_os_str()),
            ("CLAUDE_ENV_FILE", env_file.as_os_str()),
        ];
        assert!(
            run_hook_with(["hook", "claude"], &envs, None, input.as_bytes())
                .status
                .success()
        );
    };
    for _ in 0..WARM_UP {
        run();
    }
    let samples = (0..RUNS)
        .map(|_| {
            let start = Instant::now();
            run();
            milliseconds(start)
        })
        .collect();
    let (p50, p95) = percentiles(samples);
    println!("session start latency with a tag and an env file p50 {p50:.2} ms, p95 {p95:.2} ms");
    fs::remove_dir_all(&dir).unwrap();
    fs::remove_dir_all(&work).unwrap();
    assert!(p50 < 10.0, "p50 {p50:.2} ms");
    assert!(p95 < 20.0, "p95 {p95:.2} ms");
}

#[test]
#[ignore = "replays sanitized Claude Code 2.1.292 captures: cargo test --release -p agentdust --test hook_latency -- --ignored --nocapture --test-threads=1"]
fn captured_m6_scenario_hooks_are_within_budget() {
    let fixtures = captured_m6_fixtures();
    assert!(!fixtures.is_empty(), "no Claude Code 2.1.292 fixtures found");

    let expected_scenarios = [
        ("three concurrent sessions", "three-sessions"),
        ("two background subagents", "two-background-subagents"),
        ("foreground subagent", "foreground-subagent"),
        ("resume", "resume"),
        ("/clear", "clear"),
        ("abrupt exit", "abrupt-exit"),
        ("Agent Team with two teammates", "agent-team"),
    ];
    let missing: Vec<_> = expected_scenarios
        .iter()
        .filter(|(_, filename_part)| {
            !fixtures.iter().any(|(path, _)| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains(filename_part))
            })
        })
        .map(|(scenario, _)| *scenario)
        .collect();

    let warm = scratch_dir("latency-m6-warm");
    for (_, events) in &fixtures {
        for event in events {
            assert!(run_hook(&warm, event).status.success());
        }
    }
    fs::remove_dir_all(&warm).unwrap();

    let mut samples = Vec::new();
    for run in 0..CAPTURED_WORKLOAD_RUNS {
        for (_, events) in &fixtures {
            let dir = scratch_dir("latency-m6-captured");
            for event in events {
                let start = Instant::now();
                let output = run_hook(&dir, event);
                samples.push(milliseconds(start));
                assert!(output.status.success());
            }
            fs::remove_dir_all(&dir).unwrap();
        }
        println!(
            "captured scenario replay {}/{} complete",
            run + 1,
            CAPTURED_WORKLOAD_RUNS
        );
    }

    let sample_count = samples.len();
    let (p50, p95) = percentiles(samples);
    let fixture_names = fixtures
        .iter()
        .filter_map(|(path, _)| path.file_name().and_then(|name| name.to_str()))
        .collect::<Vec<_>>()
        .join(", ");
    println!(
        "captured Claude Code 2.1.292 workload: {sample_count} hook events across {} fixtures; p50 {p50:.2} ms, p95 {p95:.2} ms; fixtures: {fixture_names}",
        fixtures.len()
    );
    if !missing.is_empty() {
        println!(
            "M6 scenarios without Claude Code 2.1.292 fixtures: {}",
            missing.join("; ")
        );
    }
    assert!(p50 < 10.0, "p50 {p50:.2} ms");
    assert!(p95 < 20.0, "p95 {p95:.2} ms");

    let three_session_fixture = fixtures
        .iter()
        .find(|(path, _)| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains("three-sessions"))
        })
        .unwrap();
    let three_session_streams = captured_session_streams(&three_session_fixture.1);
    assert_eq!(three_session_streams.len(), 3);
    let mut three_session_samples = Vec::new();
    for run in 0..CAPTURED_WORKLOAD_RUNS {
        let dir = scratch_dir("latency-m6-three-sessions");
        three_session_samples.extend(measure_concurrent_streams(&dir, &three_session_streams));
        fs::remove_dir_all(&dir).unwrap();
        println!(
            "three-session concurrent replay {}/{} complete",
            run + 1,
            CAPTURED_WORKLOAD_RUNS
        );
    }
    let (three_session_p50, three_session_p95) = percentiles(three_session_samples);
    println!(
        "captured Claude Code 2.1.292 three-session replay: 3 ordered streams; p50 {three_session_p50:.2} ms, p95 {three_session_p95:.2} ms"
    );
    assert!(three_session_p50 < 10.0, "p50 {three_session_p50:.2} ms");
    assert!(three_session_p95 < 20.0, "p95 {three_session_p95:.2} ms");

    let two_subagent_fixture = fixtures
        .iter()
        .find(|(path, _)| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains("two-background-subagents"))
        })
        .unwrap();
    let (parent_events, subagent_streams) = captured_subagent_streams(&two_subagent_fixture.1);
    assert_eq!(parent_events.len(), 2);
    assert_eq!(subagent_streams.len(), 2);
    let mut subagent_samples = Vec::new();
    for _ in 0..CAPTURED_WORKLOAD_RUNS {
        let dir = scratch_dir("latency-m6-two-subagents");
        let parent_start = Instant::now();
        assert!(run_hook(&dir, &parent_events[0]).status.success());
        subagent_samples.push(milliseconds(parent_start));
        subagent_samples.extend(measure_concurrent_streams(&dir, &subagent_streams));
        let parent_end = Instant::now();
        assert!(run_hook(&dir, &parent_events[1]).status.success());
        subagent_samples.push(milliseconds(parent_end));
        fs::remove_dir_all(&dir).unwrap();
    }
    let (subagent_p50, subagent_p95) = percentiles(subagent_samples);
    println!(
        "captured Claude Code 2.1.292 two-background-subagent replay: 2 ordered agent streams; p50 {subagent_p50:.2} ms, p95 {subagent_p95:.2} ms"
    );
    assert!(subagent_p50 < 10.0, "p50 {subagent_p50:.2} ms");
    assert!(subagent_p95 < 20.0, "p95 {subagent_p95:.2} ms");
}
