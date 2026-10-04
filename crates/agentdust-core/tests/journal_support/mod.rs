#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Condvar, LazyLock, Mutex};
use std::time::Duration;

use agentdust_core::journal::volume::FixedVolume;
use agentdust_core::journal::{
    Agent, Class, FrameWriter, Journal, Kind, MAX_FRAME_LEN, ReadPoint, ReadProbe, Record, SCHEMA_VERSION,
    scan,
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

pub enum Step {
    Pass,
    Interrupt,
    Take(usize),
    Fail(i32),
    LandThenFail(usize, i32),
}

pub struct Scripted {
    steps: VecDeque<Step>,
    pub calls: Vec<usize>,
}

impl Scripted {
    pub fn new(steps: impl IntoIterator<Item = Step>) -> Self {
        Self {
            steps: steps.into_iter().collect(),
            calls: Vec::new(),
        }
    }
}

impl FrameWriter for Scripted {
    fn write(&mut self, mut file: &File, frame: &[u8]) -> io::Result<usize> {
        self.calls.push(frame.len());
        match self.steps.pop_front().unwrap_or(Step::Pass) {
            Step::Pass => file.write(frame),
            Step::Interrupt => Err(io::Error::from_raw_os_error(libc::EINTR)),
            Step::Take(n) => file.write(&frame[..n.min(frame.len())]),
            Step::Fail(errno) => Err(io::Error::from_raw_os_error(errno)),
            Step::LandThenFail(n, errno) => {
                file.write_all(&frame[..n])?;
                Err(io::Error::from_raw_os_error(errno))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritePoint {
    BeforeWrite,
    AfterWrite,
}

pub struct Paused {
    point: WritePoint,
    attempt: u32,
    seen: u32,
    reached: Sender<()>,
    release: Receiver<()>,
}

pub struct Pause {
    reached: Receiver<()>,
    release: Sender<()>,
}

pub fn paused(point: WritePoint, attempt: u32) -> (Paused, Pause) {
    let (reached_tx, reached_rx) = channel();
    let (release_tx, release_rx) = channel();
    (
        Paused {
            point,
            attempt,
            seen: 0,
            reached: reached_tx,
            release: release_rx,
        },
        Pause {
            reached: reached_rx,
            release: release_tx,
        },
    )
}

impl Pause {
    pub fn wait_reached(&self) {
        self.reached
            .recv_timeout(HANG_GUARD)
            .expect("the writer never reached its pause");
    }

    pub fn release(&self) {
        let _ = self.release.send(());
    }
}

impl Paused {
    fn pause(&self) {
        self.reached.send(()).unwrap();
        self.release
            .recv_timeout(HANG_GUARD)
            .expect("the pause of the writer was never released");
    }
}

impl FrameWriter for Paused {
    fn write(&mut self, mut file: &File, frame: &[u8]) -> io::Result<usize> {
        self.seen += 1;
        let here = self.seen == self.attempt;
        if here && self.point == WritePoint::BeforeWrite {
            self.pause();
        }
        let written = file.write(frame)?;
        if here && self.point == WritePoint::AfterWrite {
            self.pause();
        }
        Ok(written)
    }
}

pub struct RotateAfterEachWrite<'a> {
    pub dir: &'a Path,
    pub next_stamp: u64,
}

impl FrameWriter for RotateAfterEachWrite<'_> {
    fn write(&mut self, mut file: &File, frame: &[u8]) -> io::Result<usize> {
        let written = file.write(frame)?;
        rotate_by_rename(self.dir, self.next_stamp);
        self.next_stamp += 1;
        Ok(written)
    }
}

pub struct RemoveAfterWrite<'a> {
    pub dir: &'a Path,
    pub left: u32,
}

impl FrameWriter for RemoveAfterWrite<'_> {
    fn write(&mut self, mut file: &File, frame: &[u8]) -> io::Result<usize> {
        let written = file.write(frame)?;
        if self.left > 0 {
            self.left -= 1;
            fs::remove_file(self.dir.join("journal.jsonl"))?;
        }
        Ok(written)
    }
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

pub fn non_empty_segments(bytes: &[u8]) -> usize {
    bytes
        .split(|byte| *byte == 0x1e || *byte == b'\n')
        .filter(|segment| !segment.is_empty())
        .count()
}

pub fn presentation_order_violation(records: &[Record]) -> Option<String> {
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
    records.windows(2).enumerate().find_map(|(at, pair)| {
        let before = (key(&pair[0]), &pair[0]);
        let after = (key(&pair[1]), &pair[1]);
        (before >= after).then(|| format!("records {at} and {} are not in strictly increasing order", at + 1))
    })
}
