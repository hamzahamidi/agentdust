use std::collections::BTreeSet;

use agentdust_core::journal::{
    AGENT_IDENTITY_KEYS, Agent, AgentIdentity, CwdKey, ExeBase, Kind, RECORD_KEYS, Record, SCHEMA_VERSION,
    SessionTagKey,
};

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn identity() -> AgentIdentity {
    AgentIdentity::new(
        4242,
        1_800_000_000_000_000,
        501,
        Some(ExeBase::try_from("claude").unwrap()),
    )
    .unwrap()
}

fn every_field_set() -> Record {
    Record {
        v: SCHEMA_VERSION,
        kind: Kind::ShellStart,
        agent: Agent::Claude,
        session_id: "s1".to_owned(),
        subagent_id: Some("agent-7".to_owned()),
        tool_use_id: Some("toolu_1".to_owned()),
        wall_ts: 1,
        mono_ts: 2,
        boot: "b".to_owned(),
        agent_identity: Some(identity()),
        session_tag_key: Some(SessionTagKey::try_from(KEY).unwrap()),
        cwd_key: Some(CwdKey::try_from(KEY).unwrap()),
        exe_base: Some(ExeBase::try_from("node").unwrap()),
    }
}

fn fields_of(record: &Record) -> usize {
    let Record {
        v: _,
        kind: _,
        agent: _,
        session_id: _,
        subagent_id: _,
        tool_use_id: _,
        wall_ts: _,
        mono_ts: _,
        boot: _,
        cwd_key: _,
        agent_identity: _,
        session_tag_key: _,
        exe_base: _,
    } = record;
    13
}

fn keys_of(record: &Record) -> BTreeSet<String> {
    let value = serde_json::to_value(record).unwrap();
    value.as_object().unwrap().keys().cloned().collect()
}

#[test]
fn the_key_list_is_exactly_the_keys_of_a_record_with_every_field_set() {
    let listed: BTreeSet<String> = RECORD_KEYS.iter().map(|key| (*key).to_owned()).collect();
    assert_eq!(keys_of(&every_field_set()), listed);
}

#[test]
fn the_key_list_names_every_field_once() {
    let listed: BTreeSet<&str> = RECORD_KEYS.iter().copied().collect();
    assert_eq!(listed.len(), RECORD_KEYS.len());
    assert_eq!(RECORD_KEYS.len(), fields_of(&every_field_set()));
}

#[test]
fn the_keys_are_listed_in_the_order_they_are_written() {
    let line = serde_json::to_string(&every_field_set()).unwrap();
    let mut at = 0;
    for key in RECORD_KEYS {
        let found = line[at..]
            .find(&format!("\"{key}\":"))
            .unwrap_or_else(|| panic!("{key} out of order"));
        at += found;
    }
}

#[test]
fn the_first_key_is_the_version_so_a_reader_can_tell_a_newer_schema_from_its_first_bytes() {
    assert_eq!(RECORD_KEYS[0], "v");
    let line = serde_json::to_string(&every_field_set()).unwrap();
    assert!(line.starts_with("{\"v\":"));
}

#[test]
fn a_record_with_no_optional_field_writes_only_listed_keys() {
    let record = Record {
        subagent_id: None,
        tool_use_id: None,
        agent_identity: None,
        session_tag_key: None,
        cwd_key: None,
        exe_base: None,
        ..every_field_set()
    };
    let listed: BTreeSet<String> = RECORD_KEYS.iter().map(|key| (*key).to_owned()).collect();
    assert!(keys_of(&record).is_subset(&listed));
    assert_eq!(keys_of(&record).len(), 7);
}

#[test]
fn the_keys_of_the_nested_identity_are_listed_in_the_order_they_are_written() {
    let value = serde_json::to_value(identity()).unwrap();
    let written: Vec<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
    let mut listed: Vec<&str> = AGENT_IDENTITY_KEYS.to_vec();
    listed.sort_unstable();
    let mut sorted = written.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, listed);
    let line = serde_json::to_string(&identity()).unwrap();
    let mut at = 0;
    for key in AGENT_IDENTITY_KEYS {
        let found = line[at..]
            .find(&format!("\"{key}\":"))
            .unwrap_or_else(|| panic!("{key} out of order"));
        at += found;
    }
}

#[test]
fn an_identity_without_an_executable_name_writes_only_three_keys() {
    let bare = AgentIdentity::new(1, 2, 3, None).unwrap();
    let value = serde_json::to_value(bare).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 3);
}
