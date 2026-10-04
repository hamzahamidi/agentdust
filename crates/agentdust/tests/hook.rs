mod common;

use std::ffi::OsStr;
use std::fs;
use std::thread;
use std::time::{Duration, Instant};

use agentdust_core::journal::{self, Agent, Kind};
use common::{pre_tool_use, run_hook, run_hook_with, scratch_dir};

fn tool_event(event: &str, tool: &str) -> Vec<u8> {
    format!(
        r#"{{"session_id":"s1","hook_event_name":"{event}","tool_name":"{tool}","tool_use_id":"toolu_1","tool_input":{{"file_path":"/etc/hosts"}}}}"#
    )
    .into_bytes()
}

#[test]
fn pre_tool_use_appends_one_shell_start_record() {
    let dir = scratch_dir("pre");
    let output = run_hook(&dir, &pre_tool_use("s1", "toolu_1"));
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    let report = journal::read(&dir).unwrap();
    assert_eq!(report.records.len(), 1);
    let record = &report.records[0];
    assert_eq!(record.kind, Kind::ShellStart);
    assert_eq!(record.agent, Agent::Claude);
    assert_eq!(record.session_id, "s1");
    assert_eq!(record.tool_use_id.as_deref(), Some("toolu_1"));
    assert_eq!(record.boot.len(), 36);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn events_outside_the_journal_append_nothing() {
    let dir = scratch_dir("stop");
    let output = run_hook(&dir, br#"{"session_id":"s1","hook_event_name":"Stop"}"#);
    assert!(output.status.success());
    assert_eq!(journal::read(&dir).unwrap().records.len(), 0);
}

#[test]
fn tool_events_for_other_tools_append_nothing() {
    let dir = scratch_dir("read-tool");
    for event in ["PreToolUse", "PostToolUse"] {
        let output = run_hook(&dir, &tool_event(event, "Read"));
        assert!(output.status.success(), "{event}");
        assert!(output.stdout.is_empty(), "{event}");
        assert!(output.stderr.is_empty(), "{event}");
    }
    assert_eq!(journal::read(&dir).unwrap().records.len(), 0);
}

#[test]
fn tool_events_without_a_tool_name_append_nothing() {
    let dir = scratch_dir("no-tool-name");
    for event in ["PreToolUse", "PostToolUse"] {
        let input = format!(r#"{{"session_id":"s1","hook_event_name":"{event}","tool_use_id":"toolu_1"}}"#);
        assert!(run_hook(&dir, input.as_bytes()).status.success(), "{event}");
    }
    assert_eq!(journal::read(&dir).unwrap().records.len(), 0);
}

#[test]
fn session_events_are_recorded_without_a_tool_name() {
    let dir = scratch_dir("session-events");
    for event in ["SessionStart", "SessionEnd"] {
        let input = format!(r#"{{"session_id":"s1","hook_event_name":"{event}"}}"#);
        assert!(run_hook(&dir, input.as_bytes()).status.success(), "{event}");
    }
    let kinds: Vec<Kind> = journal::read(&dir)
        .unwrap()
        .records
        .iter()
        .map(|r| r.kind)
        .collect();
    assert_eq!(kinds, [Kind::SessionStart, Kind::SessionEnd]);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn malformed_input_exits_zero_and_appends_nothing() {
    let dir = scratch_dir("malformed");
    let output = run_hook(&dir, b"not json at all");
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(journal::read(&dir).unwrap().records.len(), 0);
}

#[test]
fn a_large_tool_response_is_read_but_never_stored() {
    let dir = scratch_dir("large");
    let output_text = "secret-output-".repeat(700_000);
    let input = format!(
        r#"{{"session_id":"s2","hook_event_name":"PostToolUse","tool_name":"Bash","tool_use_id":"toolu_2","tool_response":{{"stdout":"{output_text}"}}}}"#
    );
    assert!(input.len() > 9_000_000);
    let output = run_hook(&dir, input.as_bytes());
    assert!(output.status.success());
    let report = journal::read(&dir).unwrap();
    assert_eq!(report.records.len(), 1);
    assert_eq!(report.records[0].kind, Kind::ShellEnd);
    let stored = fs::read_to_string(dir.join("journal.jsonl")).unwrap();
    assert!(!stored.contains("secret-output"));
    assert!(!stored.contains("npm run dev"));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn empty_stdin_exits_zero_and_appends_nothing() {
    let dir = scratch_dir("empty");
    let output = run_hook(&dir, b"");
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(journal::read(&dir).unwrap().records.len(), 0);
}

#[test]
fn an_unwritable_data_dir_exits_zero_silently() {
    let dir = scratch_dir("unwritable");
    fs::write(&dir, b"a file, not a directory").unwrap();
    let output = run_hook(&dir, &pre_tool_use("s1", "toolu_1"));
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    fs::remove_file(&dir).unwrap();
}

#[test]
fn an_oversized_session_id_is_not_recorded() {
    let dir = scratch_dir("oversized");
    let long = "s".repeat(10_000);
    let output = run_hook(&dir, &pre_tool_use(&long, "toolu_1"));
    assert!(output.status.success());
    assert_eq!(journal::read(&dir).unwrap().records.len(), 0);
}

#[test]
fn an_oversized_session_id_still_lets_the_host_finish_writing() {
    let dir = scratch_dir("oversized-large");
    let long = "s".repeat(1_000_000);
    let trailing = "x".repeat(1_000_000);
    let input = format!(
        r#"{{"session_id":"{long}","hook_event_name":"PreToolUse","tool_name":"Bash","tool_response":"{trailing}"}}"#
    );
    let output = run_hook(&dir, input.as_bytes());
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(journal::read(&dir).unwrap().records.len(), 0);
}

#[test]
fn concurrent_hooks_never_interleave_records() {
    let dir = scratch_dir("concurrent");
    let handles: Vec<_> = (0..16)
        .map(|i| {
            let dir = dir.clone();
            thread::spawn(move || run_hook(&dir, &pre_tool_use(&format!("s{i}"), &format!("toolu_{i}"))))
        })
        .collect();
    for handle in handles {
        assert!(handle.join().unwrap().status.success());
    }
    let report = journal::read(&dir).unwrap();
    assert_eq!(report.skipped_lines, 0);
    let mut ids: Vec<&str> = report.records.iter().map(|r| r.session_id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), report.records.len());
    assert!(!ids.is_empty());
    assert!(ids.iter().all(|id| (0..16).any(|i| *id == format!("s{i}"))));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_busy_journal_lock_drops_the_record_without_noise() {
    let dir = scratch_dir("busy-lock");
    fs::create_dir_all(&dir).unwrap();
    let lock = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(dir.join("journal.lock"))
        .unwrap();
    lock.lock().unwrap();
    let started = Instant::now();
    let output = run_hook(&dir, &pre_tool_use("s1", "toolu_1"));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert_eq!(journal::read(&dir).unwrap().records.len(), 0);
    drop(lock);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_unknown_agent_hook_exits_zero_silently() {
    let dir = scratch_dir("unknown-agent");
    for args in [&["hook", "codex"][..], &["hook"][..]] {
        let output = run_hook_with(
            args,
            &[("AGENTDUST_DATA_DIR", dir.as_os_str())],
            None,
            &pre_tool_use("s1", "toolu_1"),
        );
        assert!(output.status.success(), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(output.stderr.is_empty(), "{args:?}");
    }
    assert_eq!(journal::read(&dir).unwrap().records.len(), 0);
}

#[test]
fn an_empty_data_dir_override_writes_under_home_not_the_working_directory() {
    let root = scratch_dir("empty-override");
    let (home, cwd) = (root.join("home"), root.join("repo"));
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&cwd).unwrap();
    let output = run_hook_with(
        &["hook", "claude"],
        &[("AGENTDUST_DATA_DIR", OsStr::new("")), ("HOME", home.as_os_str())],
        Some(&cwd),
        &pre_tool_use("s1", "toolu_1"),
    );
    assert!(output.status.success());
    assert!(fs::read_dir(&cwd).unwrap().next().is_none());
    let data_dir = home.join("Library/Application Support/agentdust");
    assert_eq!(journal::read(&data_dir).unwrap().records.len(), 1);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_relative_data_dir_override_records_nothing() {
    let root = scratch_dir("relative-override");
    let (home, cwd) = (root.join("home"), root.join("repo"));
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&cwd).unwrap();
    let output = run_hook_with(
        &["hook", "claude"],
        &[
            ("AGENTDUST_DATA_DIR", OsStr::new("rel")),
            ("HOME", home.as_os_str()),
        ],
        Some(&cwd),
        &pre_tool_use("s1", "toolu_1"),
    );
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert!(fs::read_dir(&cwd).unwrap().next().is_none());
    assert!(fs::read_dir(&home).unwrap().next().is_none());
    fs::remove_dir_all(&root).unwrap();
}
