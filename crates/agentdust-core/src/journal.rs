use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

mod fields;

pub use fields::{CwdKey, ExeBase, FieldError, MAX_CWD_KEY_LEN, MAX_EXE_BASE_LEN};

pub const SCHEMA_VERSION: u32 = 1;
pub const LOCK_BUDGET: Duration = Duration::from_millis(20);
const JOURNAL_FILE: &str = "journal.jsonl";
const LOCK_FILE: &str = "journal.lock";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    SessionStart,
    SessionEnd,
    ShellStart,
    ShellEnd,
    Sample,
    ServerStart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Agent {
    Claude,
    Codex,
    Cursor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Encode(#[from] serde_json::Error),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReadReport {
    pub records: Vec<Record>,
    pub skipped_lines: usize,
}

pub fn append(dir: &Path, record: &Record) -> Result<(), JournalError> {
    let mut line = serde_json::to_vec(record)?;
    line.push(b'\n');
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let lock = open_private(&dir.join(LOCK_FILE), OpenMode::Lock)?;
    lock_within(&lock, LOCK_BUDGET)?;
    let mut journal = open_private(&dir.join(JOURNAL_FILE), OpenMode::Append)?;
    journal.write_all(&line)?;
    Ok(())
}

pub fn read(dir: &Path) -> io::Result<ReadReport> {
    let file = match File::open(dir.join(JOURNAL_FILE)) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(ReadReport::default()),
        Err(err) => return Err(err),
    };
    let mut report = ReadReport::default();
    for line in BufReader::new(file).lines() {
        match serde_json::from_str::<Record>(&line?) {
            Ok(record) if record.v == SCHEMA_VERSION => report.records.push(record),
            _ => report.skipped_lines += 1,
        }
    }
    Ok(report)
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
