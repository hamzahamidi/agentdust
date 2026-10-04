mod journal_support;
mod maintenance_support;
mod scratch;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use agentdust_core::journal::{Class, FrameWriter, JournalError, Rotation, scan};
use journal_support::{journal, names_in};
use maintenance_support::{BOOT, T, expired, keep_everything, numbered};
use scratch::TempDir;

const WRITERS: usize = 3;
const PER_WRITER: usize = 400;
const STRAGGLER_ROUNDS: usize = 20;
const THREADS: usize = WRITERS + 1;
const HANG_GUARD: Duration = Duration::from_secs(60);

struct Finished<'a>(&'a AtomicUsize);

impl Drop for Finished<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct Straggler<'a> {
    cycles: &'a AtomicUsize,
    waiting: bool,
}

impl FrameWriter for Straggler<'_> {
    fn write(&mut self, mut file: &File, frame: &[u8]) -> io::Result<usize> {
        if self.waiting {
            self.waiting = false;
            let target = self.cycles.load(Ordering::SeqCst) + 2;
            let started = Instant::now();
            while self.cycles.load(Ordering::SeqCst) < target {
                assert!(started.elapsed() < HANG_GUARD, "the rotator stopped cycling");
                thread::sleep(Duration::from_micros(200));
            }
        }
        file.write(frame)
    }
}

struct Written {
    acknowledged: Vec<String>,
    dropped: Vec<String>,
}

struct Rotated {
    markers: usize,
    dropped_markers: usize,
    other_drops: usize,
    last_rotation: Rotation,
    last_changed: usize,
}

fn files_holding(dir: &std::path::Path) -> BTreeMap<String, usize> {
    let mut found = BTreeMap::new();
    for name in names_in(dir) {
        if !(name.starts_with("journal.") && name.ends_with(".jsonl")) {
            continue;
        }
        scan(File::open(dir.join(name)).unwrap(), |_, class| {
            if let Class::Record(record) = class {
                *found.entry(record.session_id).or_default() += 1;
            }
        })
        .unwrap();
    }
    found
}

