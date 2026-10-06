mod common;

use agentdust_bench::frame::{Framing, MAX_FRAME_LEN, RS, decode, encode};
use agentdust_bench::payload::record_with;
use agentdust_core::journal;
use common::{named, stamped};

fn samples() -> Vec<journal::Record> {
    let mut full = named("full", 9);
    full.subagent_id = Some("agent-7".to_owned());
    full.tool_use_id = Some("toolu_1".to_owned());
    full.cwd_key = Some("0123456789abcdef".try_into().unwrap());
    full.exe_base = Some("node".try_into().unwrap());
    let mut odd = stamped("a\u{1e}b\nc\u{0}d", "boot-\u{e9}", u64::MAX, u64::MAX);
    odd.kind = journal::Kind::Sample;
    odd.agent = journal::Agent::Cursor;
    vec![
        named("plain", 1),
        full,
        odd,
        record_with(1, 2, 4000, 1_800_000_000_000, 5),
        record_with(0, 0, MAX_FRAME_LEN, 1_800_000_000_000, 7),
    ]
}

#[test]
fn the_benchmark_and_the_journal_frame_a_record_to_the_same_bytes() {
    assert_eq!(MAX_FRAME_LEN, journal::MAX_FRAME_LEN);
    assert_eq!(RS, journal::RS);
    for record in samples() {
        match (encode(&record, Framing::Record), journal::encode(&record)) {
            (Ok(bench), Ok(core)) => assert_eq!(bench, core, "{}", record.session_id.len()),
            (Err(_), Err(_)) => {}
            (bench, core) => panic!("{bench:?} against {core:?}"),
        }
    }
}

#[test]
fn the_benchmark_and_the_journal_decode_the_same_file_to_the_same_records() {
    let mut bytes: Vec<u8> = Vec::new();
    for record in samples() {
        if let Ok(frame) = journal::encode(&record) {
            bytes.extend(frame);
        }
    }
    bytes.extend(b"\x1e{\"v\":3,\"future\":true}\n\x1e[2]\n\x1e{broken\n\x1e{\"v\":1,\"kin");
    let bench = decode(&bytes[..]).unwrap();
    let core = journal::decode(&bytes[..]).unwrap();
    assert_eq!(bench.records, core.records);
    assert_eq!(bench.malformed, core.malformed_lines);
    assert_eq!(bench.torn, core.torn_frames);
    assert_eq!(bench.newer_version, core.newer_version_lines);
    assert_eq!(bench.unknown_kind, core.unknown_kind_lines);
}
