mod journal_support;

use std::io::{self, Read};

use agentdust_core::journal::{Agent, CwdKey, ExeBase, Kind, ReadReport, Record, decode, encode};
use journal_support::frame;
use proptest::prelude::*;
use proptest::sample::select;
use serde_json::json;

const KINDS: [Kind; 6] = [
    Kind::SessionStart,
    Kind::SessionEnd,
    Kind::ShellStart,
    Kind::ShellEnd,
    Kind::Sample,
    Kind::ServerStart,
];
const KNOWN_KIND_NAMES: [&str; 6] = [
    "session_start",
    "session_end",
    "shell_start",
    "shell_end",
    "sample",
    "server_start",
];

fn arb_record() -> impl Strategy<Value = Record> {
    (
        (
            select(KINDS.to_vec()),
            select(vec![Agent::Claude, Agent::Codex, Agent::Cursor]),
        ),
        (
            ".{0,40}",
            proptest::option::of(".{0,20}"),
            proptest::option::of(".{0,20}"),
        ),
        (any::<u64>(), any::<u64>(), ".{0,40}"),
        (
            proptest::option::of("[0-9a-f]{1,64}"),
            proptest::option::of("[a-zA-Z0-9 ._()-]{0,64}"),
        ),
    )
        .prop_map(
            |(
                (kind, agent),
                (session_id, subagent_id, tool_use_id),
                (wall_ts, mono_ts, boot),
                (cwd, exe),
            )| {
                Record {
                    v: 1,
                    kind,
                    agent,
                    session_id,
                    subagent_id,
                    tool_use_id,
                    wall_ts,
                    mono_ts,
                    boot,
                    cwd_key: cwd.map(|key| CwdKey::try_from(key).unwrap()),
                    exe_base: exe.map(|name| ExeBase::try_from(name).unwrap()),
                }
            },
        )
}

#[derive(Debug, Clone)]
enum Planned {
    Valid(Record),
    Newer(Record, u64),
    UnknownKind(Record, String),
    Garbage(Vec<u8>),
}

fn arb_planned() -> impl Strategy<Value = Planned> {
    prop_oneof![
        4 => arb_record().prop_map(Planned::Valid),
        2 => (arb_record(), 2..=u64::MAX).prop_map(|(record, v)| Planned::Newer(record, v)),
        2 => (arb_record(), "[a-z_]{1,12}")
            .prop_filter("a known kind", |(_, kind)| !KNOWN_KIND_NAMES.contains(&kind.as_str()))
            .prop_map(|(record, kind)| Planned::UnknownKind(record, kind)),
        2 => proptest::collection::vec(any::<u8>(), 1..200)
            .prop_map(|bytes| bytes.into_iter().filter(|b| !matches!(*b, b'\n' | 0x1e | b'{')).collect::<Vec<u8>>())
            .prop_filter("not empty", |bytes| !bytes.is_empty())
            .prop_map(Planned::Garbage),
    ]
}

fn render(planned: &Planned) -> Vec<u8> {
    let mut bytes = vec![0x1e];
    bytes.extend(match planned {
        Planned::Valid(record) => serde_json::to_vec(record).unwrap(),
        Planned::Newer(record, v) => {
            let mut value = serde_json::to_value(record).unwrap();
            value["v"] = json!(v);
            serde_json::to_vec(&value).unwrap()
        }
        Planned::UnknownKind(record, kind) => {
            let mut value = serde_json::to_value(record).unwrap();
            value["kind"] = json!(kind);
            serde_json::to_vec(&value).unwrap()
        }
        Planned::Garbage(bytes) => bytes.clone(),
    });
    bytes.push(b'\n');
    bytes
}

fn non_empty_segments(bytes: &[u8]) -> usize {
    bytes
        .split(|byte| *byte == 0x1e || *byte == b'\n')
        .filter(|segment| !segment.is_empty())
        .count()
}

struct Dribble<'a> {
    rest: &'a [u8],
    chunk: usize,
}

impl Read for Dribble<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.chunk.min(buf.len()).min(self.rest.len());
        buf[..n].copy_from_slice(&self.rest[..n]);
        self.rest = &self.rest[n..];
        Ok(n)
    }
}

