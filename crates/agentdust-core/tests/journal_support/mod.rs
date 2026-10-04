#![allow(dead_code)]

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::{Condvar, LazyLock, Mutex};
use std::time::Duration;

use agentdust_core::journal::volume::FixedVolume;
use agentdust_core::journal::{
    Agent, Class, Journal, Kind, MAX_FRAME_LEN, ReadPoint, ReadProbe, Record, SCHEMA_VERSION, scan,
};

pub const HANG_GUARD: Duration = Duration::from_secs(60);

static APFS: LazyLock<FixedVolume> = LazyLock::new(FixedVolume::apfs_local);

pub fn journal(dir: &Path) -> Journal<'_> {
    Journal::with_volume(dir, &*APFS)
}

pub fn record(session: &str, boot: &str, wall_ts: u64, mono_ts: u64) -> Record {
    Record {
        v: SCHEMA_VERSION,
        kind: Kind::ShellStart,
        agent: Agent::Claude,
        session_id: session.to_owned(),
        subagent_id: None,
        tool_use_id: None,
        wall_ts,
        mono_ts,
        boot: boot.to_owned(),
        cwd_key: None,
        exe_base: None,
    }
}

pub fn named(session: &str) -> Record {
    record(session, "boot", 1_800_000_000_000, 2)
}

pub fn sessions(records: &[Record]) -> Vec<&str> {
    records.iter().map(|record| record.session_id.as_str()).collect()
}

pub fn count_of(records: &[Record], session: &str) -> usize {
    records
        .iter()
        .filter(|record| record.session_id == session)
        .count()
}

pub fn json(record: &Record) -> Vec<u8> {
    serde_json::to_vec(record).unwrap()
}

pub fn frame(record: &Record) -> Vec<u8> {
    let mut bytes = vec![0x1e];
    bytes.extend(json(record));
    bytes.push(b'\n');
    bytes
}

pub fn bare_line(record: &Record) -> Vec<u8> {
    let mut bytes = json(record);
    bytes.push(b'\n');
    bytes
}

pub fn edited_frame(record: &Record, edit: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let mut value = serde_json::to_value(record).unwrap();
    edit(&mut value);
    let mut bytes = vec![0x1e];
    bytes.extend(serde_json::to_vec(&value).unwrap());
    bytes.push(b'\n');
    bytes
}

pub fn frames(records: &[Record]) -> Vec<u8> {
    records.iter().flat_map(frame).collect()
}

pub fn join(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

pub fn padded_to_frame_len(total: usize, prefix: &str) -> Record {
    let mut padded = named(prefix);
    let base = frame(&padded).len();
    padded.session_id.push_str(&"x".repeat(total - base));
    assert_eq!(frame(&padded).len(), total);
    assert!(total <= MAX_FRAME_LEN + 1);
    padded
}

pub fn plant(dir: &Path, name: &str, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dir.join(name))
        .unwrap();
    file.write_all(bytes).unwrap();
}

pub fn append_raw(dir: &Path, name: &str, bytes: &[u8]) {
    let mut file = OpenOptions::new().append(true).open(dir.join(name)).unwrap();
    file.write_all(bytes).unwrap();
}

pub fn generation_name(stamp: u64) -> String {
    format!("journal.{stamp}.jsonl")
}

pub fn rotate_by_rename(dir: &Path, stamp: u64) {
    fs::rename(dir.join("journal.jsonl"), dir.join(generation_name(stamp))).unwrap();
}

pub fn compact(dir: &Path, stamp: u64, keep: impl Fn(&Record) -> bool) {
    let path = dir.join(generation_name(stamp));
    let mut kept: Vec<Vec<u8>> = Vec::new();
    scan(File::open(&path).unwrap(), |raw, class| {
        if let Class::Record(record) = &class
            && keep(record)
        {
            kept.push(raw.to_vec());
        }
    })
    .unwrap();
    if kept.is_empty() {
        fs::remove_file(&path).unwrap();
        return;
    }
    let tmp = dir.join("journal.compact.tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .unwrap();
    for raw in kept {
        file.write_all(&[0x1e]).unwrap();
        file.write_all(&raw).unwrap();
        file.write_all(b"\n").unwrap();
    }
    file.sync_all().unwrap();
    fs::rename(&tmp, &path).unwrap();
}

pub fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

pub fn copies_on_disk(dir: &Path, session: &str) -> usize {
    names_in(dir)
        .into_iter()
        .filter(|name| name.starts_with("journal.") && name.ends_with(".jsonl"))
        .map(|name| {
            let mut found = 0;
            scan(File::open(dir.join(name)).unwrap(), |_, class| {
                if let Class::Record(record) = class {
                    found += usize::from(record.session_id == session);
                }
            })
            .unwrap();
            found
        })
        .sum()
}

struct GateState {
    reached: bool,
    released: bool,
}

pub struct Gate {
    point: ReadPoint,
    state: Mutex<GateState>,
    changed: Condvar,
}

impl Gate {
    pub fn at(point: ReadPoint) -> Self {
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
            assert!(!timeout.timed_out(), "the reader never reached {:?}", self.point);
            state = next;
        }
    }

    pub fn release(&self) {
        self.state.lock().unwrap().released = true;
        self.changed.notify_all();
    }
}

impl ReadProbe for Gate {
    fn reached(&self, point: ReadPoint) {
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
