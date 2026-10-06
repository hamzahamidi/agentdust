use std::io::{self, BufRead, BufReader, Read};

use serde::Deserialize;
use serde::de::IntoDeserializer;
use serde::de::value::{Error as ValueError, StrDeserializer};

use super::{JournalError, Kind, MIN_READABLE_SCHEMA_VERSION, ReadReport, Record, SCHEMA_VERSION};

pub const RS: u8 = 0x1E;
pub const MAX_FRAME_LEN: usize = 65_536;
const LF: u8 = b'\n';
const SEGMENT_LIMIT: usize = MAX_FRAME_LEN - 2;
const VERSION_PREFIX: &[u8] = b"{\"v\":";

#[derive(Debug)]
pub enum Class {
    Record(Box<Record>),
    Malformed,
    Torn { tail: bool },
    NewerVersion,
    UnknownKind,
}

#[derive(Deserialize)]
struct VersionOnly {
    v: u64,
}

#[derive(Deserialize)]
struct KindOnly {
    kind: Option<String>,
}

pub fn encode(record: &Record) -> Result<Vec<u8>, JournalError> {
    if record.v != SCHEMA_VERSION {
        return Err(JournalError::WrongVersion { found: record.v });
    }
    let json = serde_json::to_vec(record)?;
    let len = json.len() + 2;
    if len > MAX_FRAME_LEN {
        return Err(JournalError::TooLarge {
            len,
            max: MAX_FRAME_LEN,
        });
    }
    let mut frame = Vec::with_capacity(len);
    frame.push(RS);
    frame.extend_from_slice(&json);
    frame.push(LF);
    Ok(frame)
}

pub fn decode(reader: impl Read) -> io::Result<ReadReport> {
    let mut report = ReadReport::default();
    scan(reader, |_, class| match class {
        Class::Record(record) => report.records.push(*record),
        Class::Malformed => report.malformed_lines += 1,
        Class::Torn { tail } => {
            report.torn_frames += 1;
            report.truncated_last_line |= tail;
        }
        Class::NewerVersion => {
            report.newer_version_lines += 1;
            report.unsupported_version = true;
        }
        Class::UnknownKind => report.unknown_kind_lines += 1,
    })?;
    Ok(report)
}

pub fn scan(reader: impl Read, mut each: impl FnMut(&[u8], Class)) -> io::Result<()> {
    scan_as(reader, SCHEMA_VERSION, &mut each)
}

fn scan_as(reader: impl Read, supported_version: u32, each: &mut impl FnMut(&[u8], Class)) -> io::Result<()> {
    let mut reader = BufReader::with_capacity(64 * 1024, reader);
    let mut segment: Vec<u8> = Vec::new();
    let mut length = 0usize;
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            break;
        }
        let mut rest = chunk;
        while !rest.is_empty() {
            match rest.iter().position(|byte| *byte == RS || *byte == LF) {
                Some(at) => {
                    append(&mut segment, &mut length, &rest[..at]);
                    finish(&segment, length, Some(rest[at]), supported_version, each);
                    segment.clear();
                    length = 0;
                    rest = &rest[at + 1..];
                }
                None => {
                    append(&mut segment, &mut length, rest);
                    rest = &[];
                }
            }
        }
        let consumed = chunk.len();
        reader.consume(consumed);
    }
    finish(&segment, length, None, supported_version, each);
    Ok(())
}

fn append(segment: &mut Vec<u8>, length: &mut usize, piece: &[u8]) {
    let room = (SEGMENT_LIMIT + 1).saturating_sub(segment.len());
    segment.extend_from_slice(&piece[..piece.len().min(room)]);
    *length += piece.len();
}

fn finish(
    segment: &[u8],
    length: usize,
    terminator: Option<u8>,
    supported_version: u32,
    each: &mut impl FnMut(&[u8], Class),
) {
    if length == 0 {
        return;
    }
    let class = match terminator {
        Some(LF) if length > SEGMENT_LIMIT => classify_overlong(segment, supported_version),
        Some(LF) => classify(segment, supported_version),
        Some(_) => Class::Torn { tail: false },
        None => Class::Torn { tail: true },
    };
    each(segment, class);
}

