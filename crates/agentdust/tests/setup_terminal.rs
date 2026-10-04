mod setup_support;

use std::fs;

use serde_json::Value;
use setup_support::{PROMPT, Sandbox, code, read, run_on_terminal};

const SETTINGS: &str = "{\n  \"model\": \"opus\"\n}\n";

#[test]
fn answering_n_at_the_prompt_changes_nothing() {
    let sandbox = Sandbox::new("tty-no").with_stub();
    sandbox.write_settings(SETTINGS, 0o644);
    let (status, shown) = run_on_terminal(&sandbox, &["setup"], "n\n");
    assert_eq!(status.code(), Some(1), "{shown}");
    assert!(shown.contains("--- "), "{shown}");
    assert!(shown.contains(PROMPT), "{shown}");
    assert!(shown.contains("Cancelled"), "{shown}");
    assert_eq!(read(&sandbox.settings()), SETTINGS);
    assert!(!sandbox.manifest().exists());
    assert!(sandbox.server().is_none());
    assert!(!sandbox.calls().iter().any(|call| call.starts_with("mcp add")));
}

#[test]
fn an_empty_answer_declines() {
    let sandbox = Sandbox::new("tty-empty").with_stub();
    sandbox.write_settings(SETTINGS, 0o644);
    let (status, shown) = run_on_terminal(&sandbox, &["setup"], "\n");
    assert_eq!(status.code(), Some(1), "{shown}");
    assert_eq!(read(&sandbox.settings()), SETTINGS);
}

#[test]
fn answering_y_at_the_prompt_applies_the_plan_that_was_shown() {
    let sandbox = Sandbox::new("tty-yes").with_stub();
    sandbox.write_settings(SETTINGS, 0o644);
    let (status, shown) = run_on_terminal(&sandbox, &["setup"], "y\n");
    assert_eq!(status.code(), Some(0), "{shown}");
    assert!(shown.contains(PROMPT), "{shown}");
    assert!(shown.contains("Setup finished"), "{shown}");
    let settings: Value = serde_json::from_str(&read(&sandbox.settings())).unwrap();
    assert!(settings["hooks"]["SessionStart"].is_array());
    assert!(sandbox.server().is_some());
    assert!(sandbox.manifest().exists());
}

#[test]
fn removal_asks_too_and_a_no_keeps_everything() {
    let sandbox = Sandbox::new("tty-remove").with_stub();
    assert_eq!(code(&sandbox.run(&["setup", "--yes"])), 0);
    let before = read(&sandbox.settings());
    let (status, shown) = run_on_terminal(&sandbox, &["setup", "--remove"], "n\n");
    assert_eq!(status.code(), Some(1), "{shown}");
    assert!(shown.contains(PROMPT), "{shown}");
    assert_eq!(read(&sandbox.settings()), before);
    assert!(sandbox.server().is_some());
    assert!(fs::metadata(sandbox.manifest()).is_ok());
}

#[test]
fn check_never_asks() {
    let sandbox = Sandbox::new("tty-check").with_stub();
    let (status, shown) = run_on_terminal(&sandbox, &["setup", "--check"], "y\n");
    assert_eq!(status.code(), Some(1), "{shown}");
    assert!(!shown.contains(PROMPT), "{shown}");
}
