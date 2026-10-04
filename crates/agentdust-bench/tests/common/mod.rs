#![allow(dead_code)]

use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use agentdust_bench::{Candidate, Point, Probe};
use agentdust_core::journal::{Agent, Kind, Record, SCHEMA_VERSION};

pub const EXE: &str = env!("CARGO_BIN_EXE_journal-bench");
pub const HANG_GUARD: Duration = Duration::from_secs(60);

pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "agentdust-bench-test-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn create(&self) -> &Path {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.0)
            .unwrap();
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let kind = fs::symlink_metadata(&path).unwrap().file_type();
        if kind.is_dir() {
            found.extend(files_under(&path));
        } else {
            found.push(path);
        }
    }
    found.sort();
    found
}

pub fn names_in(dir: &Path) -> Vec<String> {
    files_under(dir)
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

pub fn data_files(dir: &Path) -> Vec<PathBuf> {
    files_under(dir)
        .into_iter()
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name != "journal.lock" && name != "journal.maint")
        })
        .collect()
}

pub fn stamped(session: &str, boot: &str, wall_ts_ms: u64, mono_ns: u64) -> Record {
    Record {
        v: SCHEMA_VERSION,
        kind: Kind::ShellStart,
        agent: Agent::Claude,
        session_id: session.to_owned(),
        subagent_id: None,
        tool_use_id: None,
        wall_ts_ms,
        mono_ns,
        boot: boot.to_owned(),
    }
}

pub fn named(session: &str, mono_ns: u64) -> Record {
    stamped(session, "boot", 1_800_000_000_000, mono_ns)
}

pub fn count_of(records: &[Record], session: &str) -> usize {
    records
        .iter()
        .filter(|record| record.session_id == session)
        .count()
}

pub fn plant(dir: &Path, name: &str, candidate: Candidate, records: &[Record]) {
    let bytes: Vec<u8> = records
        .iter()
        .flat_map(|record| agentdust_bench::frame::encode(record, candidate.framing()).unwrap())
        .collect();
    plant_bytes(dir, name, &bytes);
}

pub fn plant_bytes(dir: &Path, name: &str, bytes: &[u8]) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    DirBuilder::new().recursive(true).mode(0o700).create(dir).unwrap();
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dir.join(name))
        .unwrap();
    file.write_all(bytes).unwrap();
}

struct GateState {
    armed: bool,
    reached: bool,
    released: bool,
}

pub struct Gate {
    point: Point,
    state: Mutex<GateState>,
    changed: Condvar,
}

impl Gate {
    pub fn at(point: Point) -> Self {
        Self {
            point,
            state: Mutex::new(GateState {
                armed: true,
                reached: false,
                released: false,
            }),
            changed: Condvar::new(),
        }
    }

    pub fn wait_reached(&self) {
        let mut state = self.state.lock().unwrap();
        while !state.reached {
            let (next, timeout) = self.changed.wait_timeout(state, HANG_GUARD).unwrap();
            assert!(
                !timeout.timed_out(),
                "the paused thread never reached {:?}",
                self.point
            );
            state = next;
        }
    }

    pub fn release(&self) {
        self.state.lock().unwrap().released = true;
        self.changed.notify_all();
    }
}

impl Probe for Gate {
    fn reached(&self, point: Point) {
        if point != self.point {
            return;
        }
        let mut state = self.state.lock().unwrap();
        if !state.armed {
            return;
        }
        state.armed = false;
        state.reached = true;
        self.changed.notify_all();
        while !state.released {
            let (next, timeout) = self.changed.wait_timeout(state, HANG_GUARD).unwrap();
            assert!(
                !timeout.timed_out(),
                "the pause at {:?} was never released",
                self.point
            );
            state = next;
        }
    }
}

pub struct OnPoint<F: Fn(u32) + Sync> {
    point: Point,
    hits: Mutex<u32>,
    action: F,
}

impl<F: Fn(u32) + Sync> OnPoint<F> {
    pub fn new(point: Point, action: F) -> Self {
        Self {
            point,
            hits: Mutex::new(0),
            action,
        }
    }
}

impl<F: Fn(u32) + Sync> Probe for OnPoint<F> {
    fn reached(&self, point: Point) {
        if point != self.point {
            return;
        }
        let hit = {
            let mut hits = self.hits.lock().unwrap();
            *hits += 1;
            *hits
        };
        (self.action)(hit);
    }
}
