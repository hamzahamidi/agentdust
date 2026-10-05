use std::io::{self, BufRead, BufReader, Read};

use serde::Deserialize;
use serde::de::IntoDeserializer;
use serde::de::value::{Error as ValueError, StrDeserializer};

use super::{JournalError, Kind, ReadReport, Record, SCHEMA_VERSION};

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
                    finish(&segment, length, Some(rest[at]), &mut each);
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
    finish(&segment, length, None, &mut each);
    Ok(())
}

fn append(segment: &mut Vec<u8>, length: &mut usize, piece: &[u8]) {
    let room = (SEGMENT_LIMIT + 1).saturating_sub(segment.len());
    segment.extend_from_slice(&piece[..piece.len().min(room)]);
    *length += piece.len();
}

fn finish(segment: &[u8], length: usize, terminator: Option<u8>, each: &mut impl FnMut(&[u8], Class)) {
    if length == 0 {
        return;
    }
    let class = match terminator {
        Some(LF) if length > SEGMENT_LIMIT => classify_overlong(segment),
        Some(LF) => classify(segment),
        Some(_) => Class::Torn { tail: false },
        None => Class::Torn { tail: true },
    };
    each(segment, class);
}

fn classify(raw: &[u8]) -> Class {
    if raw.first() != Some(&b'{') {
        return Class::Malformed;
    }
    match serde_json::from_slice::<Record>(raw) {
        Ok(record) if record.v == SCHEMA_VERSION => Class::Record(Box::new(record)),
        Ok(record) => other_version(u64::from(record.v)),
        Err(_) => classify_unreadable(raw),
    }
}

fn classify_unreadable(raw: &[u8]) -> Class {
    let Ok(VersionOnly { v }) = serde_json::from_slice(raw) else {
        return Class::Malformed;
    };
    if v != u64::from(SCHEMA_VERSION) {
        return other_version(v);
    }
    match serde_json::from_slice::<KindOnly>(raw) {
        Ok(KindOnly { kind: Some(kind) }) if !is_known_kind(&kind) => Class::UnknownKind,
        _ => Class::Malformed,
    }
}

fn classify_overlong(prefix: &[u8]) -> Class {
    match sniff_version(prefix) {
        Some(v) => other_version(v),
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

fn other_version(v: u64) -> Class {
    if v > u64::from(SCHEMA_VERSION) {
        Class::NewerVersion
    } else {
        Class::Malformed
    }
}

fn is_known_kind(name: &str) -> bool {
    let deserializer: StrDeserializer<ValueError> = name.into_deserializer();
    Kind::deserialize(deserializer).is_ok()
}