fn classify(raw: &[u8], supported_version: u32) -> Class {
    if raw.first() != Some(&b'{') {
        return Class::Malformed;
    }
    match serde_json::from_slice::<Record>(raw) {
        Ok(record) if record.v >= MIN_READABLE_SCHEMA_VERSION && record.v <= supported_version => {
            Class::Record(Box::new(record))
        }
        Ok(record) => other_version(u64::from(record.v), supported_version),
        Err(_) => classify_unreadable(raw, supported_version),
    }
}

fn classify_unreadable(raw: &[u8], supported_version: u32) -> Class {
    let Ok(VersionOnly { v }) = serde_json::from_slice(raw) else {
        return Class::Malformed;
    };
    if v < u64::from(MIN_READABLE_SCHEMA_VERSION) || v > u64::from(supported_version) {
        return other_version(v, supported_version);
    }
    match serde_json::from_slice::<KindOnly>(raw) {
        Ok(KindOnly { kind: Some(kind) }) if !is_known_kind(&kind) => Class::UnknownKind,
        _ => Class::Malformed,
    }
}

fn classify_overlong(prefix: &[u8], supported_version: u32) -> Class {
    match sniff_version(prefix) {
        Some(v) => other_version(v, supported_version),
        None => Class::Malformed,
    }
}

fn sniff_version(prefix: &[u8]) -> Option<u64> {
    let rest = prefix.strip_prefix(VERSION_PREFIX)?;
    let digits = rest.iter().take_while(|byte| byte.is_ascii_digit()).count();
    let delimiter = *rest.get(digits)?;
    if digits == 0
        || digits > 20
        || (digits > 1 && rest[0] == b'0')
        || (delimiter != b',' && delimiter != b'}')
    {
        return None;
    }
    std::str::from_utf8(&rest[..digits]).ok()?.parse().ok()
}

fn other_version(v: u64, supported_version: u32) -> Class {
    if v > u64::from(supported_version) {
        Class::NewerVersion
    } else {
        Class::Malformed
    }
}

fn is_known_kind(name: &str) -> bool {
    let deserializer: StrDeserializer<ValueError> = name.into_deserializer();
    Kind::deserialize(deserializer).is_ok()
}

#[cfg(test)]
mod compatibility_tests {
    use super::{Class, LF, RS, decode, scan_as};
    use crate::journal::{Agent, AgentIdentity, Kind, Record, SessionTagKey};
    use std::io::Cursor;

    fn record(kind: Kind, version: u32, pid: i32, tag: Option<SessionTagKey>) -> Record {
        Record {
            v: version,
            kind,
            agent: Agent::Claude,
            session_id: "session".to_owned(),
            subagent_id: None,
            agent_identity: Some(AgentIdentity::new(pid, pid as u64, 501, None).unwrap()),
            tool_use_id: None,
            wall_ts: pid as u64,
            mono_ts: pid as u64,
            boot: "boot".to_owned(),
            session_tag_key: tag,
            cwd_key: None,
            exe_base: None,
        }
    }

    fn frame(record: &Record) -> Vec<u8> {
        let mut frame = vec![RS];
        frame.extend(serde_json::to_vec(record).unwrap());
        frame.push(LF);
        frame
    }

    #[test]
    fn a_v1_reader_marks_m6_data_unsupported_instead_of_dropping_a_live_owner() {
        let key = SessionTagKey::try_from("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
            .unwrap();
        let mut bytes = frame(&record(Kind::SessionStart, 1, 10, Some(key)));
        bytes.extend(frame(&record(Kind::SessionEnd, 1, 10, None)));
        bytes.extend(frame(&record(Kind::SubagentStart, 2, 20, None)));

        let mut unsupported = false;
        let mut records = Vec::new();
        scan_as(Cursor::new(bytes), 1, &mut |_, class| match class {
            Class::Record(record) => records.push(*record),
            Class::NewerVersion => unsupported = true,
            _ => {}
        })
        .unwrap();

        assert!(unsupported);
        assert_eq!(records.len(), 2);
        assert!(records.iter().all(|record| record.v == 1));
    }

    #[test]
    fn the_current_reader_keeps_existing_v1_records() {
        let bytes = frame(&record(Kind::SessionStart, 1, 10, None));
        let report = decode(Cursor::new(bytes)).unwrap();

        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].v, 1);
        assert!(!report.unsupported_version);
    }
}
