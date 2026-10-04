mod common;
mod setup_support;

use std::fs;
use std::os::unix::fs::PermissionsExt;

use common::run_hook;
use setup_support::{BIN, Sandbox, code, text};

fn status(sandbox: &Sandbox) -> String {
    let output = sandbox.run(&["status"]);
    assert_eq!(code(&output), 0, "{}", text(&output));
    assert!(output.stderr.is_empty(), "{}", text(&output));
    String::from_utf8(output.stdout).unwrap()
}

fn line<'a>(report: &'a str, prefix: &str) -> &'a str {
    report
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("no line starting with {prefix:?} in:\n{report}"))
}

#[test]
fn status_on_a_fresh_machine_reports_each_part_and_creates_nothing() {
    let sandbox = Sandbox::new("status-fresh").with_stub();
    let report = status(&sandbox);
    assert!(
        report.starts_with(&format!("agentdust {}\n", env!("CARGO_PKG_VERSION"))),
        "{report}"
    );
    assert!(
        line(&report, "data directory:").contains(sandbox.data.to_str().unwrap()),
        "{report}"
    );
    assert!(
        line(&report, "data directory state:").contains("missing"),
        "{report}"
    );
    assert!(line(&report, "filesystem:").contains("apfs"), "{report}");
    assert!(line(&report, "filesystem:").contains("supported"), "{report}");
    assert_eq!(line(&report, "journal records:"), "journal records: 0");
    assert_eq!(line(&report, "setup:"), "setup: not installed");
    assert_eq!(line(&report, "apply:"), "apply: enabled");
    assert!(!sandbox.data.exists());
    assert!(sandbox.calls().is_empty());
}

#[test]
fn status_counts_journal_records_and_skipped_lines() {
    let sandbox = Sandbox::new("status-journal").with_stub();
    fs::create_dir(&sandbox.data).unwrap();
    fs::set_permissions(&sandbox.data, fs::Permissions::from_mode(0o700)).unwrap();
    for id in ["a", "b", "c"] {
        let input = format!(r#"{{"session_id":"{id}","hook_event_name":"SessionStart"}}"#);
        assert!(run_hook(&sandbox.data, input.as_bytes()).status.success());
    }
    let report = status(&sandbox);
    assert_eq!(line(&report, "journal records:"), "journal records: 3");
    assert_eq!(
        line(&report, "journal skipped lines:"),
        "journal skipped lines: 0"
    );
    let journal = sandbox.data.join("journal.jsonl");
    let mut bytes = fs::read(&journal).unwrap();
    bytes.extend_from_slice(b"this is not a journal record\n");
    fs::write(&journal, bytes).unwrap();
    let damaged = status(&sandbox);
    assert_eq!(line(&damaged, "journal records:"), "journal records: 3");
    assert!(
        line(&damaged, "journal skipped lines:").starts_with("journal skipped lines: 1"),
        "{damaged}"
    );
}

#[test]
fn status_says_when_the_journal_cannot_be_read() {
    let sandbox = Sandbox::new("status-journal-bad").with_stub();
    fs::create_dir(&sandbox.data).unwrap();
    fs::set_permissions(&sandbox.data, fs::Permissions::from_mode(0o755)).unwrap();
    let report = status(&sandbox);
    assert!(
        line(&report, "data directory state:").contains("refused"),
        "{report}"
    );
    assert!(line(&report, "journal:").contains("unavailable"), "{report}");
}

#[test]
fn status_follows_setup_and_removal() {
    let sandbox = Sandbox::new("status-setup").with_stub();
    assert_eq!(code(&sandbox.run(&["setup", "--yes"])), 0);
    let report = status(&sandbox);
    assert_eq!(line(&report, "setup:"), "setup: installed");
    assert_eq!(line(&report, "data directory state:"), "data directory state: ok");
    assert_eq!(code(&sandbox.run(&["setup", "--remove", "--yes"])), 0);
    assert_eq!(line(&status(&sandbox), "setup:"), "setup: not installed");
}

#[test]
fn status_reports_a_modified_installation() {
    let sandbox = Sandbox::new("status-modified").with_stub();
    assert_eq!(code(&sandbox.run(&["setup", "--yes"])), 0);
    let edited =
        fs::read_to_string(sandbox.settings())
            .unwrap()
            .replacen("\"timeout\": 10", "\"timeout\": 99", 1);
    fs::write(sandbox.settings(), edited).unwrap();
    let report = status(&sandbox);
    assert_eq!(line(&report, "setup:"), "setup: modified");
    assert!(report.contains("agentdust setup --check"), "{report}");
}

#[test]
fn status_reports_whether_apply_is_enabled() {
    let sandbox = Sandbox::new("status-apply").with_stub();
    fs::create_dir(&sandbox.data).unwrap();
    fs::set_permissions(&sandbox.data, fs::Permissions::from_mode(0o700)).unwrap();
    let config = sandbox.data.join("config.toml");
    fs::write(&config, "apply = false\n").unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        line(&status(&sandbox), "apply:"),
        "apply: disabled (apply = false in config.toml)"
    );
    fs::write(&config, "apply = true\n").unwrap();
    assert_eq!(line(&status(&sandbox), "apply:"), "apply: enabled");
    fs::write(&config, "apply what\n").unwrap();
    let report = status(&sandbox);
    assert!(
        line(&report, "apply:").starts_with("apply: disabled (config.toml cannot be used: "),
        "{report}"
    );
    fs::set_permissions(&config, fs::Permissions::from_mode(0o644)).unwrap();
    fs::write(&config, "apply = true\n").unwrap();
    assert!(line(&status(&sandbox), "apply:").starts_with("apply: disabled (config.toml cannot be used: "));
}

#[test]
fn status_takes_no_arguments() {
    let sandbox = Sandbox::new("status-args");
    let output = sandbox.run(&["status", "--json"]);
    assert_eq!(code(&output), 2);
    assert!(String::from_utf8_lossy(&output.stderr).contains("usage: agentdust status"));
    assert_eq!(BIN, env!("CARGO_BIN_EXE_agentdust"));
}
