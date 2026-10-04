mod journal_support;
mod scratch;

use std::collections::BTreeMap;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use agentdust_core::journal::JournalError;
use journal_support::{generation_name, journal, padded_to_frame_len};
use scratch::TempDir;

const WRITERS: usize = 8;
const PER_WRITER: usize = 150;

#[test]
fn every_acknowledged_record_is_present_exactly_once_while_the_active_file_is_rotated_and_read() {
    let dir = TempDir::absent("rotation-stress");
    journal(&dir).append(&padded_to_frame_len(200, "seed-")).unwrap();
    let stop = AtomicBool::new(false);
    let mut acknowledged: Vec<String> = Vec::new();
    let mut stale = 0usize;

    thread::scope(|scope| {
        let rotator = scope.spawn(|| {
            let mut stamp = 1u64;
            let mut rotations = 0u32;
            while !stop.load(Ordering::Relaxed) {
                if fs::rename(dir.join("journal.jsonl"), dir.join(generation_name(stamp))).is_ok() {
                    stamp += 1;
                    rotations += 1;
                }
                thread::sleep(Duration::from_millis(1));
            }
            rotations
        });
        let reader = scope.spawn(|| {
            let mut reads = 0u32;
            while !stop.load(Ordering::Relaxed) {
                journal(&dir).read().unwrap();
                reads += 1;
            }
            reads
        });
        let writers: Vec<_> = (0..WRITERS)
            .map(|writer| {
                let dir = &dir;
                scope.spawn(move || {
                    let mut outcomes = Vec::new();
                    for seq in 0..PER_WRITER {
                        let name = format!("w{writer}-s{seq}-");
                        let mut record = padded_to_frame_len(200, &name);
                        record.mono_ts = seq as u64;
                        outcomes.push((record.session_id.clone(), journal(dir).append(&record)));
                    }
                    outcomes
                })
            })
            .collect();
        for writer in writers {
            for (session, outcome) in writer.join().unwrap() {
                match outcome {
                    Ok(_) => acknowledged.push(session),
                    Err(JournalError::Stale { .. }) => stale += 1,
                    Err(other) => panic!("{other:?}"),
                }
            }
        }
        stop.store(true, Ordering::Relaxed);
        rotator.join().unwrap();
        reader.join().unwrap();
    });

    let report = journal(&dir).read().unwrap();
    assert_eq!(report.skipped_lines(), 0);
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for record in &report.records {
        *seen.entry(record.session_id.as_str()).or_default() += 1;
    }
    assert!(seen.values().all(|count| *count == 1));
    for session in &acknowledged {
        assert!(seen.contains_key(session.as_str()), "lost {session}");
    }
    assert_eq!(acknowledged.len() + stale, WRITERS * PER_WRITER);
}
