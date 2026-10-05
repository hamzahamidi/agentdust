use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::frame::{Class, RS, scan};
use super::generations::list_generations;
use super::maintenance::{
    COMPACT_TMP, CORRUPT_PREFIX, MaintenanceError, MaintenancePoint, MaintenanceProbe, refuse,
};
use super::retention::{self, Degraded, Entry, Policy};
use super::{ACTIVE_FILE, Record};
use crate::safe_open::{self, Access, SafeOpenError};

const COPY_ATTEMPTS: u64 = 1000;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RetainReport {
    pub generations: usize,
    pub held_newer_version: usize,
    pub untouched: usize,
    pub rewritten: usize,
    pub deleted: usize,
    pub kept_records: usize,
    pub dropped_earlier_boot: usize,
    pub dropped_aged: usize,
    pub dropped_over_cap: usize,
    pub dropped_pinned: usize,
    pub duplicates_removed: usize,
    pub dropped_damaged_lines: usize,
    pub malformed_lines: usize,
    pub torn_frames: usize,
    pub unknown_kind_lines: usize,
    pub newer_version_lines: usize,
    pub corrupt_copies: Vec<PathBuf>,
    pub degraded: Vec<Degraded>,
}

pub(super) struct Source {
    path: PathBuf,
    stamp: Option<u64>,
    file: File,
}

enum Slot {
    Record(usize),
    Duplicate,
    Unknown,
    Malformed,
    Torn,
}

struct Line {
    raw: Rc<[u8]>,
    slot: Slot,
}

struct Loaded {
    source: Source,
    droppable: bool,
    lines: Vec<Line>,
}

struct Parsed {
    record: Record,
    bytes: u64,
    droppable: bool,
}

