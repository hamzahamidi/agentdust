#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::fs::{self, File};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::thread;

use agentdust_core::journal::retention::Policy;
use agentdust_core::journal::{
    Appended, JournalError, MaintenanceError, MaintenancePoint, MaintenanceProbe, Record,
};
use agentdust_core::safe_open::SafeOpenError;

use crate::journal_support::{HANG_GUARD, Pause, Paused, WritePoint, journal, named, paused, plant};

pub const T: u64 = 1_800_000_000_000;
pub const BOOT: &str = "boot";
pub const OLD_BOOT: &str = "an-earlier-boot";

pub type Written = Result<Appended, JournalError>;

struct GateState {
    reached: bool,
    released: bool,
}

pub struct MaintenanceGate {
    point: MaintenancePoint,
    state: Mutex<GateState>,
    changed: Condvar,
}

impl MaintenanceGate {
    pub fn at(point: MaintenancePoint) -> Self {
        Self {
            point,
            state: Mutex::new(GateState {
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
            assert!(!timeout.timed_out(), "maintenance never reached {:?}", self.point);
            state = next;
        }
    }

    pub fn release(&self) {
        self.state.lock().unwrap().released = true;
        self.changed.notify_all();
    }
}

impl MaintenanceProbe for MaintenanceGate {
    fn reached(&self, point: MaintenancePoint) {
        if point != self.point {
            return;
        }
        let mut state = self.state.lock().unwrap();
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

pub struct Recorded(pub Mutex<Vec<MaintenancePoint>>);

impl Recorded {
    pub fn new() -> Self {
        Self(Mutex::new(Vec::new()))
    }

    pub fn points(&self) -> Vec<MaintenancePoint> {
        self.0.lock().unwrap().clone()
    }
}

impl MaintenanceProbe for Recorded {
    fn reached(&self, point: MaintenancePoint) {
        self.0.lock().unwrap().push(point);
    }
}

pub fn keep_everything() -> Policy {
    Policy {
        max_bytes: u64::MAX,
        ..Policy::default()
    }
}

pub fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            files.insert(
                entry.file_name().into_string().unwrap(),
                fs::read(entry.path()).unwrap(),
            );
        }
    }
    files
}

pub fn without_lock(mut files: BTreeMap<String, Vec<u8>>) -> BTreeMap<String, Vec<u8>> {
    files.remove("journal.maint");
    files
}

pub fn mode_of(path: &Path) -> u32 {
    fs::metadata(path).unwrap().mode() & 0o777
}

pub fn inode_of(path: &Path) -> u64 {
    fs::metadata(path).unwrap().ino()
}

pub fn on_boot(mut record: Record, boot: &str) -> Record {
    record.boot = boot.to_owned();
    record
}

pub fn numbered(session: &str, mono_ts: u64) -> Record {
    let mut record = named(session);
    record.mono_ts = mono_ts;
    record
}

pub fn expired(session: &str, mono_ts: u64) -> Record {
    on_boot(numbered(session, mono_ts), OLD_BOOT)
}

pub fn refused_by<T: Debug>(result: Result<T, MaintenanceError>) -> (PathBuf, SafeOpenError) {
    match result {
        Err(MaintenanceError::Refused { path, source }) => (path, source),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

pub fn hold_exclusively(dir: &Path) -> File {
    let path = dir.join("journal.maint");
    if !path.exists() {
        plant(dir, "journal.maint", b"");
    }
    let held = File::open(path).unwrap();
    held.lock().unwrap();
    held
}

pub fn lock_is_free(dir: &Path) -> bool {
    let probe = File::open(dir.join("journal.maint")).unwrap();
    probe.try_lock().is_ok()
}

pub fn with_paused_writer<R>(
    dir: &Path,
    record: &Record,
    point: WritePoint,
    attempt: u32,
    during: impl FnOnce() -> R,
) -> (Written, R) {
    let (mut writer, pause): (Paused, Pause) = paused(point, attempt);
    thread::scope(|scope| {
        let appending = scope.spawn(|| journal(dir).append_with(record, &mut writer));
        pause.wait_reached();
        let observed = during();
        pause.release();
        (appending.join().unwrap(), observed)
    })
}

pub fn attempts(written: &Written) -> u32 {
    written.as_ref().expect("the append was acknowledged").attempts
}
