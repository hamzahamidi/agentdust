#![no_main]

use agentdust_core::journal::{self, Agent, Kind, Record, SCHEMA_VERSION};
use agentdust_fuzz::bounds::{Dribble, accounted, assert_bounded, retained_bytes, segment_count};
use agentdust_fuzz::meter::{self, Meter};
use libfuzzer_sys::fuzz_target;

#[global_allocator]
static ALLOCATOR: Meter = Meter;

const ROUND_TRIPPED: usize = 32;

fn next_record() -> Record {
    Record {
        v: SCHEMA_VERSION,
        kind: Kind::ShellEnd,
        agent: Agent::Claude,
        session_id: "the record after the input".to_owned(),
        subagent_id: None,
        tool_use_id: Some("toolu_next".to_owned()),
        wall_ts: 1,
        mono_ts: 2,
        boot: "boot".to_owned(),
        cwd_key: None,
        agent_identity: None,
        session_tag_key: None,
        exe_base: None,
    }
}

fuzz_target!(|data: &[u8]| {
    let (report, peak) = meter::measure(|| journal::decode(data));
    let report = report.expect("a slice never fails to read");
    assert_bounded("decode", peak, data.len(), retained_bytes(&report));
    assert_eq!(accounted(&report), segment_count(data));
    assert_eq!(report.unsupported_version, report.newer_version_lines > 0);
    assert!(report.torn_frames >= usize::from(report.truncated_last_line));

    let chunk = 1 + data.len() * 7919 % 70_000;
    let dribbled = journal::decode(Dribble { rest: data, chunk }).expect("a slice never fails to read");
    assert_eq!(dribbled, report);

    for record in report.records.iter().take(ROUND_TRIPPED) {
        let frame = journal::encode(record).expect("a decoded record can be written");
        let back = journal::decode(&frame[..]).expect("a slice never fails to read");
        assert_eq!(back.records, std::slice::from_ref(record));
        assert_eq!(back.skipped_lines(), 0);
    }

    let next = next_record();
    let frame = journal::encode(&next).expect("the next record fits");
    let mut after = data.to_vec();
    after.extend_from_slice(&frame);
    let resynced = journal::decode(&after[..]).expect("a slice never fails to read");
    let mut expected = report.records.clone();
    expected.push(next.clone());
    assert_eq!(
        resynced.records, expected,
        "the record after any input is recovered"
    );
    assert_eq!(resynced.malformed_lines, report.malformed_lines);
    assert_eq!(resynced.newer_version_lines, report.newer_version_lines);
    assert_eq!(resynced.unknown_kind_lines, report.unknown_kind_lines);
    assert_eq!(resynced.torn_frames, report.torn_frames);
    assert!(!resynced.truncated_last_line);

    let mut before = frame.clone();
    before.extend_from_slice(data);
    let led = journal::decode(&before[..]).expect("a slice never fails to read");
    assert_eq!(
        led.records.first(),
        Some(&next),
        "the record before any input is kept"
    );
});
