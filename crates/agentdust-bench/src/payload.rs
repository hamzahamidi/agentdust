use agentdust_core::journal::{Agent, Kind, Record, SCHEMA_VERSION};

pub const BOOT: &str = "bench";
const MARKER_PREFIX: &str = "expired-";

const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

pub fn wall(record: &Record) -> u64 {
    record.wall_ts
}

pub fn mono(record: &Record) -> u64 {
    record.mono_ts
}

fn base(session_id: String, kind: Kind, wall_ms: u64, mono_ts: u64) -> Record {
    Record {
        v: SCHEMA_VERSION,
        kind,
        agent: Agent::Claude,
        session_id,
        subagent_id: None,
        tool_use_id: None,
        wall_ts: wall_ms,
        mono_ts,
        boot: BOOT.to_owned(),
        cwd_key: None,
        exe_base: None,
    }
}

pub fn record_with(writer: u32, seq: u64, size: usize, wall_ms: u64, mono_ts: u64) -> Record {
    let mut record = base(format!("w{writer}-{seq}"), Kind::ShellStart, wall_ms, mono_ts);
    let floor = line_len(&record);
    if size > floor {
        record.session_id.push('-');
        record.session_id.extend(pad(writer, seq, size - floor - 1));
    }
    record
}

pub fn marker(n: u64, wall_ms: u64, mono_ts: u64) -> Record {
    base(format!("{MARKER_PREFIX}{n}"), Kind::ShellEnd, wall_ms, mono_ts)
}

pub fn is_marker(record: &Record) -> bool {
    record.session_id.starts_with(MARKER_PREFIX)
}

pub fn line_len(record: &Record) -> usize {
    serde_json::to_vec(record).map_or(0, |bytes| bytes.len() + 1)
}

pub fn identify(record: &Record, size: usize) -> Option<(u32, u64)> {
    let rest = record.session_id.strip_prefix('w')?;
    let mut parts = rest.splitn(3, '-');
    let writer = parts.next()?.parse().ok()?;
    let seq = parts.next()?.parse().ok()?;
    let expected = record_with(writer, seq, size, wall(record), mono(record));
    (expected == *record).then_some((writer, seq))
}

fn pad(writer: u32, seq: u64, len: usize) -> impl Iterator<Item = char> {
    let start = u64::from(writer) * 7 + seq;
    (0..len as u64).map(move |i| char::from(ALPHABET[((start + i) % ALPHABET.len() as u64) as usize]))
}
