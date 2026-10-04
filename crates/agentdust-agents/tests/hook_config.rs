use std::path::Path;

use agentdust_agents::hook_config::{
    HOOK_SPECS, HOOK_TIMEOUT_SECS, HookState, PatchError, canonical_json, group_hash, hook_command, inspect,
    install, remove, shell_quote, stable_exe,
};
use agentdust_core::digest::sha256_hex;
use agentdust_core::manifest::{Entry, Origin, Resource};
use serde_json::{Value, json};

const CMD: &str = "/opt/homebrew/bin/agentdust hook claude";
const TARGET: &str = "/home/u/.claude/settings.json";

fn states(report: &[agentdust_agents::hook_config::HookReport]) -> Vec<(&'static str, HookState)> {
    report.iter().map(|item| (item.event, item.state)).collect()
}

fn all(state: HookState) -> Vec<(&'static str, HookState)> {
    HOOK_SPECS.iter().map(|spec| (spec.event, state)).collect()
}

fn parsed(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

fn our_group(event: &str) -> Value {
    let mut group = json!({
        "hooks": [{"type": "command", "command": CMD, "timeout": 10}]
    });
    if event.ends_with("ToolUse") {
        group["matcher"] = json!("Bash");
    }
    group
}

fn user_group(matcher: &str, command: &str) -> Value {
    json!({"matcher": matcher, "hooks": [{"type": "command", "command": command}]})
}

fn fresh(text: Option<&str>) -> agentdust_agents::hook_config::Install {
    install(text, TARGET, &[], CMD).unwrap()
}

fn entry_for<'a>(entries: &'a [Entry], event: &str) -> &'a Entry {
    entries.iter().find(|entry| entry.key == event).unwrap()
}

const EXPECTED_FRESH: &str = r#"{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/opt/homebrew/bin/agentdust hook claude",
            "timeout": 10
          }
        ]
      }
    ],
    "SessionEnd": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/opt/homebrew/bin/agentdust hook claude",
            "timeout": 10
          }
        ]
      }
    ],
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "/opt/homebrew/bin/agentdust hook claude",
            "timeout": 10
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "/opt/homebrew/bin/agentdust hook claude",
            "timeout": 10
          }
        ]
      }
    ]
  }
}
"#;

const SETTINGS: &str = r#"{
  "model": "opus",
  "permissions": {
    "allow": ["Bash(ls:*)"]
  },
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Edit",
        "hooks": [
          {
            "type": "command",
            "command": "/usr/bin/python3 \"$HOME/.claude/hooks/lint.py\""
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [{"type": "command", "command": "afplay /System/Library/Sounds/Glass.aiff"}]
      }
    ]
  },
  "env": {"A": "1"}
}
"#;

#[test]
fn the_four_hooks_are_the_ones_the_task_names() {
    let specs: Vec<_> = HOOK_SPECS.iter().map(|spec| (spec.event, spec.matcher)).collect();
    assert_eq!(
        specs,
        [
            ("SessionStart", None),
            ("SessionEnd", None),
            ("PreToolUse", Some("Bash")),
            ("PostToolUse", Some("Bash")),
        ]
    );
    assert_eq!(HOOK_TIMEOUT_SECS, 10);
}

#[test]
fn the_command_is_the_binary_then_hook_claude() {
    assert_eq!(
        hook_command(Path::new("/opt/homebrew/bin/agentdust")),
        "/opt/homebrew/bin/agentdust hook claude"
    );
}

#[test]
fn a_path_with_shell_characters_is_quoted_for_the_shell_that_runs_the_hook() {
    assert_eq!(
        shell_quote("/opt/homebrew/bin/agentdust"),
        "/opt/homebrew/bin/agentdust"
    );
    assert_eq!(
        shell_quote("/Users/Jo Doe/bin/agentdust"),
        "'/Users/Jo Doe/bin/agentdust'"
    );
    assert_eq!(shell_quote("/tmp/it's/agentdust"), "'/tmp/it'\\''s/agentdust'");
    for risky in [
        "/a$b/x", "/a`b`/x", "/a\"b/x", "/a\\b/x", "/a*b/x", "/a;b/x", "/a&b/x", "/a|b/x", "/a(b)/x", "/a b",
        "~/x",
    ] {
        let quoted = shell_quote(risky);
        assert!(
            quoted.starts_with('\'') && quoted.ends_with('\''),
            "{risky}: {quoted}"
        );
    }
    assert_eq!(
        hook_command(Path::new("/Users/Jo Doe/bin/agentdust")),
        "'/Users/Jo Doe/bin/agentdust' hook claude"
    );
}