fn accounted(report: &ReadReport) -> usize {
    report.records.len() + report.skipped_lines()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn every_segment_of_arbitrary_bytes_lands_in_exactly_one_counter(
        bytes in proptest::collection::vec(
            prop_oneof![8 => any::<u8>(), 1 => Just(b'\n'), 1 => Just(0x1e)],
            0..4096,
        )
    ) {
        let report = decode(&bytes[..]).unwrap();
        prop_assert_eq!(accounted(&report), non_empty_segments(&bytes));
        prop_assert_eq!(report.unsupported_version, report.newer_version_lines > 0);
        prop_assert!(report.torn_frames >= usize::from(report.truncated_last_line));
    }

    #[test]
    fn a_planned_journal_decodes_to_exactly_its_valid_frames_and_counts_the_rest(
        planned in proptest::collection::vec(arb_planned(), 0..24),
        tail in proptest::option::of(
            proptest::collection::vec(
                any::<u8>().prop_filter("no separator", |b| *b != b'\n' && *b != 0x1e),
                1..64,
            )
        ),
    ) {
        let mut bytes: Vec<u8> = planned.iter().flat_map(render).collect();
        if let Some(tail) = &tail {
            bytes.push(0x1e);
            bytes.extend_from_slice(tail);
        }
        let report = decode(&bytes[..]).unwrap();
        let valid: Vec<&Record> = planned
            .iter()
            .filter_map(|p| if let Planned::Valid(record) = p { Some(record) } else { None })
            .collect();
        prop_assert_eq!(report.records.iter().collect::<Vec<_>>(), valid);
        let newer = planned.iter().filter(|p| matches!(p, Planned::Newer(..))).count();
        let unknown = planned.iter().filter(|p| matches!(p, Planned::UnknownKind(..))).count();
        let garbage = planned.iter().filter(|p| matches!(p, Planned::Garbage(..))).count();
        prop_assert_eq!(report.newer_version_lines, newer);
        prop_assert_eq!(report.unknown_kind_lines, unknown);
        prop_assert_eq!(report.malformed_lines, garbage);
        prop_assert_eq!(report.unsupported_version, newer > 0);
        prop_assert_eq!(report.truncated_last_line, tail.is_some());
        prop_assert_eq!(report.torn_frames, usize::from(tail.is_some()));
        prop_assert_eq!(accounted(&report), non_empty_segments(&bytes));
    }

    #[test]
    fn a_cut_frame_never_costs_the_record_before_or_after_it(
        before in arb_record(),
        cut in arb_record(),
        after in arb_record(),
        keep in 0.0f64..1.0,
    ) {
        let whole = encode(&cut).unwrap();
        let kept = ((whole.len() - 1) as f64 * keep) as usize;
        let mut bytes = frame(&before);
        bytes.extend_from_slice(&whole[..kept]);
        bytes.extend(frame(&after));
        let report = decode(&bytes[..]).unwrap();
        prop_assert_eq!(report.records, vec![before, after]);
        prop_assert_eq!(report.malformed_lines, 0);
        prop_assert!(report.torn_frames <= 1);
        prop_assert!(!report.truncated_last_line);
    }

    #[test]
    fn the_report_does_not_depend_on_how_the_bytes_arrive(
        planned in proptest::collection::vec(arb_planned(), 0..12),
        chunk in 1usize..=9,
    ) {
        let bytes: Vec<u8> = planned.iter().flat_map(render).collect();
        let whole = decode(&bytes[..]).unwrap();
        let dribbled = decode(Dribble { rest: &bytes, chunk }).unwrap();
        prop_assert_eq!(whole, dribbled);
    }

    #[test]
    fn a_newer_version_line_never_yields_a_record(record in arb_record(), v in 2..=u64::MAX) {
        let bytes = render(&Planned::Newer(record, v));
        let report = decode(&bytes[..]).unwrap();
        prop_assert!(report.records.is_empty());
        prop_assert_eq!(report.newer_version_lines, 1);
        prop_assert!(report.unsupported_version);
    }

    #[test]
    fn a_valid_record_always_decodes_to_itself(record in arb_record()) {
        let bytes = encode(&record).unwrap();
        let report = decode(&bytes[..]).unwrap();
        prop_assert_eq!(accounted(&report), 1);
        prop_assert_eq!(report.records, vec![record]);
    }
}
