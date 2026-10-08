#![cfg(target_os = "macos")]

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::{self, DirBuilder, File};
use std::io::Read;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use agentdust_core::journal::{self, Class, scan};
use agentdust_testkit::Fixture;

const FIXTURE: &str = env!("CARGO_BIN_EXE_fixture-journal");
const WRITERS: u64 = 3;
const PER_WRITER: u64 = 4_000;
const WRITER_PACE_US: &str = "100";
const STRAGGLER_ROUNDS: u64 = 12;
const ROTATOR_PERIOD_MS: &str = "5";

struct Writer {
    attempted: u64,
    failed: BTreeSet<String>,
}

struct Straggler {
    attempted: u64,
    fewest_attempts: u64,
    failed: BTreeSet<String>,
}

struct Rotator {
    cycles: u64,
    rotations: u64,
    changes: u64,
    busy: u64,
    errors: u64,
    markers: u64,
    dropped: u64,
    other_drops: u64,
    releases: u64,
    final_rotated: bool,
    final_changed: u64,
}

fn spawn(args: &[&str]) -> Fixture {
    Fixture(
        Command::new(FIXTURE)
            .args(args)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    )
}

fn finish(mut fixture: Fixture) -> String {
    let mut output = String::new();
    fixture
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut output)
        .unwrap();
    let status = fixture.0.wait().unwrap();
    assert!(status.success(), "{status}: {output}");
    output
}

fn field<'a>(line: &'a str, name: &str) -> &'a str {
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(name)?.strip_prefix('='))
        .unwrap_or_else(|| panic!("no {name} in {line:?}"))
}

fn count(line: &str, name: &str) -> u64 {
    field(line, name).parse().unwrap()
}

fn ids(line: &str, name: &str) -> BTreeSet<String> {
    field(line, name)
        .split(',')
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect()
}

fn writer_stats(line: &str) -> Writer {
    Writer {
        attempted: count(line, "attempted"),
        failed: ids(line, "failed"),
    }
}

fn straggler_stats(line: &str) -> Straggler {
    Straggler {
        attempted: count(line, "attempted"),
        fewest_attempts: count(line, "fewest_attempts"),
        failed: ids(line, "failed"),
    }
}

fn rotator_stats(line: &str) -> Rotator {
    Rotator {
        cycles: count(line, "cycles"),
        rotations: count(line, "rotations"),
        changes: count(line, "changes"),
        busy: count(line, "busy"),
        errors: count(line, "errors"),
        markers: count(line, "markers"),
        dropped: count(line, "dropped"),
        other_drops: count(line, "other_drops"),
        releases: count(line, "releases"),
        final_rotated: count(line, "final_rotated") == 1,
        final_changed: count(line, "final_changed"),
    }
}

