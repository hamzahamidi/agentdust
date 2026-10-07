use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use agentdust_agents::claude::{self, EventError, MAX_CWD_LEN, MAX_ID_LEN};
use agentdust_core::journal::Kind;

fn seeds() -> Vec<(String, Vec<u8>)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/claude_payload");
    let mut seeds: Vec<_> = fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("{}: {err}", dir.display()))
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().into_string().unwrap(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect();
    seeds.sort();
    seeds
}

fn fixture_events(input: &'static str) -> Vec<claude::HookEvent> {
    input
        .lines()
        .map(|line| claude::parse_event(line.as_bytes()).unwrap())
        .collect()
}

fn kinds(events: &[claude::HookEvent]) -> Vec<Option<Kind>> {
    events.iter().map(claude::journal_kind).collect()
}

fn assert_fixture_is_sanitized(input: &str) {
    for private_field in [
        "\"cwd\":",
        "\"transcript_path\":",
        "\"tool_input\":",
        "\"tool_response\":",
        "\"agent_type\":",
    ] {
        assert!(!input.contains(private_field), "{private_field}");
    }
}

fn tool_event_signatures(
    events: &[claude::HookEvent],
    hook_event_name: &str,
) -> BTreeSet<(String, Option<String>, String, String)> {
    events
        .iter()
        .filter(|event| event.hook_event_name == hook_event_name)
        .map(|event| {
            (
                event.session_id.clone(),
                event.agent_id.clone(),
                event.tool_name.clone().expect("tool event has a name"),
                event.tool_use_id.clone().expect("tool event has an ID"),
            )
        })
        .collect()
}

fn assert_tool_events_pair(events: &[claude::HookEvent]) {
    let starts = tool_event_signatures(events, "PreToolUse");
    let ends = tool_event_signatures(events, "PostToolUse");
    assert_eq!(
        starts.len(),
        events
            .iter()
            .filter(|event| event.hook_event_name == "PreToolUse")
            .count()
    );
    assert_eq!(
        ends.len(),
        events
            .iter()
            .filter(|event| event.hook_event_name == "PostToolUse")
            .count()
    );
    assert_eq!(starts, ends);
}

#[test]
fn every_payload_seed_parses_without_panicking_and_keeps_its_limits() {
    let seeds = seeds();
    assert!(seeds.len() >= 18, "{} seeds", seeds.len());
    for (name, data) in &seeds {
        let Ok(event) = claude::parse_event(&data[..]) else {
            continue;
        };
        for text in [
            Some(&event.session_id),
            Some(&event.hook_event_name),
            event.tool_name.as_ref(),
            event.tool_use_id.as_ref(),
            event.agent_id.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            assert!(text.len() <= MAX_ID_LEN, "{name}");
        }
        assert!(
            event.cwd.as_ref().is_none_or(|cwd| cwd.len() <= MAX_CWD_LEN),
            "{name}"
        );
        let _ = claude::journal_kind(&event);
    }
}

#[test]
fn sanitized_real_claude_subagent_fixture_replays_through_the_adapter() {
    const FIXTURE: &str = include_str!("../../../fixtures/m6/claude-2.1.289-two-subagents.jsonl");
    let events = fixture_events(FIXTURE);
    let kinds = kinds(&events);
    assert_eq!(
        kinds,
        [
            Some(Kind::SessionStart),
            Some(Kind::SubagentStart),
            Some(Kind::SubagentStart),
            Some(Kind::ShellStart),
            Some(Kind::ShellStart),
            Some(Kind::ShellEnd),
            Some(Kind::ShellEnd),
            Some(Kind::SubagentStop),
            Some(Kind::SubagentStop),
            Some(Kind::SessionEnd),
        ]
    );

    let session_ids: std::collections::BTreeSet<_> =
        events.iter().map(|event| event.session_id.as_str()).collect();
    assert_eq!(session_ids.len(), 1);
    let started: std::collections::BTreeSet<_> = events
        .iter()
        .filter(|event| event.hook_event_name == "SubagentStart")
        .map(|event| event.agent_id.as_deref().unwrap())
        .collect();
    let stopped: std::collections::BTreeSet<_> = events
        .iter()
        .filter(|event| event.hook_event_name == "SubagentStop")
        .map(|event| event.agent_id.as_deref().unwrap())
        .collect();
    assert_eq!(started.len(), 2);
    assert_eq!(started, stopped);

    let shell_starts: std::collections::BTreeSet<_> = events
        .iter()
        .filter(|event| claude::journal_kind(event) == Some(Kind::ShellStart))
        .map(|event| {
            (
                event.session_id.as_str(),
                event.agent_id.as_deref().unwrap(),
                event.tool_use_id.as_deref().unwrap(),
            )
        })
        .collect();
    let shell_ends: std::collections::BTreeSet<_> = events
        .iter()
        .filter(|event| claude::journal_kind(event) == Some(Kind::ShellEnd))
        .map(|event| {
            (
                event.session_id.as_str(),
                event.agent_id.as_deref().unwrap(),
                event.tool_use_id.as_deref().unwrap(),
            )
        })
        .collect();
    assert_eq!(shell_starts.len(), 2);
    assert_eq!(shell_starts, shell_ends);
    for agent_id in &started {
        assert_eq!(
            shell_starts
                .iter()
                .filter(|(_, owner, _)| *owner == *agent_id)
                .count(),
            1,
            "each captured helper call belongs to its own subagent"
        );
    }
    assert_fixture_is_sanitized(FIXTURE);
}