#[test]
fn every_acknowledged_record_is_present_once_after_a_forced_final_run_whatever_the_interleaving() {
    let dir = TempDir::private("retain-stress");
    let finished = AtomicUsize::new(0);
    let cycles = AtomicUsize::new(0);
    let acknowledged = Mutex::new(Vec::<String>::new());

    let (writers, rotator, reads) = thread::scope(|scope| {
        let writers: Vec<_> = (0..WRITERS)
            .map(|writer| {
                let (dir, finished, acknowledged) = (&dir, &finished, &acknowledged);
                scope.spawn(move || {
                    let _finished = Finished(finished);
                    let mut written = Written {
                        acknowledged: Vec::new(),
                        dropped: Vec::new(),
                    };
                    for seq in 0..PER_WRITER {
                        let id = format!("w{writer}-{seq}");
                        let record = numbered(&id, (writer * PER_WRITER + seq) as u64 + 1);
                        match journal(dir).append(&record) {
                            Ok(_) => {
                                acknowledged.lock().unwrap().push(id.clone());
                                written.acknowledged.push(id);
                            }
                            Err(JournalError::Stale { .. }) => written.dropped.push(id),
                            Err(other) => panic!("{id}: {other}"),
                        }
                        if seq % 8 == 0 {
                            thread::sleep(Duration::from_micros(100));
                        }
                    }
                    written
                })
            })
            .collect();

        let straggler = scope.spawn(|| {
            let _finished = Finished(&finished);
            let mut written = Written {
                acknowledged: Vec::new(),
                dropped: Vec::new(),
            };
            for round in 0..STRAGGLER_ROUNDS {
                let id = format!("s-{round}");
                let record = numbered(&id, 100_000 + round as u64);
                let mut writer = Straggler {
                    cycles: &cycles,
                    waiting: true,
                };
                match journal(&dir).append_with(&record, &mut writer) {
                    Ok(appended) => {
                        assert!(appended.attempts >= 2, "{id} was written once across a full run");
                        acknowledged.lock().unwrap().push(id.clone());
                        written.acknowledged.push(id);
                    }
                    Err(JournalError::Stale { .. }) => written.dropped.push(id),
                    Err(other) => panic!("{id}: {other}"),
                }
            }
            written
        });

        let rotator = scope.spawn(|| {
            let mut summary = Rotated {
                markers: 0,
                dropped_markers: 0,
                other_drops: 0,
                last_rotation: Rotation::Empty,
                last_changed: 0,
            };
            loop {
                let last = finished.load(Ordering::SeqCst) == THREADS;
                summary.markers += 1;
                let marker = expired(
                    &format!("marker-{}", summary.markers),
                    10_000 + summary.markers as u64,
                );
                journal(&dir).append(&marker).unwrap();
                let report = journal(&dir)
                    .prune(&keep_everything(), T + summary.markers as u64, BOOT)
                    .unwrap();
                summary.dropped_markers += report.retained.dropped_earlier_boot;
                summary.other_drops += report.retained.dropped_aged
                    + report.retained.dropped_over_cap
                    + report.retained.dropped_pinned;
                summary.last_rotation = report.rotation;
                summary.last_changed = report.retained.rewritten + report.retained.deleted;
                cycles.fetch_add(1, Ordering::SeqCst);
                if last {
                    return summary;
                }
                thread::sleep(Duration::from_millis(1));
            }
        });

        let reader = scope.spawn(|| {
            let mut reads = 0usize;
            loop {
                let last = finished.load(Ordering::SeqCst) == THREADS;
                let before = acknowledged.lock().unwrap().clone();
                let report = journal(&dir).read().unwrap();
                reads += 1;
                assert_eq!(report.malformed_lines, 0);
                assert_eq!(report.newer_version_lines + report.unknown_kind_lines, 0);
                let current: BTreeSet<&str> = report
                    .records
                    .iter()
                    .map(|record| record.session_id.as_str())
                    .collect();
                let missing: Vec<_> = before
                    .iter()
                    .filter(|id| !current.contains(id.as_str()))
                    .take(3)
                    .collect();
                assert!(
                    missing.is_empty(),
                    "a read lacks records acknowledged before it began: {missing:?}"
                );
                if last {
                    return reads;
                }
                thread::sleep(Duration::from_millis(2));
            }
        });

        let mut written: Vec<Written> = writers.into_iter().map(|writer| writer.join().unwrap()).collect();
        written.push(straggler.join().unwrap());
        (written, rotator.join().unwrap(), reader.join().unwrap())
    });

    assert!(reads >= 1);
    assert_eq!(
        rotator.last_rotation,
        Rotation::Rotated {
            stamp: T + rotator.markers as u64
        }
    );
    assert!(rotator.last_changed >= 1, "the final run found nothing to drop");
    assert_eq!(rotator.dropped_markers, rotator.markers);
    assert_eq!(rotator.other_drops, 0);

    let report = journal(&dir).read().unwrap();
    assert_eq!(report.duplicates_removed, 0);
    assert_eq!(report.torn_frames + report.malformed_lines, 0);
    let mut in_read: BTreeMap<&str, usize> = BTreeMap::new();
    for record in &report.records {
        *in_read.entry(record.session_id.as_str()).or_default() += 1;
    }
    let on_disk = files_holding(&dir);
    for written in &writers {
        for id in &written.acknowledged {
            assert_eq!(in_read.remove(id.as_str()), Some(1), "{id} in the read");
            assert_eq!(on_disk.get(id), Some(&1), "{id} on disk");
        }
        for id in &written.dropped {
            assert!(in_read.remove(id.as_str()).unwrap_or(1) <= 1, "{id}");
        }
    }
    assert!(
        in_read.is_empty(),
        "unknown records {:?}",
        in_read.keys().take(5).collect::<Vec<_>>()
    );

    let names = names_in(&dir);
    assert!(!names.contains(&"journal.jsonl".to_owned()), "{names:?}");
    assert!(!names.contains(&"journal.compact.tmp".to_owned()), "{names:?}");
    assert!(names.iter().all(|name| !name.contains("corrupt")), "{names:?}");
    for name in &names {
        let path = dir.join(name);
        if fs::metadata(&path).unwrap().is_file() {
            assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600, "{name}");
        }
    }
}
