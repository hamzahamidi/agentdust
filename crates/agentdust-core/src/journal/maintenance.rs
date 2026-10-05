use std::fs::{self, File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

use super::ACTIVE_FILE;
use super::generations::generation_path;
use super::retain::{self, RetainReport};
use super::retention::Policy;
use super::volume::{self, FsFacts, VolumeProbe};
use crate::safe_open::{self, Access, SafeOpenError};

pub const MAINT_FILE: &str = "journal.maint";
pub const COMPACT_TMP: &str = "journal.compact.tmp";
pub const CORRUPT_PREFIX: &str = "journal.jsonl.corrupt-";

#[derive(Debug, Error)]
pub enum MaintenanceError {
    #[error("another rotation or retention run holds {MAINT_FILE}")]
    Busy,
    #[error("the current boot is empty, so every record would look like an earlier boot")]
    EmptyBoot,
    #[error("the journal directory is on a volume that is not supported: {}", .0.describe())]
    UnsupportedFilesystem(FsFacts),
    #[error("refused to touch {}: {source}", path.display())]
    Refused { path: PathBuf, source: SafeOpenError },
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    Rotated { stamp: u64 },
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PruneReport {
    pub rotation: Rotation,
    pub retained: RetainReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenancePoint {
    Locked,
    Rotated,
    Planned,
}

pub trait MaintenanceProbe: Sync {
    fn reached(&self, point: MaintenancePoint);
}

pub struct NoMaintenanceProbe;

impl MaintenanceProbe for NoMaintenanceProbe {
    fn reached(&self, _point: MaintenancePoint) {}
}

pub(super) fn refuse(path: &Path, err: SafeOpenError) -> MaintenanceError {
    match err {
        SafeOpenError::Io(err) => MaintenanceError::Io(err),
        source => MaintenanceError::Refused {
            path: path.to_path_buf(),
            source,
        },
    }
}

pub(super) fn rotate(
    dir: &Path,
    volume: &dyn VolumeProbe,
    probe: &dyn MaintenanceProbe,
    now_ms: u64,
) -> Result<Rotation, MaintenanceError> {
    let Some(dir_handle) = prepare(dir, volume)? else {
        return Ok(Rotation::Empty);
    };
    let _lock = lock(dir)?;
    probe.reached(MaintenancePoint::Locked);
    for generation in super::generations::list_generations(dir)? {
        if let Err(SafeOpenError::ExtendedAcl) = safe_open::open_file(&generation.path, Access::Read) {
            return Err(refuse(&generation.path, SafeOpenError::ExtendedAcl));
        }
    }
    rotate_locked(dir, &dir_handle, probe, now_ms)
}

pub(super) fn retain(
    dir: &Path,
    volume: &dyn VolumeProbe,
    probe: &dyn MaintenanceProbe,
    policy: &Policy,
    now_ms: u64,
    current_boot: &str,
) -> Result<RetainReport, MaintenanceError> {
    if current_boot.is_empty() {
        return Err(MaintenanceError::EmptyBoot);
    }
    let Some(dir_handle) = prepare(dir, volume)? else {
        return Ok(RetainReport::default());
    };
    let _lock = lock(dir)?;
    probe.reached(MaintenancePoint::Locked);
    let sources = retain::open_sources(dir)?;
    retain::remove_stale_tmp(dir)?;
    retain::retain_locked(dir, &dir_handle, sources, policy, now_ms, current_boot, probe)
}

pub(super) fn prune(
    dir: &Path,
    volume: &dyn VolumeProbe,
    probe: &dyn MaintenanceProbe,
    policy: &Policy,
    now_ms: u64,
    current_boot: &str,
) -> Result<PruneReport, MaintenanceError> {
    if current_boot.is_empty() {
        return Err(MaintenanceError::EmptyBoot);
    }
    let Some(dir_handle) = prepare(dir, volume)? else {
        return Ok(PruneReport {
            rotation: Rotation::Empty,
            retained: RetainReport::default(),
        });
    };
    let _lock = lock(dir)?;
    probe.reached(MaintenancePoint::Locked);
    drop(retain::open_sources(dir)?);
    retain::remove_stale_tmp(dir)?;
    let rotation = rotate_locked(dir, &dir_handle, probe, now_ms)?;
    let sources = retain::open_sources(dir)?;
    let retained = retain::retain_locked(dir, &dir_handle, sources, policy, now_ms, current_boot, probe)?;
    Ok(PruneReport { rotation, retained })
}

fn prepare(dir: &Path, volume: &dyn VolumeProbe) -> Result<Option<File>, MaintenanceError> {
    let handle = match safe_open::open_dir(dir) {
        Ok(handle) => handle,
        Err(SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(refuse(dir, err)),
    };
    let facts = volume::locate(volume, dir)?;
    if !facts.supported {
        return Err(MaintenanceError::UnsupportedFilesystem(facts));
    }
    Ok(Some(handle))
}

fn lock(dir: &Path) -> Result<File, MaintenanceError> {
    let path = dir.join(MAINT_FILE);
    match safe_open::open_file(&path, Access::Create) {
        Ok(_) => {}
        Err(SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::AlreadyExists => {}
        Err(err) => return Err(refuse(&path, err)),
    }
    let file = safe_open::open_file(&path, Access::Read).map_err(|err| refuse(&path, err))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(MaintenanceError::Busy),
        Err(TryLockError::Error(err)) => Err(err.into()),
    }
}

fn rotate_locked(
    dir: &Path,
    dir_handle: &File,
    probe: &dyn MaintenanceProbe,
    now_ms: u64,
) -> Result<Rotation, MaintenanceError> {
    let active = dir.join(ACTIVE_FILE);
    let file = match safe_open::open_file(&active, Access::Read) {
        Ok(file) => file,
        Err(SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(Rotation::Empty);
        }
        Err(err) => return Err(refuse(&active, err)),
    };
    if file.metadata()?.len() == 0 {
        return Ok(Rotation::Empty);
    }
    let (stamp, sealed) = free_stamp(dir, now_ms)?;
    fs::rename(&active, &sealed)?;
    dir_handle.sync_all()?;
    probe.reached(MaintenancePoint::Rotated);
    Ok(Rotation::Rotated { stamp })
}

fn free_stamp(dir: &Path, now_ms: u64) -> io::Result<(u64, PathBuf)> {
    let mut stamp = now_ms;
    loop {
        let path = generation_path(dir, stamp);
        match fs::symlink_metadata(&path) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok((stamp, path)),
            Err(err) => return Err(err),
            Ok(_) => {
                stamp = stamp.checked_add(1).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::AlreadyExists, "no free generation stamp")
                })?;
            }
        }
    }
}
