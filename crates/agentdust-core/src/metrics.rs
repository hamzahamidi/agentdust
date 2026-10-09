use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::class::Class;
use crate::safe_open::{self, Access, SafeOpenError};

pub const OUTCOME_FILE: &str = "outcomes.jsonl";
pub const OUTCOME_MAX_BYTES: u64 = 5 * 1024 * 1024;
const OLDER_FILE: &str = "outcomes.jsonl.1";
const LOCK_FILE: &str = "outcomes.lock";
const VERSION: u32 = 1;
const DAY_MS: u64 = 86_400_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Automatic,
    Manual,
}

impl Mode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Event {
    v: u32,
    wall_ms: u64,
    mode: Mode,
    class: Class,
    result: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DailySummary {
    pub day_start_wall_ms: u64,
    pub by_mode: BTreeMap<String, BTreeMap<String, u64>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub schema_version: u32,
    pub event_count: u64,
    pub skipped_lines: u64,
    pub start_wall_ms: Option<u64>,
    pub end_wall_ms: Option<u64>,
    pub by_outcome: BTreeMap<String, u64>,
    pub by_mode: BTreeMap<String, BTreeMap<String, u64>>,
    pub by_class: BTreeMap<String, u64>,
    pub daily: Vec<DailySummary>,
}

impl Default for Summary {
    fn default() -> Self {
        Self {
            schema_version: VERSION,
            event_count: 0,
            skipped_lines: 0,
            start_wall_ms: None,
            end_wall_ms: None,
            by_outcome: BTreeMap::new(),
            by_mode: BTreeMap::new(),
            by_class: BTreeMap::new(),
            daily: Vec::new(),
        }
    }
}

pub struct OutcomeLog {
    dir: PathBuf,
    max_bytes: u64,
}

impl OutcomeLog {
    pub fn new(dir: &Path) -> Self {
        Self::with_limit(dir, OUTCOME_MAX_BYTES)
    }

    pub fn with_limit(dir: &Path, max_bytes: u64) -> Self {
        Self {
            dir: dir.to_path_buf(),
            max_bytes,
        }
    }

    pub fn append(&self, mode: Mode, class: Class, result: &str, wall_ms: u64) -> io::Result<()> {
        if !valid_result(result) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unknown cleanup result",
            ));
        }
        let mut line = serde_json::to_vec(&Event {
            v: VERSION,
            wall_ms,
            mode,
            class,
            result: result.to_owned(),
        })
        .map_err(io::Error::other)?;
        line.push(b'\n');
        if line.len() as u64 > self.max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cleanup outcome exceeds its size limit",
            ));
        }
        safe_open::ensure_dir(&self.dir).map_err(io_error)?;
        let _guard = Lock::exclusive(&self.dir.join(LOCK_FILE))?;
        let path = self.dir.join(OUTCOME_FILE);
        let mut file = safe_open::open_private_append(&path).map_err(io_error)?;
        let length = file.metadata()?.len();
        if length > 0 && length + line.len() as u64 > self.max_bytes {
            match safe_open::open_file(&self.dir.join(OLDER_FILE), Access::Read) {
                Ok(_) => {}
                Err(SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(io_error(err)),
            }
            fs::rename(&path, self.dir.join(OLDER_FILE))?;
            file = safe_open::open_private_append(&path).map_err(io_error)?;
        }
        file.write_all(&line)
    }

    pub fn summary(dir: &Path) -> io::Result<Summary> {
        match safe_open::check_dir(dir) {
            Ok(()) => {}
            Err(SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(Summary::default());
            }
            Err(err) => return Err(io_error(err)),
        }
        let current = dir.join(OUTCOME_FILE);
        let older = dir.join(OLDER_FILE);
        if !entry_present(&current)? && !entry_present(&older)? {
            return Ok(Summary::default());
        }
        let _guard = Lock::shared(&dir.join(LOCK_FILE))?;
        let mut summary = Summary::default();
        let mut daily = BTreeMap::new();
        for path in [&older, &current] {
            read_events(path, &mut summary, &mut daily)?;
        }
        summary.daily = daily.into_values().collect();
        Ok(summary)
    }
}

