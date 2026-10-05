use agentdust_agents::claude::{EventError, HookEvent, MAX_CWD_LEN, MAX_ID_LEN, journal_kind, parse_event};
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
            cwd: None,
        }
    );
}

#[test]
fn reads_the_working_directory() {
    let input = br#"{"session_id":"s1","hook_event_name":"PreToolUse","cwd":"/Users/dev/project"}"#;
    let event = parse_event(&input[..]).unwrap();
    assert_eq!(event.cwd.as_deref(), Some("/Users/dev/project"));
}

#[test]
fn an_event_without_a_working_directory_has_none() {
    let event = parse_event(&br#"{"session_id":"s1","hook_event_name":"SessionStart"}"#[..]).unwrap();
    assert_eq!(event.cwd, None);
}

#[test]
fn a_working_directory_keeps_spaces_unicode_and_escapes() {
    let input = r#"{"session_id":"s","hook_event_name":"Stop","cwd":"/Users/d\u00e9v/My Project/\u4e2d"}"#;
    let event = parse_event(input.as_bytes()).unwrap();
    assert_eq!(event.cwd.as_deref(), Some("/Users/d\u{e9}v/My Project/\u{4e2d}"));
}

#[test]
fn the_working_directory_limit_is_4096_bytes() {
    assert_eq!(MAX_CWD_LEN, 4096);
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
        cwd: None,
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

#[test]
fn fields_are_accepted_in_any_order_among_unknown_ones() {
    let input = br#"{"cwd":"/x","tool_name":"Bash","extra":{"a":[1,2,{"b":null}]},
        "hook_event_name":"PreToolUse","tool_use_id":"t1","session_id":"s1","agent_id":"a1"}"#;
    let event = parse_event(&input[..]).unwrap();
    assert_eq!(event.session_id, "s1");
    assert_eq!(event.hook_event_name, "PreToolUse");
    assert_eq!(event.tool_name.as_deref(), Some("Bash"));
    assert_eq!(event.tool_use_id.as_deref(), Some("t1"));
    assert_eq!(event.agent_id.as_deref(), Some("a1"));
    assert_eq!(event.cwd.as_deref(), Some("/x"));
}

#[test]
fn null_optional_fields_are_none() {
    let input = br#"{"session_id":"s","hook_event_name":"Stop","tool_name":null,"tool_use_id":null,"agent_id":null,"cwd":null}"#;
    let event = parse_event(&input[..]).unwrap();
    assert_eq!(
        (event.tool_name, event.tool_use_id, event.agent_id, event.cwd),
        (None, None, None, None)
    );
}

#[test]
fn malformed_events_are_errors() {
    let cases: [&[u8]; 9] = [
        br#"{"session_id":"s"}"#,
        br#"{"session_id":"s","session_id":"t","hook_event_name":"Stop"}"#,
        br#"{"session_id":5,"hook_event_name":"Stop"}"#,
        br#"{"session_id":"s","hook_event_name":"Stop","tool_name":{"a":1}}"#,
        br#"{"session_id":"s","hook_event_name":"Stop","cwd":5}"#,
        br#"{"session_id":"s","hook_event_name":"Stop","cwd":"/a","cwd":"/b"}"#,
        br#"{"session_id":"s","hook_event_name":"Stop","cwd":["/a"]}"#,
        br#"["session_id"]"#,
        br#"{"session_id":"s","hook_event_name":"Sto"#,
    ];
    for input in cases {
        let result = parse_event(input);
        assert!(
            matches!(result, Err(EventError::Json(_))),
            "{}: {result:?}",
            String::from_utf8_lossy(input)
        );
    }
}
