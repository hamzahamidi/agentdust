use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::safe_open::SafeOpenError;

mod fields;
mod frame;
mod generations;
mod store;
pub mod volume;

pub use fields::{CwdKey, ExeBase, FieldError, MAX_CWD_KEY_LEN, MAX_EXE_BASE_LEN};
pub use frame::{Class, MAX_FRAME_LEN, RS, decode, encode, scan};
pub use generations::{Generation, generation_path, generation_stamp, list_generations};
pub use volume::{FixedVolume, FsFacts, SystemVolume, VolumeProbe};

pub const SCHEMA_VERSION: u32 = 1;
pub const LOCK_BUDGET: Duration = Duration::from_millis(20);
pub const ACTIVE_FILE: &str = "journal.jsonl";
const LOCK_FILE: &str = "journal.lock";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    SessionStart,
    SessionEnd,
    ShellStart,
    ShellEnd,
    Sample,
    ServerStart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Agent {
    Claude,
    Codex,
    Cursor,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Record {
    pub v: u32,
    pub kind: Kind,
    pub agent: Agent,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    pub wall_ts: u64,
    pub mono_ts: u64,
    pub boot: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd_key: Option<CwdKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exe_base: Option<ExeBase>,
}

#[derive(Debug, Error)]
pub enum JournalError {
    #[error("journal lock stayed busy beyond the write budget")]
    LockBusy,
    #[error("refused to open the journal: {0}")]
    Refused(SafeOpenError),
    #[error("the frame is {len} bytes and the limit is {max}")]
    TooLarge { len: usize, max: usize },
    #[error("the record has schema version {found} and this build writes version {SCHEMA_VERSION}")]
    WrongVersion { found: u32 },
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Encode(#[from] serde_json::Error),
}

impl From<SafeOpenError> for JournalError {
    fn from(err: SafeOpenError) -> Self {
        match err {
            SafeOpenError::Io(err) => Self::Io(err),
            refused => Self::Refused(refused),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReadReport {
    pub records: Vec<Record>,
    pub malformed_lines: usize,
    pub torn_frames: usize,
    pub truncated_last_line: bool,
    pub newer_version_lines: usize,
    pub unknown_kind_lines: usize,
    pub unsupported_version: bool,
    pub duplicates_removed: usize,
    pub unsafe_files: usize,
    pub filesystem: Option<FsFacts>,
}

impl ReadReport {
    pub fn skipped_lines(&self) -> usize {
        self.malformed_lines + self.torn_frames + self.newer_version_lines + self.unknown_kind_lines
    }

    pub fn on_unsupported_filesystem(&self) -> bool {
        self.filesystem.as_ref().is_some_and(|facts| !facts.supported)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadPoint {
    ActiveOpened,
    GenerationsListed,
}

pub trait ReadProbe: Sync {
    fn reached(&self, point: ReadPoint);
}

pub struct NoProbe;

impl ReadProbe for NoProbe {
    fn reached(&self, _point: ReadPoint) {}
}

#[derive(Clone, Copy)]
pub struct Journal<'a> {
    dir: &'a Path,
    volume: &'a dyn VolumeProbe,
}

impl<'a> Journal<'a> {
    pub fn new(dir: &'a Path) -> Self {
        Self::with_volume(dir, &SystemVolume)
    }

    pub fn with_volume(dir: &'a Path, volume: &'a dyn VolumeProbe) -> Self {
        Self { dir, volume }
    }

    pub fn read(&self) -> Result<ReadReport, JournalError> {
        self.read_observed(&NoProbe)
    }

    pub fn read_observed(&self, probe: &dyn ReadProbe) -> Result<ReadReport, JournalError> {
        store::read(self.dir, self.volume, probe)
    }

    pub fn status(&self) -> io::Result<FsFacts> {
        volume::locate(self.volume, self.dir)
    }
}

pub fn status(dir: &Path) -> io::Result<FsFacts> {
    Journal::new(dir).status()
}

pub fn append(dir: &Path, record: &Record) -> Result<(), JournalError> {
    let mut line = serde_json::to_vec(record)?;
    line.push(b'\n');
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let lock = open_private(&dir.join(LOCK_FILE), OpenMode::Lock)?;
    lock_within(&lock, LOCK_BUDGET)?;
    let mut journal = open_private(&dir.join(ACTIVE_FILE), OpenMode::Append)?;
    journal.write_all(&line)?;
    Ok(())
}

pub fn read(dir: &Path) -> Result<ReadReport, JournalError> {
    Journal::new(dir).read()
}

enum OpenMode {
    Lock,
    Append,
}

fn open_private(path: &Path, mode: OpenMode) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).mode(0o600).custom_flags(libc::O_NOFOLLOW);
    match mode {
        OpenMode::Lock => options.write(true),
        OpenMode::Append => options.append(true),
    };
    options.open(path)
}

fn lock_within(file: &File, budget: Duration) -> Result<(), JournalError> {
    let start = Instant::now();
    loop {
        // SAFETY: the descriptor stays open for as long as `file` is borrowed.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EWOULDBLOCK) {
            return Err(err.into());
        }
        if start.elapsed() >= budget {
            return Err(JournalError::LockBusy);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
