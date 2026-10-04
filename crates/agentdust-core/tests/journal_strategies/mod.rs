#![allow(dead_code)]

use agentdust_core::journal::{Agent, CwdKey, ExeBase, Kind, Record, SCHEMA_VERSION};
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::sample::select;

pub const KINDS: [Kind; 6] = [
    Kind::SessionStart,
    Kind::SessionEnd,
    Kind::ShellStart,
    Kind::ShellEnd,
    Kind::Sample,
    Kind::ServerStart,
];
pub const AGENTS: [Agent; 3] = [Agent::Claude, Agent::Codex, Agent::Cursor];
pub const NASTY: [char; 26] = [
    '\n', '\r', '\t', '"', '\\', '{', '}', '[', ']', ',', ':', '/', '\0', '\u{7f}', '\u{85}', '\u{1c}',
    '\u{1e}', '\u{2028}', '\u{2029}', '\u{feff}', '\u{fffd}', 'é', '日', '😀', 'a', ' ',
];
pub const EXE_ALPHABET: [char; 14] = [
    'a', 'Z', '0', ' ', '-', '.', '_', 'é', '日', '😀', '(', ')', '\'', '~',
];
pub const CHUNKS: [usize; 13] = [
    1, 2, 3, 7, 4095, 4096, 4097, 8191, 8192, 8193, 65_535, 65_536, 65_537,
];

pub fn text(max_chars: usize) -> impl Strategy<Value = String> {
    prop_oneof![
        1 => vec(any::<char>(), 0..=max_chars).prop_map(String::from_iter),
        2 => vec(select(NASTY.to_vec()), 0..=max_chars).prop_map(String::from_iter),
    ]
}

pub fn exe_base() -> impl Strategy<Value = ExeBase> {
    vec(select(EXE_ALPHABET.to_vec()), 0..=16)
        .prop_map(|chars| ExeBase::try_from(String::from_iter(chars)).unwrap())
}

pub fn arb_record() -> impl Strategy<Value = Record> {
    (
        (select(KINDS.to_vec()), select(AGENTS.to_vec())),
        (
            text(60),
            proptest::option::of(text(30)),
            proptest::option::of(text(30)),
        ),
        (any::<u64>(), any::<u64>(), text(40)),
        (
            proptest::option::of("[0-9a-f]{1,64}"),
            proptest::option::of(exe_base()),
        ),
    )
        .prop_map(
            |(
                (kind, agent),
                (session_id, subagent_id, tool_use_id),
                (wall_ts, mono_ts, boot),
                (cwd_key, exe_base),
            )| Record {
                v: SCHEMA_VERSION,
                kind,
                agent,
                session_id,
                subagent_id,
                tool_use_id,
                wall_ts,
                mono_ts,
                boot,
                cwd_key: cwd_key.map(|key| CwdKey::try_from(key).unwrap()),
                exe_base,
            },
        )
}

pub fn in_append_order(records: Vec<Record>) -> Vec<Record> {
    records
        .into_iter()
        .enumerate()
        .map(|(position, mut record)| {
            record.boot = "boot".to_owned();
            record.wall_ts = 1_800_000_000_000;
            record.mono_ts = position as u64 + 1;
            record
        })
        .collect()
}

pub fn small_record() -> impl Strategy<Value = Record> {
    (
        select(vec!["b1", "b2", "b3"]),
        0u64..4,
        0u64..6,
        select(vec!["s1", "s2"]),
        select(KINDS.to_vec()),
        select(AGENTS.to_vec()),
    )
        .prop_map(|(boot, wall_ts, mono_ts, session, kind, agent)| Record {
            v: SCHEMA_VERSION,
            kind,
            agent,
            session_id: session.to_owned(),
            subagent_id: None,
            tool_use_id: None,
            wall_ts,
            mono_ts,
            boot: boot.to_owned(),
            cwd_key: None,
            exe_base: None,
        })
}
