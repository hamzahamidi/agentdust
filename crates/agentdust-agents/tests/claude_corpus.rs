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
