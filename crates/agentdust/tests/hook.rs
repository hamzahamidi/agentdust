mod common;

use std::fs;
use std::thread;

use agentdust_core::journal::{self, Agent, Kind};
use common::{pre_tool_use, run_hook, scratch_dir};

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
        r#"{{"session_id":"s2","hook_event_name":"PostToolUse","tool_use_id":"toolu_2","tool_response":{{"stdout":"{output_text}"}}}}"#
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
fn concurrent_hooks_append_complete_records() {
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
    assert_eq!(report.records.len(), 16);
    fs::remove_dir_all(&dir).unwrap();
}
