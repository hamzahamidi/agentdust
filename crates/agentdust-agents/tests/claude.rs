use agentdust_agents::claude::{EventError, HookEvent, MAX_ID_LEN, journal_kind, parse_event};
use agentdust_core::journal::Kind;

#[test]
fn reads_only_the_fields_it_needs() {
    let input = br#"{"session_id":"s1","hook_event_name":"PostToolUse","tool_use_id":"toolu_9",
        "tool_name":"Bash","tool_input":{"command":"ls"},"tool_response":{"stdout":"secret"}}"#;
    let event = parse_event(&input[..]).unwrap();
    assert_eq!(
        event,
        HookEvent {
            session_id: "s1".into(),
            hook_event_name: "PostToolUse".into(),
            tool_name: Some("Bash".into()),
            tool_use_id: Some("toolu_9".into()),
            agent_id: None,
        }
    );
}

#[test]
fn an_event_without_a_tool_name_has_none() {
    let event = parse_event(&br#"{"session_id":"s1","hook_event_name":"SessionStart"}"#[..]).unwrap();
    assert_eq!(event.tool_name, None);
}

fn event(name: &str, tool: Option<&str>) -> HookEvent {
    HookEvent {
        session_id: "s".into(),
        hook_event_name: name.into(),
        tool_name: tool.map(Into::into),
        tool_use_id: None,
        agent_id: None,
    }
}

#[test]
fn maps_bash_tool_events_to_shell_kinds() {
    assert_eq!(
        journal_kind(&event("PreToolUse", Some("Bash"))),
        Some(Kind::ShellStart)
    );
    assert_eq!(
        journal_kind(&event("PostToolUse", Some("Bash"))),
        Some(Kind::ShellEnd)
    );
}

#[test]
fn ignores_tool_events_for_any_other_tool() {
    for tool in [
        None,
        Some("Read"),
        Some("Edit"),
        Some("mcp__srv__Bash"),
        Some("bash"),
        Some("Bash "),
        Some(""),
    ] {
        assert_eq!(journal_kind(&event("PreToolUse", tool)), None, "{tool:?}");
        assert_eq!(journal_kind(&event("PostToolUse", tool)), None, "{tool:?}");
    }
}

#[test]
fn session_events_map_whatever_the_tool_name() {
    for tool in [None, Some("Bash"), Some("Read")] {
        assert_eq!(
            journal_kind(&event("SessionStart", tool)),
            Some(Kind::SessionStart)
        );
        assert_eq!(journal_kind(&event("SessionEnd", tool)), Some(Kind::SessionEnd));
        assert_eq!(journal_kind(&event("Stop", tool)), None);
    }
}

#[test]
fn rejects_identifiers_longer_than_the_limit() {
    let long = "s".repeat(MAX_ID_LEN + 1);
    let input = format!(r#"{{"session_id":"{long}","hook_event_name":"PreToolUse"}}"#);
    assert!(matches!(
        parse_event(input.as_bytes()),
        Err(EventError::FieldTooLong)
    ));
}

#[test]
fn rejects_an_event_without_session_id() {
    assert!(parse_event(&br#"{"hook_event_name":"PreToolUse"}"#[..]).is_err());
}