#[test]
fn a_cellar_path_is_mapped_to_the_stable_prefix() {
    assert_eq!(
        stable_exe(Path::new("/opt/homebrew/Cellar/agentdust/0.1.0/bin/agentdust")),
        Path::new("/opt/homebrew/bin/agentdust")
    );
    assert_eq!(
        stable_exe(Path::new("/usr/local/Cellar/agentdust/0.1.0_1/bin/agentdust")),
        Path::new("/usr/local/bin/agentdust")
    );
    for unchanged in [
        "/opt/homebrew/bin/agentdust",
        "/Users/u/target/debug/agentdust",
        "/opt/homebrew/Cellar/agentdust/0.1.0/libexec/agentdust",
        "/opt/homebrew/Cellar/agentdust/bin/agentdust",
        "/opt/homebrew/Cellar/agentdust/0.1.0/bin/agentdust/extra",
    ] {
        assert_eq!(
            stable_exe(Path::new(unchanged)),
            Path::new(unchanged),
            "{unchanged}"
        );
    }
}

#[test]
fn the_hash_ignores_key_order_and_whitespace_and_nothing_else() {
    let a = json!({"b": [1, {"y": 2, "x": 1}], "a": "s"});
    let b: Value = serde_json::from_str("{ \"a\" : \"s\",\n \"b\": [1, {\"x\": 1, \"y\": 2}] }").unwrap();
    assert_eq!(canonical_json(&a), r#"{"a":"s","b":[1,{"x":1,"y":2}]}"#);
    assert_eq!(group_hash(&a), group_hash(&b));
    assert_eq!(group_hash(&a), sha256_hex(canonical_json(&a).as_bytes()));
    assert_ne!(
        group_hash(&a),
        group_hash(&json!({"b": [{"x": 1, "y": 2}, 1], "a": "s"}))
    );
    assert_ne!(group_hash(&json!({"t": 10})), group_hash(&json!({"t": 11})));
}

#[test]
fn a_fresh_install_writes_the_four_hooks_in_the_usual_layout() {
    for input in [None, Some(""), Some("  \n"), Some("{}\n")] {
        let result = fresh(input);
        assert_eq!(result.text, EXPECTED_FRESH, "{input:?}");
        assert_eq!(states(&result.states), all(HookState::Absent));
    }
    assert_eq!(fresh(Some("{}")).text, EXPECTED_FRESH.trim_end());
}

#[test]
fn a_fresh_install_records_what_it_created() {
    let result = fresh(None);
    assert_eq!(result.entries.len(), 4);
    for spec in HOOK_SPECS {
        let entry = entry_for(&result.entries, spec.event);
        assert_eq!(entry.resource, Resource::Hook);
        assert_eq!(entry.origin, Origin::Created);
        assert_eq!(entry.target, TARGET);
        assert_eq!(entry.command, CMD);
        assert_eq!(entry.hash, group_hash(&our_group(spec.event)));
        assert!(entry.created_event_key, "{}", spec.event);
        assert_eq!(
            entry.created_hooks_key,
            spec.event == "SessionStart",
            "{}",
            spec.event
        );
    }
}

#[test]
fn every_inserted_group_has_the_hash_the_manifest_records() {
    let result = fresh(Some(SETTINGS));
    let settings = parsed(&result.text);
    for spec in HOOK_SPECS {
        let groups = settings["hooks"][spec.event].as_array().unwrap();
        let hashes: Vec<String> = groups.iter().map(group_hash).collect();
        assert!(
            hashes.contains(&entry_for(&result.entries, spec.event).hash),
            "{}",
            spec.event
        );
    }
}

#[test]
fn other_settings_and_other_hooks_survive_an_install() {
    let result = fresh(Some(SETTINGS));
    let before = parsed(SETTINGS);
    let after = parsed(&result.text);
    for key in ["model", "permissions", "env"] {
        assert_eq!(after[key], before[key], "{key}");
    }
    assert_eq!(after["hooks"]["Stop"], before["hooks"]["Stop"]);
    let pre = after["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(pre.len(), 2);
    assert_eq!(pre[0], before["hooks"]["PreToolUse"][0]);
    assert_eq!(pre[1], our_group("PreToolUse"));
    for event in ["SessionStart", "SessionEnd", "PostToolUse"] {
        assert_eq!(after["hooks"][event], json!([our_group(event)]), "{event}");
    }
}

#[test]
fn an_install_changes_nothing_but_insertions() {
    let result = fresh(Some(SETTINGS));
    let removed = remove(Some(&result.text), TARGET, &result.entries).unwrap();
    assert_eq!(removed.text, SETTINGS);
}

#[test]
fn only_the_events_that_had_no_array_record_a_created_event_key() {
    let result = fresh(Some(SETTINGS));
    for (event, created_event, created_hooks) in [
        ("SessionStart", true, false),
        ("SessionEnd", true, false),
        ("PreToolUse", false, false),
        ("PostToolUse", true, false),
    ] {
        let entry = entry_for(&result.entries, event);
        assert_eq!(entry.created_event_key, created_event, "{event}");
        assert_eq!(entry.created_hooks_key, created_hooks, "{event}");
    }
}

#[test]
fn a_second_install_is_a_no_op() {
    let first = fresh(Some(SETTINGS));
    let second = install(Some(&first.text), TARGET, &first.entries, CMD).unwrap();
    assert_eq!(second.text, first.text);
    assert_eq!(second.entries, first.entries);
    assert_eq!(states(&second.states), all(HookState::Installed));
}

#[test]
fn an_equivalent_hook_the_user_already_has_is_recorded_and_not_duplicated() {
    let text = json!({"hooks": {
        "SessionStart": [{"hooks": [{"type": "command", "command": CMD}]}],
        "PreToolUse": [user_group("Bash", CMD)]
    }})
    .to_string();
    let result = fresh(Some(&text));
    assert_eq!(
        states(&result.states),
        [
            ("SessionStart", HookState::PreExisting),
            ("SessionEnd", HookState::Absent),
            ("PreToolUse", HookState::PreExisting),
            ("PostToolUse", HookState::Absent),
        ]
    );
    let after = parsed(&result.text);
    assert_eq!(after["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
    assert_eq!(after["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
    let entry = entry_for(&result.entries, "PreToolUse");
    assert_eq!(entry.origin, Origin::PreExisting);
    assert_eq!(entry.hash, group_hash(&user_group("Bash", CMD)));
    assert_eq!(entry_for(&result.entries, "SessionEnd").origin, Origin::Created);
}

#[test]
fn a_pre_existing_hook_is_never_removed() {
    let text = json!({"hooks": {"PreToolUse": [user_group("Bash", CMD)]}}).to_string();
    let result = fresh(Some(&text));
    let removed = remove(Some(&result.text), TARGET, &result.entries).unwrap();
    assert_eq!(parsed(&removed.text), parsed(&text));
    assert_eq!(removed.released, ["PreToolUse"]);
    assert_eq!(removed.removed, ["SessionStart", "SessionEnd", "PostToolUse"]);
    assert!(removed.kept.is_empty());
}

#[test]
fn the_same_command_under_another_matcher_or_type_is_not_equivalent() {
    let text = json!({"hooks": {
        "PreToolUse": [user_group("Edit", CMD)],
        "PostToolUse": [{"matcher": "Bash", "hooks": [{"type": "prompt", "command": CMD}]}],
        "SessionStart": [{"matcher": "startup", "hooks": [{"type": "command", "command": CMD}]}]
    }})
    .to_string();
    let result = fresh(Some(&text));
    assert_eq!(states(&result.states), all(HookState::Absent));
    assert_eq!(
        parsed(&result.text)["hooks"]["PreToolUse"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn another_command_for_the_same_event_is_left_alone() {
    let text = json!({"hooks": {"SessionStart": [user_group("", "/usr/local/bin/other-tool")]}}).to_string();
    let result = fresh(Some(&text));
    let after = parsed(&result.text);
    assert_eq!(
        after["hooks"]["SessionStart"][0],
        user_group("", "/usr/local/bin/other-tool")
    );
    assert_eq!(after["hooks"]["SessionStart"][1], our_group("SessionStart"));
}

#[test]
fn a_hand_edited_group_is_reported_as_modified_and_left_alone() {
    let first = fresh(Some(SETTINGS));
    let edited = first.text.replacen("\"timeout\": 10", "\"timeout\": 99", 1);
    assert_ne!(edited, first.text);
    let report = inspect(Some(&edited), TARGET, &first.entries, CMD).unwrap();
    assert_eq!(report[2].state, HookState::Modified);
    assert_eq!(report[0].state, HookState::Installed);
    let again = install(Some(&edited), TARGET, &first.entries, CMD).unwrap();
    assert_eq!(again.text, edited);
    assert_eq!(
        entry_for(&again.entries, "PreToolUse"),
        entry_for(&first.entries, "PreToolUse")
    );
    let removed = remove(Some(&edited), TARGET, &first.entries).unwrap();
    assert_eq!(removed.drift, ["PreToolUse"]);
    assert_eq!(removed.kept.len(), 1);
    assert_eq!(removed.kept[0].key, "PreToolUse");
    let after = parsed(&removed.text);
    assert_eq!(after["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
    assert_eq!(after["hooks"]["PreToolUse"][1]["hooks"][0]["timeout"], json!(99));
    assert!(after["hooks"].get("SessionEnd").is_none());
}

#[test]
fn a_group_whose_command_was_changed_counts_as_missing_not_as_ours() {
    let first = fresh(Some(SETTINGS));
    let edited = first.text.replacen(CMD, "/usr/bin/true", 1);
    let report = inspect(Some(&edited), TARGET, &first.entries, CMD).unwrap();
    assert_eq!(report[2].state, HookState::Missing);
    assert_eq!(report[0].state, HookState::Installed);
    let removed = remove(Some(&edited), TARGET, &first.entries).unwrap();
    assert_eq!(removed.gone, ["PreToolUse"]);
    assert!(removed.drift.is_empty());
}

#[test]
fn a_group_the_user_deleted_is_missing_and_install_puts_it_back() {
    let first = fresh(Some(SETTINGS));
    let without = remove_event(&first.text, "SessionEnd");
    let report = inspect(Some(&without), TARGET, &first.entries, CMD).unwrap();
    assert_eq!(report[1].state, HookState::Missing);
    let again = install(Some(&without), TARGET, &first.entries, CMD).unwrap();
    assert_eq!(
        parsed(&again.text)["hooks"]["SessionEnd"],
        json!([our_group("SessionEnd")])
    );
    assert_eq!(states(&again.states)[1], ("SessionEnd", HookState::Missing));
    assert_eq!(
        entry_for(&again.entries, "SessionEnd").hash,
        group_hash(&our_group("SessionEnd"))
    );
}

fn remove_event(text: &str, event: &str) -> String {
    let mut value = parsed(text);
    value["hooks"].as_object_mut().unwrap().remove(event);
    serde_json::to_string_pretty(&value).unwrap()
}

#[test]
fn an_entry_written_for_another_binary_is_stale_and_install_does_not_double_it() {
    let old_cmd = "/Users/u/target/debug/agentdust hook claude";
    let first = install(None, TARGET, &[], old_cmd).unwrap();
    let report = inspect(Some(&first.text), TARGET, &first.entries, CMD).unwrap();
    assert_eq!(states(&report), all(HookState::Stale));
    let again = install(Some(&first.text), TARGET, &first.entries, CMD).unwrap();
    assert_eq!(again.text, first.text);
    assert_eq!(again.entries, first.entries);
    let removed = remove(Some(&first.text), TARGET, &first.entries).unwrap();
    assert_eq!(removed.removed.len(), 4);
}

#[test]
fn entries_for_another_settings_file_are_ignored() {
    let first = install(None, "/other/settings.json", &[], CMD).unwrap();
    let report = inspect(Some(EXPECTED_FRESH), TARGET, &first.entries, CMD).unwrap();
    assert_eq!(states(&report), all(HookState::PreExisting));
    let removed = remove(Some(EXPECTED_FRESH), TARGET, &first.entries).unwrap();
    assert_eq!(removed.text, EXPECTED_FRESH);
    assert!(removed.removed.is_empty());
}

#[test]
fn removing_from_a_missing_file_reports_everything_gone() {
    let first = fresh(None);
    let removed = remove(None, TARGET, &first.entries).unwrap();
    assert_eq!(removed.gone.len(), 4);
    assert!(removed.removed.is_empty());
    assert!(removed.kept.is_empty());
}

#[test]
fn removing_what_setup_created_from_a_fresh_file_leaves_an_empty_object() {
    let first = fresh(None);
    let removed = remove(Some(&first.text), TARGET, &first.entries).unwrap();
    assert_eq!(parsed(&removed.text), json!({}));
    assert_eq!(removed.removed.len(), 4);
}

#[test]
fn an_empty_hooks_object_the_user_had_is_kept_on_removal() {
    let text = "{\n  \"hooks\": {}\n}\n";
    let first = fresh(Some(text));
    assert!(first.entries.iter().all(|entry| !entry.created_hooks_key));
    let removed = remove(Some(&first.text), TARGET, &first.entries).unwrap();
    assert_eq!(removed.text, text);
}

#[test]
fn an_empty_event_array_the_user_had_is_kept_on_removal() {
    let text = "{\n  \"hooks\": {\n    \"SessionEnd\": []\n  }\n}\n";
    let first = fresh(Some(text));
    assert!(!entry_for(&first.entries, "SessionEnd").created_event_key);
    let removed = remove(Some(&first.text), TARGET, &first.entries).unwrap();
    assert_eq!(removed.text, text);
}

#[test]
fn a_hooks_object_that_still_holds_other_events_is_kept_on_removal() {
    let first = fresh(Some("{\n  \"hooks\": {\n    \"Stop\": []\n  }\n}\n"));
    let removed = remove(Some(&first.text), TARGET, &first.entries).unwrap();
    assert_eq!(removed.text, "{\n  \"hooks\": {\n    \"Stop\": []\n  }\n}\n");
}

#[test]
fn the_layout_of_other_files_round_trips_byte_for_byte() {
    for text in [
        "{\"model\":\"opus\"}",
        "{\"model\":\"opus\"}\n",
        "{\n    \"model\": \"opus\"\n}",
        "{\n\t\"model\": \"opus\",\n\t\"hooks\": {\n\t\t\"Stop\": []\n\t}\n}\n",
        "{\n  \"a\": 1,\n  \"hooks\": {\n    \"SessionStart\": [\n      {\n        \"hooks\": []\n      }\n    ]\n  }\n}\n",
    ] {
        let first = fresh(Some(text));
        assert_ne!(first.text, text);
        let removed = remove(Some(&first.text), TARGET, &first.entries).unwrap();
        assert_eq!(removed.text, text, "{text:?}");
    }
}

#[test]
fn unusable_shapes_are_refused_and_never_edited() {
    for text in [
        "[]",
        "\"x\"",
        "null",
        "{",
        "{\"hooks\": []}",
        "{\"hooks\": \"x\"}",
        "{\"hooks\": {\"PreToolUse\": {}}}",
        "{\"hooks\": {\"SessionStart\": \"x\"}}",
        "{\"hooks\": {}, \"hooks\": {}}",
        "{\"hooks\": {\"Stop\": [], \"Stop\": []}}",
    ] {
        assert!(install(Some(text), TARGET, &[], CMD).is_err(), "{text}");
        assert!(inspect(Some(text), TARGET, &[], CMD).is_err(), "{text}");
    }
    assert!(matches!(
        install(Some("{\"hooks\": []}"), TARGET, &[], CMD),
        Err(PatchError::Shape(_))
    ));
    assert!(matches!(
        install(Some("{"), TARGET, &[], CMD),
        Err(PatchError::Syntax(_))
    ));
    assert!(matches!(
        install(Some("{\"hooks\": {}, \"hooks\": {}}"), TARGET, &[], CMD),
        Err(PatchError::Duplicate(_))
    ));
}

#[test]
fn a_group_that_is_not_an_object_does_not_stop_the_scan() {
    let text = json!({"hooks": {"SessionStart": ["junk", 3, null, {"hooks": "x"}]}}).to_string();
    let result = fresh(Some(&text));
    assert_eq!(states(&result.states)[0], ("SessionStart", HookState::Absent));
    assert_eq!(
        parsed(&result.text)["hooks"]["SessionStart"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
}
