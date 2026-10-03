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
            tool_use_id: Some("toolu_9".into()),
            agent_id: None,
        }
    );
}

#[test]
fn maps_hook_events_to_journal_kinds() {
    let event = |name: &str| HookEvent {
        session_id: "s".into(),
        hook_event_name: name.into(),
        tool_use_id: None,
        agent_id: None,
    };
    assert_eq!(journal_kind(&event("SessionStart")), Some(Kind::SessionStart));
    assert_eq!(journal_kind(&event("SessionEnd")), Some(Kind::SessionEnd));
    assert_eq!(journal_kind(&event("PreToolUse")), Some(Kind::ShellStart));
    assert_eq!(journal_kind(&event("PostToolUse")), Some(Kind::ShellEnd));
    assert_eq!(journal_kind(&event("Stop")), None);
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
