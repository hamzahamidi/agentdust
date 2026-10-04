use std::collections::{HashMap, HashSet};
use std::fs::{self, DirBuilder, File, OpenOptions, TryLockError};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::time::{Duration, Instant};

use agentdust_core::journal::Record;

use crate::frame::decode;
use crate::maintenance::list_generations;
use crate::payload::{mono, wall};
use crate::probe::{Point, Probe};

pub const ACTIVE: &str = "journal.jsonl";
pub const LOCK: &str = "journal.lock";
pub const MAINT: &str = "journal.maint";
pub const COMPACT_TMP: &str = "journal.compact.tmp";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReadOutcome {
    pub records: Vec<Record>,
    pub malformed_lines: usize,
    pub torn_frames: usize,
    pub newer_version_lines: usize,
    pub unknown_kind_lines: usize,
    pub duplicates_removed: usize,
}

impl ReadOutcome {
    pub fn skipped_lines(&self) -> usize {
        self.malformed_lines + self.torn_frames + self.newer_version_lines + self.unknown_kind_lines
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadLock {
    None,
    Whole,
    Opening,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    Shared,
    Exclusive,
}

pub enum LockError {
    Busy,
    Io(io::Error),
}

pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(dir)
}

pub fn open_private(
    path: &Path,
    access: impl FnOnce(&mut OpenOptions) -> &mut OpenOptions,
) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).mode(0o600).custom_flags(libc::O_NOFOLLOW);
    access(&mut options).open(path)
}

pub fn open_active(dir: &Path) -> io::Result<File> {
    open_private(&dir.join(ACTIVE), |options| options.append(true))
}

pub fn open_lock(dir: &Path, name: &str) -> io::Result<File> {
    open_private(&dir.join(name), |options| options.write(true))
}

pub fn open_regular(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    if file.metadata()?.is_file() {
        Ok(file)
    } else {
        Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"))
    }
}

pub fn lock_within(file: &File, mode: LockMode, budget: Duration, pause: Duration) -> Result<(), LockError> {
    let start = Instant::now();
    loop {
        let attempt = match mode {
            LockMode::Shared => file.try_lock_shared(),
            LockMode::Exclusive => file.try_lock(),
        };
        match attempt {
            Ok(()) => return Ok(()),
            Err(TryLockError::WouldBlock) => {
                if start.elapsed() >= budget {
                    return Err(LockError::Busy);
                }
                std::thread::sleep(pause);
            }
            Err(TryLockError::Error(err)) => return Err(LockError::Io(err)),
        }
    }
}

pub fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

pub fn count_files(dir: &Path) -> io::Result<usize> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err),
    };
    let mut count = 0;
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            count += count_files(&entry.path())?;
        } else {
            count += 1;
        }
    }
    Ok(count)
}

pub fn read_snapshot(dir: &Path, lock: ReadLock, probe: &dyn Probe) -> io::Result<ReadOutcome> {
    match fs::symlink_metadata(dir) {
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(ReadOutcome::default()),
        Err(err) => return Err(err),
    }
    let guard = match lock {
        ReadLock::None => None,
        ReadLock::Whole | ReadLock::Opening => {
            let file = open_lock(dir, LOCK)?;
            file.lock_shared()?;
            Some(file)
        }
    };
    let (files, unreadable) = open_all(dir, probe)?;
    let guard = if lock == ReadLock::Opening { None } else { guard };
    let mut outcome = ReadOutcome {
        malformed_lines: unreadable,
        ..ReadOutcome::default()
    };
    let mut records = Vec::new();
    for file in files {
        let decoded = decode(file)?;
        records.extend(decoded.records);
        outcome.malformed_lines += decoded.malformed;
        outcome.torn_frames += decoded.torn;
        outcome.newer_version_lines += decoded.newer_version;
        outcome.unknown_kind_lines += decoded.unknown_kind;
    }
    drop(guard);
    order(&mut records);
    let before = records.len();
    records.dedup();
    outcome.duplicates_removed = before - records.len();
    outcome.records = records;
    Ok(outcome)
}

fn open_all(dir: &Path, probe: &dyn Probe) -> io::Result<(Vec<File>, usize)> {
    let mut unreadable = 0;
    let active = match open_regular(&dir.join(ACTIVE)) {
        Ok(file) => Some(file),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(_) => {
            unreadable += 1;
            None
        }
    };
    probe.reached(Point::ReaderAfterActive);
    let generations = list_generations(dir)?;
    probe.reached(Point::ReaderAfterList);
    let mut identities = HashSet::new();
    let mut files = Vec::new();
    if let Some(file) = active {
        identities.insert(identity(&file)?);
        files.push(file);
    }
    for generation in generations {
        match open_regular(&generation.path) {
            Ok(file) => {
                if identities.insert(identity(&file)?) {
                    files.push(file);
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(_) => unreadable += 1,
        }
    }
    Ok((files, unreadable))
}

fn identity(file: &File) -> io::Result<(u64, u64)> {
    let meta = file.metadata()?;
    Ok((meta.dev(), meta.ino()))
}

fn order(records: &mut [Record]) {
    let mut first_wall: HashMap<&str, u64> = HashMap::new();
    for record in records.iter() {
        let first = first_wall.entry(record.boot.as_str()).or_insert(u64::MAX);
        *first = (*first).min(wall(record));
    }
    let mut boots: Vec<(u64, &str)> = first_wall.iter().map(|(boot, wall)| (*wall, *boot)).collect();
    boots.sort_unstable();
    let rank: HashMap<String, usize> = boots
        .into_iter()
        .enumerate()
        .map(|(position, (_, boot))| (boot.to_owned(), position))
        .collect();
    records.sort_by_cached_key(|record| (rank[record.boot.as_str()], mono(record)));
    let mut start = 0;
    while start < records.len() {
        let mut end = start + 1;
        while end < records.len() && same_place(&records[end], &records[start]) {
            end += 1;
        }
        if end - start > 1 {
            records[start..end].sort_by(|a, b| tie_key(a).cmp(&tie_key(b)));
        }
        start = end;
    }
}

fn same_place(a: &Record, b: &Record) -> bool {
    a.boot == b.boot && mono(a) == mono(b)
}

type TieKey<'a> = (u64, u8, &'a str, u8, Option<&'a str>, Option<&'a str>, u32);

fn tie_key(record: &Record) -> TieKey<'_> {
    (
        wall(record),
        record.agent as u8,
        record.session_id.as_str(),
        record.kind as u8,
        record.tool_use_id.as_deref(),
        record.subagent_id.as_deref(),
        record.v,
    )
}