fn read_events(
    path: &Path,
    summary: &mut Summary,
    daily: &mut BTreeMap<u64, DailySummary>,
) -> io::Result<()> {
    let file = match safe_open::open_file(path, Access::Read) {
        Ok(file) => file,
        Err(SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(io_error(err)),
    };
    let mut bytes = Vec::new();
    file.take(OUTCOME_MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > OUTCOME_MAX_BYTES {
        return Err(io::Error::other("cleanup outcome log exceeds its size limit"));
    }
    for line in BufReader::new(bytes.as_slice()).lines() {
        let Ok(line) = line else {
            summary.skipped_lines += 1;
            continue;
        };
        let Ok(event) = serde_json::from_str::<Event>(&line) else {
            summary.skipped_lines += 1;
            continue;
        };
        if event.v != VERSION || !valid_result(&event.result) {
            summary.skipped_lines += 1;
            continue;
        }
        summary.add(event, daily);
    }
    Ok(())
}

impl Summary {
    fn add(&mut self, event: Event, daily: &mut BTreeMap<u64, DailySummary>) {
        self.event_count += 1;
        self.start_wall_ms = Some(
            self.start_wall_ms
                .map_or(event.wall_ms, |seen| seen.min(event.wall_ms)),
        );
        self.end_wall_ms = Some(
            self.end_wall_ms
                .map_or(event.wall_ms, |seen| seen.max(event.wall_ms)),
        );
        increment(&mut self.by_outcome, &event.result);
        increment(&mut self.by_class, event.class.as_str());
        let mode = event.mode.as_str();
        increment_nested(&mut self.by_mode, mode, &event.result);
        let day = event.wall_ms / DAY_MS * DAY_MS;
        let bucket = daily.entry(day).or_insert_with(|| DailySummary {
            day_start_wall_ms: day,
            by_mode: BTreeMap::new(),
        });
        increment_nested(&mut bucket.by_mode, mode, &event.result);
    }
}

fn entry_present(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

fn increment(counts: &mut BTreeMap<String, u64>, key: &str) {
    *counts.entry(key.to_owned()).or_default() += 1;
}

fn increment_nested(counts: &mut BTreeMap<String, BTreeMap<String, u64>>, mode: &str, result: &str) {
    increment(counts.entry(mode.to_owned()).or_default(), result);
}

fn valid_result(result: &str) -> bool {
    matches!(
        result,
        "terminated"
            | "survivor"
            | "gone"
            | "revalidation_failed"
            | "handled_elsewhere"
            | "signal_failed"
            | "audit_unavailable"
            | "lock_unavailable"
            | "disabled"
            | "declined"
            | "cancelled"
            | "timed_out"
            | "empty"
            | "wrong_code"
            | "expired"
            | "plan_expired"
            | "skipped"
    )
}

fn io_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

struct Lock {
    _file: File,
}

impl Lock {
    fn exclusive(path: &Path) -> io::Result<Self> {
        let file = safe_open::open_lock_file(path).map_err(io_error)?;
        Self::take(file, libc::LOCK_EX)
    }

    fn shared(path: &Path) -> io::Result<Self> {
        let file = safe_open::open_file(path, Access::Read).map_err(io_error)?;
        Self::take(file, libc::LOCK_SH)
    }

    fn take(file: File, kind: i32) -> io::Result<Self> {
        loop {
            // SAFETY: the descriptor belongs to `file`, which stays open for the duration of the lock.
            if unsafe { libc::flock(file.as_raw_fd(), kind) } == 0 {
                return Ok(Self { _file: file });
            }
            let err = io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::EINTR) {
                return Err(err);
            }
        }
    }
}

pub fn is_automatic_plan(plan_id: &str) -> bool {
    plan_id.starts_with("auto-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_plan_ids_are_recognized() {
        assert!(is_automatic_plan("auto-123"));
        assert!(!is_automatic_plan("manual-123"));
    }
}