#[test]
fn sanitized_real_claude_2_1_292_fixtures_replay_with_consistent_session_and_tool_ids() {
    const TWO_SUBAGENTS: &str =
        include_str!("../../../fixtures/m6/claude-2.1.292-two-background-subagents.jsonl");
    const THREE_SESSIONS: &str = include_str!("../../../fixtures/m6/claude-2.1.292-three-sessions.jsonl");
    const FOREGROUND: &str = include_str!("../../../fixtures/m6/claude-2.1.292-foreground-subagent.jsonl");
    const RESUME: &str = include_str!("../../../fixtures/m6/claude-2.1.292-resume.jsonl");
    const CLEAR: &str = include_str!("../../../fixtures/m6/claude-2.1.292-clear.jsonl");

    let two_subagents = fixture_events(TWO_SUBAGENTS);
    assert_eq!(
        kinds(&two_subagents),
        [
            Some(Kind::SessionStart),
            Some(Kind::SubagentStart),
            Some(Kind::SubagentStart),
            Some(Kind::ShellStart),
            Some(Kind::ShellStart),
            Some(Kind::ShellEnd),
            Some(Kind::ShellEnd),
            Some(Kind::SubagentStop),
            Some(Kind::SubagentStop),
            Some(Kind::SessionEnd),
        ]
    );
    let two_subagent_ids: BTreeSet<_> = two_subagents
        .iter()
        .filter(|event| event.hook_event_name == "SubagentStart")
        .map(|event| event.agent_id.as_deref().unwrap())
        .collect();
    let two_stopped_ids: BTreeSet<_> = two_subagents
        .iter()
        .filter(|event| event.hook_event_name == "SubagentStop")
        .map(|event| event.agent_id.as_deref().unwrap())
        .collect();
    assert_eq!(two_subagent_ids.len(), 2);
    assert_eq!(two_subagent_ids, two_stopped_ids);
    assert_tool_events_pair(&two_subagents);
    assert_eq!(
        two_subagents
            .iter()
            .map(|event| event.session_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        1
    );
    let shell_starts: BTreeSet<_> = two_subagents
        .iter()
        .filter(|event| claude::journal_kind(event) == Some(Kind::ShellStart))
        .map(|event| {
            (
                event.session_id.as_str(),
                event.agent_id.as_deref().unwrap(),
                event.tool_use_id.as_deref().unwrap(),
            )
        })
        .collect();
    let shell_ends: BTreeSet<_> = two_subagents
        .iter()
        .filter(|event| claude::journal_kind(event) == Some(Kind::ShellEnd))
        .map(|event| {
            (
                event.session_id.as_str(),
                event.agent_id.as_deref().unwrap(),
                event.tool_use_id.as_deref().unwrap(),
            )
        })
        .collect();
    assert_eq!(shell_starts.len(), 2);
    assert_eq!(shell_starts, shell_ends);
    let called_subagent_ids: BTreeSet<_> = two_subagents
        .iter()
        .filter(|event| event.hook_event_name == "PreToolUse")
        .map(|event| event.agent_id.as_deref().unwrap())
        .collect();
    assert_eq!(called_subagent_ids, two_subagent_ids);
    for agent_id in &two_subagent_ids {
        assert_eq!(
            shell_starts
                .iter()
                .filter(|(_, owner, _)| owner == agent_id)
                .count(),
            1
        );
    }
    assert_fixture_is_sanitized(TWO_SUBAGENTS);

    let three_sessions = fixture_events(THREE_SESSIONS);
    assert_eq!(
        kinds(&three_sessions),
        [
            Some(Kind::SessionStart),
            Some(Kind::SessionStart),
            Some(Kind::SessionStart),
            Some(Kind::ShellStart),
            Some(Kind::ShellStart),
            Some(Kind::ShellStart),
            Some(Kind::ShellEnd),
            Some(Kind::ShellEnd),
            Some(Kind::ShellEnd),
            Some(Kind::SessionEnd),
            Some(Kind::SessionEnd),
            Some(Kind::SessionEnd),
        ]
    );
    assert_tool_events_pair(&three_sessions);
    let session_ids: BTreeSet<_> = three_sessions
        .iter()
        .map(|event| event.session_id.as_str())
        .collect();
    assert_eq!(session_ids.len(), 3);
    for session_id in &session_ids {
        let events: Vec<_> = three_sessions
            .iter()
            .filter(|event| event.session_id == **session_id)
            .collect();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.hook_event_name == "SessionStart")
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| claude::journal_kind(event) == Some(Kind::ShellStart))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| claude::journal_kind(event) == Some(Kind::ShellEnd))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.hook_event_name == "SessionEnd")
                .count(),
            1
        );
    }
    assert_fixture_is_sanitized(THREE_SESSIONS);

    let foreground = fixture_events(FOREGROUND);
    assert_eq!(
        kinds(&foreground),
        [
            Some(Kind::SessionStart),
            Some(Kind::SubagentStart),
            Some(Kind::ShellStart),
            Some(Kind::ShellEnd),
            Some(Kind::SubagentStop),
            Some(Kind::SessionEnd),
        ]
    );
    assert_eq!(
        foreground
            .iter()
            .filter(|event| event.hook_event_name == "SubagentStart")
            .count(),
        1
    );
    assert_tool_events_pair(&foreground);
    let foreground_session_ids: BTreeSet<_> =
        foreground.iter().map(|event| event.session_id.as_str()).collect();
    assert_eq!(foreground_session_ids.len(), 1);
    let foreground_agent_id = foreground
        .iter()
        .find(|event| event.hook_event_name == "SubagentStart")
        .unwrap()
        .agent_id
        .as_deref()
        .unwrap();
    assert!(foreground.iter().all(|event| {
        event
            .agent_id
            .as_deref()
            .is_none_or(|agent_id| agent_id == foreground_agent_id)
    }));
    assert_eq!(
        foreground
            .iter()
            .find(|event| event.hook_event_name == "SubagentStop")
            .unwrap()
            .agent_id
            .as_deref(),
        Some(foreground_agent_id)
    );
    assert_fixture_is_sanitized(FOREGROUND);

    let resume = fixture_events(RESUME);
    assert_eq!(
        kinds(&resume),
        [
            Some(Kind::SessionStart),
            Some(Kind::ShellStart),
            Some(Kind::ShellEnd),
            Some(Kind::SessionEnd),
        ]
    );
    assert_tool_events_pair(&resume);
    let resume_values: Vec<serde_json::Value> = RESUME
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(resume_values[0]["source"], "resume");
    assert_eq!(resume[0].session_id, resume[1].session_id);
    assert_eq!(resume[0].session_id, two_subagents[0].session_id);
    assert_fixture_is_sanitized(RESUME);

    let clear = fixture_events(CLEAR);
    assert_eq!(
        kinds(&clear),
        [
            Some(Kind::SessionStart),
            Some(Kind::SessionEnd),
            Some(Kind::SessionStart),
            Some(Kind::SessionEnd),
        ]
    );
    let clear_values: Vec<serde_json::Value> = CLEAR
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(clear_values[0]["source"], "resume");
    assert_eq!(clear_values[1]["reason"], "clear");
    assert_eq!(clear_values[2]["source"], "clear");
    assert_eq!(clear_values[3]["reason"], "other");
    assert_eq!(clear[0].session_id, clear[1].session_id);
    assert_eq!(clear[2].session_id, clear[3].session_id);
    assert_eq!(clear[0].session_id, two_subagents[0].session_id);
    assert_ne!(clear[0].session_id, clear[2].session_id);
    assert_fixture_is_sanitized(CLEAR);
}

