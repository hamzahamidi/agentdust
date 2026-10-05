use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use serde::Serialize;
use thiserror::Error;

use crate::class::Class;
use crate::classifier::Evidence;
use crate::clock;
use crate::finding::ModelFinding;
use crate::identity::KernelIdentity;
use crate::safe_open::{self, SafeOpenError};

pub const AUDIT_FILE: &str = "audit.log";
pub const AUDIT_MAX_BYTES: u64 = 5 * 1024 * 1024;
pub const AUDIT_KEYS: [&str; 13] = [
    "v", "wall_ms", "plan", "item", "pid", "start_us", "uid", "class", "evidence", "exe_base", "phase",
    "result", "reason",
];
const OLDER_FILE: &str = "audit.log.1";
const LOCK_FILE: &str = "audit.lock";
const VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum AuditError {
    #[error("refused to open an audit file: {0}")]
    Refused(SafeOpenError),
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl From<SafeOpenError> for AuditError {
    fn from(err: SafeOpenError) -> Self {
        match err {
            SafeOpenError::Io(err) => Self::Io(err),
            refused => Self::Refused(refused),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Attempt,
    Result,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    plan: String,
    item: String,
    pid: i32,
    start_us: u64,
    uid: u32,
    class: Class,
    evidence: Vec<Evidence>,
    exe_base: Option<String>,
    phase: Phase,
    result: Option<String>,
    reason: Option<String>,
}

impl Entry {
    pub fn attempt(plan: &str, model: &ModelFinding, kernel: &KernelIdentity) -> Self {
        Self::of(plan, model, kernel, Phase::Attempt, None, None)
    }

    pub fn result(
        plan: &str,
        model: &ModelFinding,
        kernel: &KernelIdentity,
        result: &str,
        reason: Option<&str>,
    ) -> Self {
        Self::of(
            plan,
            model,
            kernel,
            Phase::Result,
            Some(result.to_owned()),
            reason.map(str::to_owned),
        )
    }

    fn of(
        plan: &str,
        model: &ModelFinding,
        kernel: &KernelIdentity,
        phase: Phase,
        result: Option<String>,
        reason: Option<String>,
    ) -> Self {
        Self {
            plan: plan.to_owned(),
            item: model.item_id.clone(),
            pid: kernel.pid,
            start_us: kernel.start_time_us,
            uid: kernel.uid,
            class: model.class,
            evidence: model.evidence.clone(),
            exe_base: model.exe_base.clone(),
            phase,
            result,
            reason,
        }
    }
}

#[derive(Serialize)]
struct Line<'a> {
    v: u32,
    wall_ms: u64,
    plan: &'a str,
    item: &'a str,
    pid: i32,
    start_us: u64,
    uid: u32,
    class: Class,
    evidence: &'a [Evidence],
    exe_base: Option<&'a str>,
    phase: Phase,
    result: Option<&'a str>,
    reason: Option<&'a str>,
}

pub struct AuditLog {
    dir: PathBuf,
    max_bytes: u64,
}

impl AuditLog {
    pub fn new(dir: &Path, max_bytes: u64) -> Self {
        Self {
            dir: dir.to_path_buf(),
            max_bytes,
        }
    }

    pub fn append(&self, entry: &Entry) -> Result<(), AuditError> {
        let mut line = serde_json::to_vec(&Line {
            v: VERSION,
            wall_ms: clock::wall_ms(),
            plan: &entry.plan,
            item: &entry.item,
            pid: entry.pid,
            start_us: entry.start_us,
            uid: entry.uid,
            class: entry.class,
            evidence: &entry.evidence,
            exe_base: entry.exe_base.as_deref(),
            phase: entry.phase,
            result: entry.result.as_deref(),
            reason: entry.reason.as_deref(),
        })
        .expect("an entry of plain fields always serialises");
        line.push(b'\n');
        safe_open::ensure_dir(&self.dir)?;
        let _guard = Guard::take(&self.dir.join(LOCK_FILE))?;
        let path = self.dir.join(AUDIT_FILE);
        let mut file = safe_open::open_private_append(&path)?;
        let length = file.metadata()?.len();
        if length > 0 && length + line.len() as u64 > self.max_bytes {
            fs::rename(&path, self.dir.join(OLDER_FILE))?;
            file = safe_open::open_private_append(&path)?;
        }
        file.write_all(&line)?;
        Ok(())
    }
}

struct Guard {
    _file: File,
}

impl Guard {
    fn take(path: &Path) -> Result<Self, AuditError> {
        let file = safe_open::open_lock_file(path)?;
        loop {
            // SAFETY: the descriptor belongs to `file`, which is open for the duration of the call.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
                return Ok(Self { _file: file });
            }
            let err = io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::EINTR) {
                return Err(err.into());
            }
        }
    }
}
