mod setup_support;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::process::{Command, Stdio};

use agentdust_core::journal::{self, Kind};
use serde_json::{Value, json};
use setup_support::{BIN, Sandbox, code, hook_command, read, text};

const SETTINGS: &str = "{\n  \"model\": \"opus\",\n  \"hooks\": {\n    \"Stop\": [\n      {\n        \"hooks\": [{\"type\": \"command\", \"command\": \"afplay done.aiff\"}]\n      }\n    ]\n  }\n}\n";

fn settings_json(sandbox: &Sandbox) -> Value {
    serde_json::from_str(&read(&sandbox.settings())).unwrap()
}

fn is_root() -> bool {
    Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .is_ok_and(|output| output.stdout == b"0\n")
}

#[test]
fn setup_with_yes_installs_the_hooks_registers_the_server_and_says_what_it_did() {
    let sandbox = Sandbox::new("fresh").with_stub();
    let output = sandbox.run(&["setup", "--yes"]);
    assert_eq!(code(&output), 0, "{}", text(&output));
    let settings = settings_json(&sandbox);
    for (event, matcher) in [
        ("SessionStart", None),
        ("SessionEnd", None),
        ("PreToolUse", Some("Bash")),
        ("PostToolUse", Some("Bash")),
    ] {
        let group = &settings["hooks"][event][0];
        assert_eq!(group["hooks"][0]["command"], json!(hook_command()), "{event}");
        assert_eq!(group["hooks"][0]["type"], json!("command"), "{event}");
        assert_eq!(group["hooks"][0]["timeout"], json!(10), "{event}");
        assert_eq!(group.get("matcher").and_then(Value::as_str), matcher, "{event}");
    }
    assert_eq!(
        sandbox.calls(),
        [
            "mcp get agentdust".to_owned(),
            format!("mcp add --scope user agentdust -- {BIN} mcp"),
            "mcp get agentdust".to_owned()
        ]
    );
    assert_eq!(sandbox.server(), Some((BIN.to_owned(), "mcp".to_owned())));
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    for needed in ["--- ", "+++ ", "SessionStart", "Setup finished"] {
        assert!(stdout.contains(needed), "{needed}: {stdout}");
    }
    assert_eq!(
        fs::metadata(sandbox.manifest()).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(&sandbox.data).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[test]
fn the_hook_that_setup_writes_runs_and_records_an_event() {
    let sandbox = Sandbox::new("hook-runs").with_stub();
    assert_eq!(code(&sandbox.run(&["setup", "--yes"])), 0);
    let command = settings_json(&sandbox)["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut child = Command::new("/bin/sh")
        .args(["-c", &command])
        .env_clear()
        .env("AGENTDUST_DATA_DIR", &sandbox.data)
        .env("HOME", &sandbox.home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(br#"{"session_id":"s1","hook_event_name":"SessionStart"}"#)
            .unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    let report = journal::read(&sandbox.data).unwrap();
    assert_eq!(report.records.len(), 1);
    assert_eq!(report.records[0].kind, Kind::SessionStart);
}

#[test]
fn a_second_run_is_a_no_op_and_changes_no_file() {
    let sandbox = Sandbox::new("idempotent").with_stub();
    sandbox.write_settings(SETTINGS, 0o644);
    assert_eq!(code(&sandbox.run(&["setup", "--yes"])), 0);
    let settings = read(&sandbox.settings());
    let manifest = read(&sandbox.manifest());
    let calls = sandbox.calls().len();
    let output = sandbox.run(&["setup", "--yes"]);
    assert_eq!(code(&output), 0, "{}", text(&output));
    assert!(text(&output).contains("Nothing to change"), "{}", text(&output));
    assert_eq!(read(&sandbox.settings()), settings);
    assert_eq!(read(&sandbox.manifest()), manifest);
    assert_eq!(sandbox.calls().len(), calls + 1);
    assert_eq!(
        settings_json(&sandbox)["hooks"]["SessionStart"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn unrelated_settings_and_hooks_survive_and_the_mode_is_kept() {
    let sandbox = Sandbox::new("unrelated").with_stub();
    sandbox.write_settings(SETTINGS, 0o644);
    assert_eq!(code(&sandbox.run(&["setup", "--yes"])), 0);
    let settings = settings_json(&sandbox);
    assert_eq!(settings["model"], json!("opus"));
    assert_eq!(
        settings["hooks"]["Stop"][0]["hooks"][0]["command"],
        json!("afplay done.aiff")
    );
    assert_eq!(
        fs::metadata(sandbox.settings()).unwrap().permissions().mode() & 0o777,
        0o644
    );
    let names: Vec<_> = fs::read_dir(&sandbox.claude)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, ["settings.json"]);
}

#[test]
fn check_exits_1_before_setup_and_0_after_and_1_when_an_entry_was_edited() {
    let sandbox = Sandbox::new("check").with_stub();
    let before = sandbox.run(&["setup", "--check"]);
    assert_eq!(code(&before), 1, "{}", text(&before));
    assert!(text(&before).contains("not installed"), "{}", text(&before));
    assert_eq!(code(&sandbox.run(&["setup", "--yes"])), 0);
    let after = sandbox.run(&["setup", "--check"]);
    assert_eq!(code(&after), 0, "{}", text(&after));
    assert!(text(&after).contains("Setup is installed"), "{}", text(&after));
    let edited = read(&sandbox.settings()).replacen("\"timeout\": 10", "\"timeout\": 99", 1);
    fs::write(sandbox.settings(), edited).unwrap();
    let drift = sandbox.run(&["setup", "--check"]);
    assert_eq!(code(&drift), 1, "{}", text(&drift));
    assert!(text(&drift).contains("modified"), "{}", text(&drift));
}

#[test]
fn check_changes_nothing() {
    let sandbox = Sandbox::new("check-readonly").with_stub();
    sandbox.write_settings(SETTINGS, 0o644);
    let output = sandbox.run(&["setup", "--check"]);
    assert_eq!(code(&output), 1, "{}", text(&output));
    assert_eq!(read(&sandbox.settings()), SETTINGS);
    assert!(!sandbox.data.exists());
    assert!(sandbox.server().is_none());
}

#[test]
fn remove_takes_out_only_what_setup_added_and_says_so() {
    let sandbox = Sandbox::new("remove").with_stub();
    sandbox.write_settings(SETTINGS, 0o644);
    assert_eq!(code(&sandbox.run(&["setup", "--yes"])), 0);
    let output = sandbox.run(&["setup", "--remove", "--yes"]);
    assert_eq!(code(&output), 0, "{}", text(&output));
    assert_eq!(read(&sandbox.settings()), SETTINGS);
    assert!(sandbox.server().is_none());
    assert!(!sandbox.manifest().exists());
    assert!(text(&output).contains("removed"), "{}", text(&output));
}

#[test]
fn remove_leaves_what_it_did_not_create() {
    let sandbox = Sandbox::new("remove-own").with_stub();
    sandbox.write_settings(SETTINGS, 0o644);
    assert_eq!(code(&sandbox.run(&["setup", "--yes"])), 0);
    let mut settings = settings_json(&sandbox);
    settings["hooks"]["Notification"] =
        json!([{"hooks": [{"type": "command", "command": "afplay ping.aiff"}]}]);
    fs::write(
        sandbox.settings(),
        serde_json::to_string_pretty(&settings).unwrap(),
    )
    .unwrap();
    assert_eq!(code(&sandbox.run(&["setup", "--remove", "--yes"])), 0);
    let after = settings_json(&sandbox);
    assert_eq!(
        after["hooks"]["Notification"][0]["hooks"][0]["command"],
        json!("afplay ping.aiff")
    );
    assert_eq!(
        after["hooks"]["Stop"][0]["hooks"][0]["command"],
        json!("afplay done.aiff")
    );
    assert!(after["hooks"].get("SessionStart").is_none());
}

#[test]
fn remove_reports_an_edited_entry_as_drift_and_exits_1() {
    let sandbox = Sandbox::new("remove-drift").with_stub();
    assert_eq!(code(&sandbox.run(&["setup", "--yes"])), 0);
    let edited = read(&sandbox.settings()).replacen("\"timeout\": 10", "\"timeout\": 99", 1);
    fs::write(sandbox.settings(), &edited).unwrap();
    let output = sandbox.run(&["setup", "--remove", "--yes"]);
    assert_eq!(code(&output), 1, "{}", text(&output));
    assert!(text(&output).contains("modified"), "{}", text(&output));
    assert!(read(&sandbox.settings()).contains("\"timeout\": 99"));
    assert!(sandbox.manifest().exists());
}

#[test]
fn a_symlinked_settings_file_is_refused_and_the_change_is_printed() {
    let sandbox = Sandbox::new("symlink").with_stub();
    let target = sandbox.root.join("dotfiles-settings.json");
    fs::write(&target, SETTINGS).unwrap();
    symlink(&target, sandbox.settings()).unwrap();
    let output = sandbox.run(&["setup", "--yes"]);
    assert_eq!(code(&output), 1, "{}", text(&output));
    let all = text(&output);
    assert!(all.contains("symbolic link"), "{all}");
    assert!(all.contains("\"SessionStart\""), "{all}");
    assert_eq!(read(&target), SETTINGS);
    assert!(
        fs::symlink_metadata(sandbox.settings())
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn a_cli_that_lies_is_detected_and_everything_is_rolled_back() {
    let sandbox = Sandbox::new("liar").with_stub();
    sandbox.write_settings(SETTINGS, 0o644);
    let output = sandbox.run_mode(&["setup", "--yes"], "liar");
    assert_eq!(code(&output), 1, "{}", text(&output));
    let all = text(&output);
    assert!(all.contains("no such server"), "{all}");
    assert!(all.contains("Rolled back"), "{all}");
    assert_eq!(read(&sandbox.settings()), SETTINGS);
    assert!(!sandbox.manifest().exists());
    assert!(sandbox.server().is_none());
}

#[test]
fn a_failing_cli_leaves_no_settings_file_behind() {
    let sandbox = Sandbox::new("add-fails").with_stub();
    let output = sandbox.run_mode(&["setup", "--yes"], "add-fails");
    assert_eq!(code(&output), 1, "{}", text(&output));
    assert!(
        text(&output).contains("cannot write the configuration"),
        "{}",
        text(&output)
    );
    assert!(!sandbox.settings().exists());
    assert!(!sandbox.manifest().exists());
}

#[test]
fn an_unreadable_settings_file_is_reported_and_left_alone() {
    if is_root() {
        return;
    }
    let sandbox = Sandbox::new("unreadable").with_stub();
    sandbox.write_settings(SETTINGS, 0o000);
    let output = sandbox.run(&["setup", "--yes"]);
    assert_eq!(code(&output), 1, "{}", text(&output));
    assert!(text(&output).contains("cannot be read"), "{}", text(&output));
    fs::set_permissions(sandbox.settings(), fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(read(&sandbox.settings()), SETTINGS);
}

#[test]
fn without_a_terminal_and_without_yes_nothing_is_touched_and_the_cli_is_never_run() {
    let sandbox = Sandbox::new("no-consent").with_stub();
    sandbox.write_settings(SETTINGS, 0o644);
    let output = sandbox.run(&["setup"]);
    assert_eq!(code(&output), 1, "{}", text(&output));
    assert!(text(&output).contains("--yes"), "{}", text(&output));
    assert_eq!(read(&sandbox.settings()), SETTINGS);
    assert!(!sandbox.data.exists());
    assert!(sandbox.calls().is_empty());
    let removal = sandbox.run(&["setup", "--remove"]);
    assert_eq!(code(&removal), 1, "{}", text(&removal));
    assert!(sandbox.calls().is_empty());
}

#[test]
fn without_the_claude_cli_the_hooks_are_installed_and_the_exact_command_is_printed() {
    let sandbox = Sandbox::new("no-cli");
    let output = sandbox.run_without_cli(&["setup", "--yes"]);
    assert_eq!(code(&output), 0, "{}", text(&output));
    let manual = format!("claude mcp add --scope user agentdust -- {BIN} mcp");
    assert!(text(&output).contains(&manual), "{}", text(&output));
    assert_eq!(
        settings_json(&sandbox)["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        json!(hook_command())
    );
    let check = sandbox.run_without_cli(&["setup", "--check"]);
    assert_eq!(code(&check), 1, "{}", text(&check));
}

#[test]
fn a_server_with_the_same_name_is_never_overwritten() {
    let sandbox = Sandbox::new("conflict").with_stub();
    fs::write(sandbox.state.join("server"), "/theirs/agentdust\nserve\n").unwrap();
    let output = sandbox.run(&["setup", "--yes"]);
    assert_eq!(code(&output), 1, "{}", text(&output));
    assert!(text(&output).contains("/theirs/agentdust"), "{}", text(&output));
    assert_eq!(
        sandbox.server(),
        Some(("/theirs/agentdust".to_owned(), "serve".to_owned()))
    );
    assert!(
        !sandbox
            .calls()
            .iter()
            .any(|call| call.starts_with("mcp add") || call.starts_with("mcp remove"))
    );
}

#[test]
fn a_manifest_from_a_future_version_is_refused_by_setup_check_and_remove() {
    let sandbox = Sandbox::new("future").with_stub();
    fs::create_dir(&sandbox.data).unwrap();
    fs::set_permissions(&sandbox.data, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(sandbox.manifest(), "{\"version\":2,\"entries\":[]}").unwrap();
    fs::set_permissions(sandbox.manifest(), fs::Permissions::from_mode(0o600)).unwrap();
    for args in [
        &["setup", "--yes"][..],
        &["setup", "--check"],
        &["setup", "--remove", "--yes"],
    ] {
        let output = sandbox.run(args);
        assert_eq!(code(&output), 1, "{args:?}: {}", text(&output));
        assert!(text(&output).contains("version 2"), "{args:?}: {}", text(&output));
    }
    assert!(!sandbox.settings().exists());
    assert!(sandbox.calls().is_empty());
}

#[test]
fn bad_arguments_are_a_usage_error_and_change_nothing() {
    let sandbox = Sandbox::new("usage").with_stub();
    for args in [
        &["setup", "--check", "--remove"][..],
        &["setup", "--check", "--yes"],
        &["setup", "--bogus"],
        &["setup", "extra"],
        &["setup", "--yes", "--yes"],
    ] {
        let output = sandbox.run(args);
        assert_eq!(code(&output), 2, "{args:?}: {}", text(&output));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("usage: agentdust setup"),
            "{args:?}"
        );
    }
    assert!(!sandbox.settings().exists());
    assert!(!sandbox.data.exists());
    assert!(sandbox.calls().is_empty());
}

#[test]
fn the_usage_line_for_an_unknown_command_names_setup_and_status() {
    let output = Command::new(BIN).arg("nope").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.starts_with("usage: agentdust"), "{stderr}");
    assert!(stderr.contains("setup") && stderr.contains("status"), "{stderr}");
}
