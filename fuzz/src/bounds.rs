use std::collections::HashMap;
use std::io::{self, Read};
use std::mem::size_of;

use agentdust_core::journal::{MAX_FRAME_LEN, ReadReport, Record};

pub const FIXED_BYTES: usize = 6 * MAX_FRAME_LEN;
pub const PER_INPUT_BYTE: usize = 8;
pub const PER_RETAINED_BYTE: usize = 6;
pub const PAYLOAD_PEAK_BYTES: usize = 128 * 1024;
pub const FILE_SEPARATOR: u8 = 0x1c;
pub const MAX_FILES: usize = 4;

pub fn retained_bytes(report: &ReadReport) -> usize {
    report
        .records
        .iter()
        .map(|record| {
            size_of::<Record>()
                + record.session_id.len()
                + record.subagent_id.as_ref().map_or(0, String::len)
                + record.tool_use_id.as_ref().map_or(0, String::len)
                + record.boot.len()
                + record.cwd_key.as_ref().map_or(0, |key| key.as_str().len())
                + record.exe_base.as_ref().map_or(0, |name| name.as_str().len())
                + record.session_tag_key.as_ref().map_or(0, |key| key.as_str().len())
                + record
                    .agent_identity
                    .as_ref()
                    .and_then(|identity| identity.exe_base())
                    .map_or(0, |name| name.as_str().len())
        })
        .sum()
}

pub fn assert_bounded(what: &str, peak: usize, input_len: usize, retained: usize) {
    let by_input = FIXED_BYTES + PER_INPUT_BYTE * input_len;
    assert!(
        peak <= by_input,
        "{what}: peak {peak} over {by_input} for {input_len} input bytes"
    );
    let by_retained = FIXED_BYTES + PER_RETAINED_BYTE * retained;
    assert!(
        peak <= by_retained,
        "{what}: peak {peak} over {by_retained} for {retained} retained bytes"
    );
}

pub fn segment_count(bytes: &[u8]) -> usize {
    bytes
        .split(|byte| *byte == 0x1e || *byte == b'\n')
        .filter(|segment| !segment.is_empty())
        .count()
}

pub fn accounted(report: &ReadReport) -> usize {
    report.records.len() + report.skipped_lines()
}

pub fn order_violation(records: &[Record]) -> Option<usize> {
    let mut first_wall: HashMap<&str, u64> = HashMap::new();
    for record in records {
        first_wall
            .entry(record.boot.as_str())
            .and_modify(|first| *first = (*first).min(record.wall_ts))
            .or_insert(record.wall_ts);
    }
    let mut boots: Vec<(u64, &str)> = first_wall.into_iter().map(|(boot, wall)| (wall, boot)).collect();
    boots.sort_unstable();
    let rank: HashMap<&str, usize> = boots
        .iter()
        .enumerate()
        .map(|(position, (_, boot))| (*boot, position))
        .collect();
    let key = |record: &Record| (rank[record.boot.as_str()], record.mono_ts, record.wall_ts);
    records
        .windows(2)
        .position(|pair| (key(&pair[0]), &pair[0]) >= (key(&pair[1]), &pair[1]))
}

pub struct Dribble<'a> {
    pub rest: &'a [u8],
    pub chunk: usize,
}

impl Read for Dribble<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.chunk.min(buf.len()).min(self.rest.len());
        buf[..n].copy_from_slice(&self.rest[..n]);
        self.rest = &self.rest[n..];
        Ok(n)
    }
}

pub fn stretch(data: &[u8]) -> Option<Vec<u8>> {
    let (&size, rest) = data.split_first()?;
    let (&which, rest) = rest.split_first()?;
    let quotes: Vec<usize> = rest
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'"')
        .map(|(at, _)| at)
        .collect();
    let first = usize::from(which) % 4 * 2;
    let (open, close) = (*quotes.get(first)?, *quotes.get(first + 1)?);
    let inner = &rest[open + 1..close];
    if inner.is_empty() {
        return None;
    }
    let copies = (1usize << (size % 21)) / inner.len() + 1;
    let mut out = Vec::with_capacity(rest.len() + inner.len() * copies);
    out.extend_from_slice(&rest[..=open]);
    for _ in 0..copies {
        out.extend_from_slice(inner);
    }
    out.extend_from_slice(&rest[close..]);
    Some(out)
}