#[test]
fn the_payload_seeds_reach_every_outcome_of_the_parser() {
    let outcomes: Vec<_> = seeds()
        .iter()
        .map(|(_, data)| claude::parse_event(&data[..]))
        .collect();
    let kinds: Vec<Option<Kind>> = outcomes
        .iter()
        .filter_map(|outcome| outcome.as_ref().ok())
        .map(claude::journal_kind)
        .collect();
    for kind in [
        Kind::SessionStart,
        Kind::SessionEnd,
        Kind::SubagentStart,
        Kind::SubagentStop,
        Kind::ShellStart,
        Kind::ShellEnd,
    ] {
        assert!(kinds.contains(&Some(kind)), "no seed maps to {kind:?}");
    }
    assert!(kinds.contains(&None));
    assert!(
        outcomes
            .iter()
            .any(|outcome| matches!(outcome, Err(EventError::FieldTooLong)))
    );
    assert!(
        outcomes
            .iter()
            .any(|outcome| matches!(outcome, Err(EventError::Json(_))))
    );
    assert!(outcomes.iter().flatten().any(|event| event.cwd.is_some()));
    assert!(outcomes.iter().flatten().any(|event| event.agent_id.is_some()));
}

#[test]
fn a_top_level_string_over_the_leading_budget_is_too_long_and_not_a_json_error() {
    let (_, data) = seeds()
        .into_iter()
        .find(|(name, _)| name == "top-level-string-long")
        .unwrap();
    assert!(matches!(
        claude::parse_event(&data[..]),
        Err(EventError::FieldTooLong)
    ));
}