pub(super) fn open_sources(dir: &Path) -> Result<Vec<Source>, MaintenanceError> {
    let mut sources = Vec::new();
    for generation in list_generations(dir)? {
        match safe_open::open_file(&generation.path, Access::Read) {
            Ok(file) => sources.push(Source {
                path: generation.path,
                stamp: Some(generation.stamp),
                file,
            }),
            Err(SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(refuse(&generation.path, err)),
        }
    }
    let active = dir.join(ACTIVE_FILE);
    match safe_open::open_file(&active, Access::Read) {
        Ok(file) => sources.push(Source {
            path: active,
            stamp: None,
            file,
        }),
        Err(SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(refuse(&active, err)),
    }
    Ok(sources)
}

pub(super) fn remove_stale_tmp(dir: &Path) -> io::Result<()> {
    let path = dir.join(COMPACT_TMP);
    match fs::symlink_metadata(&path) {
        Ok(meta) if !meta.is_dir() => fs::remove_file(&path),
        Ok(_) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

pub(super) fn retain_locked(
    dir: &Path,
    dir_handle: &File,
    sources: Vec<Source>,
    policy: &Policy,
    now_ms: u64,
    current_boot: &str,
    probe: &dyn MaintenanceProbe,
) -> Result<RetainReport, MaintenanceError> {
    let mut report = RetainReport::default();
    let (loaded, parsed) = load(sources, &mut report)?;
    let entries: Vec<Entry<'_>> = parsed
        .iter()
        .map(|parsed| Entry {
            record: &parsed.record,
            bytes: parsed.bytes,
            droppable: parsed.droppable,
        })
        .collect();
    let plan = retention::plan(&entries, policy, now_ms, current_boot);
    report.kept_records = plan.keep.iter().filter(|keep| **keep).count();
    report.dropped_earlier_boot = plan.dropped_earlier_boot;
    report.dropped_aged = plan.dropped_aged;
    report.dropped_over_cap = plan.dropped_over_cap;
    report.dropped_pinned = plan.dropped_pinned;
    report.degraded = plan.degraded.clone();
    probe.reached(MaintenancePoint::Planned);
    apply(dir, dir_handle, &loaded, &plan.keep, now_ms, &mut report)?;
    Ok(report)
}

fn load(
    sources: Vec<Source>,
    report: &mut RetainReport,
) -> Result<(Vec<Loaded>, Vec<Parsed>), MaintenanceError> {
    let mut parsed: Vec<Parsed> = Vec::new();
    let mut loaded: Vec<Loaded> = Vec::new();
    let mut seen: HashSet<Rc<[u8]>> = HashSet::new();
    for source in sources {
        let mut scanned: Vec<(Rc<[u8]>, Class)> = Vec::new();
        scan(&source.file, |raw, class| scanned.push((Rc::from(raw), class)))?;
        let held = scanned
            .iter()
            .any(|(_, class)| matches!(class, Class::NewerVersion));
        let droppable = source.stamp.is_some() && !held;
        if source.stamp.is_some() {
            report.generations += 1;
            report.held_newer_version += usize::from(held);
        }
        let mut lines = Vec::new();
        for (raw, class) in scanned {
            let slot = match class {
                Class::Record(record) => {
                    if droppable && seen.contains(&raw) {
                        Slot::Duplicate
                    } else {
                        seen.insert(Rc::clone(&raw));
                        parsed.push(Parsed {
                            record: *record,
                            bytes: raw.len() as u64 + 2,
                            droppable,
                        });
                        Slot::Record(parsed.len() - 1)
                    }
                }
                Class::UnknownKind => {
                    report.unknown_kind_lines += 1;
                    Slot::Unknown
                }
                Class::Malformed => {
                    report.malformed_lines += 1;
                    Slot::Malformed
                }
                Class::Torn { .. } => {
                    report.torn_frames += 1;
                    Slot::Torn
                }
                Class::NewerVersion => {
                    report.newer_version_lines += 1;
                    continue;
                }
            };
            if droppable {
                lines.push(Line { raw, slot });
            }
        }
        loaded.push(Loaded {
            source,
            droppable,
            lines,
        });
    }
    Ok((loaded, parsed))
}

fn apply(
    dir: &Path,
    dir_handle: &File,
    loaded: &[Loaded],
    keep: &[bool],
    now_ms: u64,
    report: &mut RetainReport,
) -> Result<(), MaintenanceError> {
    let mut changed = false;
    for file in loaded.iter().filter(|file| file.droppable) {
        let mut kept: Vec<&[u8]> = Vec::new();
        let (mut dropped, mut duplicates, mut malformed, mut torn) = (0, 0, 0, 0);
        for line in &file.lines {
            match line.slot {
                Slot::Record(index) if keep[index] => kept.push(&line.raw),
                Slot::Record(_) => dropped += 1,
                Slot::Duplicate => duplicates += 1,
                Slot::Unknown => kept.push(&line.raw),
                Slot::Malformed => malformed += 1,
                Slot::Torn => torn += 1,
            }
        }
        if dropped + duplicates == 0 {
            report.untouched += 1;
            continue;
        }
        report.duplicates_removed += duplicates;
        report.dropped_damaged_lines += malformed + torn;
        if malformed > 0 {
            report
                .corrupt_copies
                .push(copy_aside(dir, &file.source.file, now_ms)?);
            dir_handle.sync_all()?;
        }
        if kept.is_empty() {
            fs::remove_file(&file.source.path)?;
            report.deleted += 1;
        } else {
            let tmp = write_tmp(dir, &kept)?;
            fs::rename(&tmp, &file.source.path)?;
            report.rewritten += 1;
        }
        changed = true;
    }
    if changed {
        dir_handle.sync_all()?;
    }
    Ok(())
}

fn copy_aside(dir: &Path, source: &File, now_ms: u64) -> Result<PathBuf, MaintenanceError> {
    let mut number = now_ms;
    for _ in 0..COPY_ATTEMPTS {
        let path = dir.join(format!("{CORRUPT_PREFIX}{number}"));
        match safe_open::open_file(&path, Access::Create) {
            Ok(mut copy) => {
                let written = (|| {
                    let mut source = source;
                    source.seek(SeekFrom::Start(0))?;
                    io::copy(&mut source, &mut copy)?;
                    copy.sync_all()
                })();
                return match written {
                    Ok(()) => Ok(path),
                    Err(err) => {
                        let _ = fs::remove_file(&path);
                        Err(err.into())
                    }
                };
            }
            Err(SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::AlreadyExists => {
                number = number.saturating_add(1);
            }
            Err(err) => return Err(refuse(&path, err)),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "every corrupt copy name in range is taken",
    )
    .into())
}

fn write_tmp(dir: &Path, kept: &[&[u8]]) -> Result<PathBuf, MaintenanceError> {
    let path = dir.join(COMPACT_TMP);
    let file = safe_open::open_file(&path, Access::Create).map_err(|err| refuse(&path, err))?;
    let written = (|| {
        let mut out = BufWriter::new(file);
        for raw in kept {
            out.write_all(&[RS])?;
            out.write_all(raw)?;
            out.write_all(b"\n")?;
        }
        let file = out.into_inner().map_err(io::IntoInnerError::into_error)?;
        file.sync_all()
    })();
    match written {
        Ok(()) => Ok(path),
        Err(err) => {
            let _ = fs::remove_file(&path);
            Err(err.into())
        }
    }
}