fn scratch_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("agentdust-stress-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    DirBuilder::new().mode(0o700).create(&root).unwrap();
    root
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn copies_on_disk(dir: &Path) -> BTreeMap<String, usize> {
    let mut found = BTreeMap::new();
    for name in names(dir) {
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
fn a_release_from_another_round_does_not_unblock_the_current_straggler() {
    let root = scratch_root("stale-release");
    let dir = root.join("data");
    let sync = root.join("sync");
    DirBuilder::new().mode(0o700).create(&sync).unwrap();
    let dir_arg = dir.to_str().unwrap();
    finish(spawn(&["write", dir_arg, "99", "1", "0"]));
    fs::write(sync.join("release"), b"stale").unwrap();
    fs::write(sync.join("release-99"), b"stale").unwrap();
    let mut straggler = spawn(&["straggle", dir_arg, sync.to_str().unwrap(), "1"]);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !sync.join("held-0").exists() {
        assert!(
            straggler.0.try_wait().unwrap().is_none(),
            "stale release unblocked the fixture"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "fixture did not hold its descriptor"
        );
        thread::sleep(Duration::from_millis(1));
    }
    assert!(straggler.0.try_wait().unwrap().is_none());
    let report = journal::prune(
        &dir,
        &Default::default(),
        agentdust_core::clock::wall_ms(),
        "stress-boot",
    )
    .unwrap();
    assert!(matches!(report.rotation, journal::Rotation::Rotated { .. }));
    fs::write(sync.join("release-0"), b"").unwrap();
    let stats = straggler_stats(&finish(straggler));
    assert_eq!(stats.attempted, 1);
    assert_eq!(stats.fewest_attempts, 2);
    assert!(stats.failed.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn three_writers_a_straggler_and_a_rotator_lose_tear_and_duplicate_nothing() {
    let root = scratch_root("writers");
    let (dir, sync, stop) = (root.join("data"), root.join("sync"), root.join("stop"));
    DirBuilder::new().mode(0o700).create(&sync).unwrap();
    let facts = journal::status(&dir).unwrap();
    assert!(facts.supported, "the temporary directory is {}", facts.describe());
    let (dir_arg, sync_arg, stop_arg) = (
        dir.to_str().unwrap(),
        sync.to_str().unwrap(),
        stop.to_str().unwrap(),
    );

    let rotator = spawn(&["rotate", dir_arg, sync_arg, stop_arg, ROTATOR_PERIOD_MS]);
    let mut writers: Vec<Fixture> = (0..WRITERS)
        .map(|writer| {
            spawn(&[
                "write",
                dir_arg,
                &writer.to_string(),
                &PER_WRITER.to_string(),
                WRITER_PACE_US,
            ])
        })
        .collect();
    let straggler = spawn(&["straggle", dir_arg, sync_arg, &STRAGGLER_ROUNDS.to_string()]);

    let done = Arc::new(AtomicBool::new(false));
    let reads = Arc::new(AtomicUsize::new(0));
    let failures = Arc::new(Mutex::new(Vec::<String>::new()));
    let reader = {
        let (dir, done, reads, failures) = (dir.clone(), done.clone(), reads.clone(), failures.clone());
        thread::spawn(move || {
            loop {
                let last = done.load(Ordering::SeqCst);
                match journal::read(&dir) {
                    Ok(report) => {
                        reads.fetch_add(1, Ordering::SeqCst);
                        let clean =
                            report.malformed_lines + report.newer_version_lines + report.unknown_kind_lines
                                == 0;
                        if !clean {
                            let summary: String = format!("{report:?}").chars().take(300).collect();
                            failures.lock().unwrap().push(summary);
                        }
                    }
                    Err(err) => failures.lock().unwrap().push(err.to_string()),
                }
                if last {
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }
        })
    };

    let writer_stats: Vec<Writer> = writers
        .drain(..)
        .map(|writer| writer_stats(&finish(writer)))
        .collect();
    let straggler_stats = straggler_stats(&finish(straggler));
    fs::write(&stop, b"stop").unwrap();
    let rotator_stats = rotator_stats(&finish(rotator));
    done.store(true, Ordering::SeqCst);
    reader.join().unwrap();

    eprintln!(
        "writers attempted {:?} failed {:?}, straggler fewest attempts {}, reads {}, cycles {}, rotations {}, changes {}",
        writer_stats.iter().map(|w| w.attempted).collect::<Vec<_>>(),
        writer_stats.iter().map(|w| w.failed.len()).collect::<Vec<_>>(),
        straggler_stats.fewest_attempts,
        reads.load(Ordering::SeqCst),
        rotator_stats.cycles,
        rotator_stats.rotations,
        rotator_stats.changes,
    );

    assert!(
        failures.lock().unwrap().is_empty(),
        "{:?}",
        failures.lock().unwrap()
    );
    assert!(reads.load(Ordering::SeqCst) >= 1);
    assert!(writer_stats.iter().all(|w| w.attempted == PER_WRITER));
    assert_eq!(straggler_stats.attempted, STRAGGLER_ROUNDS);
    assert!(
        straggler_stats.failed.len() < STRAGGLER_ROUNDS as usize && straggler_stats.fewest_attempts >= 2,
        "a straggler append finished without a second attempt: {}",
        straggler_stats.fewest_attempts
    );
    assert_eq!(rotator_stats.errors, 0);
    assert_eq!(rotator_stats.busy, 0);
    assert_eq!(rotator_stats.releases, STRAGGLER_ROUNDS);
    assert!(rotator_stats.rotations >= 1);
    assert!(rotator_stats.final_rotated, "the final run found no active file");
    assert!(
        rotator_stats.final_changed >= 1,
        "the final run found nothing to drop"
    );
    assert_eq!(rotator_stats.dropped, rotator_stats.markers);
    assert_eq!(rotator_stats.other_drops, 0);

    let report = journal::read(&dir).unwrap();
    assert_eq!(report.malformed_lines, 0);
    assert_eq!(report.torn_frames, 0);
    assert_eq!(report.duplicates_removed, 0);
    let mut in_read: BTreeMap<&str, usize> = BTreeMap::new();
    for record in &report.records {
        *in_read.entry(record.session_id.as_str()).or_default() += 1;
    }
    let on_disk = copies_on_disk(&dir);
    let mut dropped: HashSet<String> = straggler_stats.failed.iter().cloned().collect();
    dropped.extend(writer_stats.iter().flat_map(|w| w.failed.iter().cloned()));
    let expected: Vec<String> = (0..WRITERS)
        .flat_map(|writer| (0..PER_WRITER).map(move |seq| format!("w{writer}-{seq}")))
        .chain((0..STRAGGLER_ROUNDS).map(|round| format!("s-{round}")))
        .collect();
    for id in &expected {
        let found = in_read.remove(id.as_str()).unwrap_or(0);
        if dropped.contains(id) {
            assert!(found <= 1, "{id} was dropped and appears {found} times");
        } else {
            assert_eq!(
                found, 1,
                "{id} was acknowledged and appears {found} times in the read"
            );
            assert_eq!(on_disk.get(id), Some(&1), "{id} on disk");
        }
    }
    assert!(
        in_read.is_empty(),
        "unknown records {:?}",
        in_read.keys().take(5).collect::<Vec<_>>()
    );

    let files = names(&dir);
    assert!(!files.contains(&"journal.jsonl".to_owned()), "{files:?}");
    assert!(!files.contains(&"journal.compact.tmp".to_owned()), "{files:?}");
    assert!(files.iter().all(|name| !name.contains("corrupt")), "{files:?}");
    assert!(files.contains(&"journal.maint".to_owned()), "{files:?}");
    assert!(files.len() >= 2, "{files:?}");
    for name in &files {
        assert_eq!(
            fs::metadata(dir.join(name)).unwrap().mode() & 0o777,
            0o600,
            "{name}"
        );
    }
    assert_eq!(names(&sync), Vec::<String>::new());
    fs::remove_dir_all(&root).unwrap();
}
