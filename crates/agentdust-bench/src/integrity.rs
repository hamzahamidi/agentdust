use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::payload::{identify, is_marker, mono};
use crate::store::ReadOutcome;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Integrity {
    pub lost: u64,
    pub duplicates: u64,
    pub unacked: u64,
    pub torn: u64,
    pub interleaved: u64,
    pub out_of_order: u64,
    pub mono_inversions: u64,
    pub markers: u64,
}

pub fn check(report: &ReadOutcome, acked: &BTreeSet<(u32, u64)>, size: usize) -> Integrity {
    let mut found = Integrity {
        torn: report.skipped_lines() as u64,
        duplicates: report.duplicates_removed as u64,
        ..Integrity::default()
    };
    let mut seen = BTreeSet::new();
    let mut last_seq: HashMap<u32, u64> = HashMap::new();
    let mut last_mono: Option<u64> = None;
    for record in &report.records {
        if is_marker(record) {
            found.markers += 1;
            continue;
        }
        let Some((writer, seq)) = identify(record, size) else {
            found.interleaved += 1;
            continue;
        };
        if !seen.insert((writer, seq)) {
            found.duplicates += 1;
        } else if !acked.contains(&(writer, seq)) {
            found.unacked += 1;
        }
        if let Some(previous) = last_seq.insert(writer, seq)
            && seq < previous
        {
            found.out_of_order += 1;
        }
        if let Some(previous) = last_mono
            && mono(record) < previous
        {
            found.mono_inversions += 1;
        }
        last_mono = Some(mono(record));
    }
    found.lost = acked.iter().filter(|key| !seen.contains(key)).count() as u64;
    found
}
