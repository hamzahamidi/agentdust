use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use agentdust_core::journal::Record;
use thiserror::Error;

use crate::frame::{Class, Framing, scan, wrap};
use crate::store::{ACTIVE, COMPACT_TMP, LockError, open_private, open_regular, sync_dir};

#[derive(Debug, Error)]
pub enum MaintenanceError {
    #[error("a lock needed for maintenance is held by an appender, a reader or another maintenance run")]
    Busy,
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl From<LockError> for MaintenanceError {
    fn from(err: LockError) -> Self {
        match err {
            LockError::Busy => MaintenanceError::Busy,
            LockError::Io(err) => MaintenanceError::Io(err),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    Rotated { stamp: u64 },
    Empty,
}

pub struct Retention<'a> {
    pub now_ms: u64,
    pub grace_ms: u64,
    pub keep: &'a dyn Fn(&Record) -> bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RetainReport {
    pub eligible: usize,
    pub waiting: usize,
    pub held_newer_version: usize,
    pub refused: usize,
    pub untouched: usize,
    pub rewritten: usize,
    pub deleted: usize,
    pub dropped_records: usize,
    pub duplicates_removed: usize,
    pub dropped_damaged_lines: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generation {
    pub stamp: u64,
    pub path: PathBuf,
}

pub type Swap<'a> = &'a dyn Fn() -> Result<Option<File>, MaintenanceError>;

pub fn generation_stamp(name: &str) -> Option<u64> {
    let digits = name.strip_prefix("journal.")?.strip_suffix(".jsonl")?;
    let stamp: u64 = digits.parse().ok()?;
    (digits == stamp.to_string()).then_some(stamp)
}

pub fn generation_path(dir: &Path, stamp: u64) -> PathBuf {
    dir.join(format!("journal.{stamp}.jsonl"))
}

pub fn list_generations(dir: &Path) -> io::Result<Vec<Generation>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if let Some(stamp) = entry.file_name().to_str().and_then(generation_stamp) {
            found.push(Generation {
                stamp,
                path: entry.path(),
            });
        }
    }
    found.sort_by_key(|generation| generation.stamp);
    Ok(found)
}

pub fn rotate(dir: &Path, now_ms: u64, swap: Swap) -> Result<Rotation, MaintenanceError> {
    let active = dir.join(ACTIVE);
    let meta = match fs::symlink_metadata(&active) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Rotation::Empty),
        Err(err) => return Err(err.into()),
    };
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the active journal is not a regular file",
        )
        .into());
    }
    if meta.len() == 0 {
        return Ok(Rotation::Empty);
    }
    let guard = swap()?;
    let mut stamp = now_ms;
    let sealed = loop {
        let path = generation_path(dir, stamp);
        match fs::symlink_metadata(&path) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => break path,
            Err(err) => return Err(err.into()),
            Ok(_) => {
                stamp = stamp.checked_add(1).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::AlreadyExists, "no free generation stamp")
                })?;
            }
        }
    };
    fs::rename(&active, &sealed)?;
    drop(guard);
    sync_dir(dir)?;
    Ok(Rotation::Rotated { stamp })
}

struct Line {
    raw: Vec<u8>,
    class: Class,
}

pub fn retain(
    dir: &Path,
    plan: &Retention,
    framing: Framing,
    swap: Swap,
) -> Result<RetainReport, MaintenanceError> {
    let mut report = RetainReport::default();
    let generations = match list_generations(dir) {
        Ok(generations) => generations,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(report),
        Err(err) => return Err(err.into()),
    };
    remove_stale_tmp(dir)?;
    let mut seen: HashSet<Vec<u8>> = HashSet::new();
    let mut changed = false;
    for generation in generations {
        let age_ok = plan.now_ms >= generation.stamp && plan.now_ms - generation.stamp >= plan.grace_ms;
        if !age_ok {
            report.waiting += 1;
            continue;
        }
        let file = match open_regular(&generation.path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => {
                report.refused += 1;
                continue;
            }
        };
        report.eligible += 1;
        let mut lines = Vec::new();
        scan(file, |raw, class| {
            lines.push(Line {
                raw: raw.to_vec(),
                class,
            });
        })?;
        if lines.iter().any(|line| matches!(line.class, Class::NewerVersion)) {
            report.held_newer_version += 1;
            continue;
        }
        let (mut kept, mut dropped, mut duplicates, mut damaged) = (Vec::new(), 0, 0, 0);
        for line in &lines {
            match &line.class {
                Class::Record(record) if !(plan.keep)(record) => dropped += 1,
                Class::Record(_) if !seen.insert(line.raw.clone()) => duplicates += 1,
                Class::Record(_) | Class::UnknownKind => kept.push(line.raw.as_slice()),
                Class::Malformed | Class::Torn | Class::NewerVersion => damaged += 1,
            }
        }
        report.dropped_records += dropped;
        report.duplicates_removed += duplicates;
        if dropped + duplicates == 0 {
            report.untouched += 1;
            continue;
        }
        report.dropped_damaged_lines += damaged;
        if kept.is_empty() {
            let guard = swap()?;
            fs::remove_file(&generation.path)?;
            drop(guard);
            report.deleted += 1;
        } else {
            write_tmp(dir, &kept, framing)?;
            let tmp = dir.join(COMPACT_TMP);
            let guard = match swap() {
                Ok(guard) => guard,
                Err(err) => {
                    let _ = fs::remove_file(&tmp);
                    return Err(err);
                }
            };
            fs::rename(&tmp, &generation.path)?;
            drop(guard);
            report.rewritten += 1;
        }
        changed = true;
    }
    if changed {
        sync_dir(dir)?;
    }
    Ok(report)
}

fn remove_stale_tmp(dir: &Path) -> io::Result<()> {
    let path = dir.join(COMPACT_TMP);
    match fs::symlink_metadata(&path) {
        Ok(meta) if !meta.is_dir() => fs::remove_file(&path),
        Ok(_) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

fn write_tmp(dir: &Path, kept: &[&[u8]], framing: Framing) -> io::Result<()> {
    let file = open_private(&dir.join(COMPACT_TMP), |options| {
        options.write(true).create_new(true)
    })?;
    let mut out = BufWriter::new(file);
    for raw in kept {
        out.write_all(&wrap(raw, framing))?;
    }
    let file = out.into_inner().map_err(io::IntoInnerError::into_error)?;
    file.sync_all()
}
