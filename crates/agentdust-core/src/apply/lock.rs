use std::fs::{self, File};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::safe_open::{self, SafeOpenError};

const MAX_NAME_LEN: usize = 64;
const ATTEMPTS: usize = 3;
const SUFFIX: &str = ".lock";

#[derive(Debug, Error)]
pub enum LockError {
    #[error("the lock name is not a plain identity name")]
    InvalidName,
    #[error("refused to open a lock file: {0}")]
    Refused(SafeOpenError),
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl From<SafeOpenError> for LockError {
    fn from(err: SafeOpenError) -> Self {
        match err {
            SafeOpenError::Io(err) => Self::Io(err),
            refused => Self::Refused(refused),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockPoint {
    Opened,
}

pub trait LockProbe {
    fn reached(&self, point: LockPoint);
}

struct NoProbe;

impl LockProbe for NoProbe {
    fn reached(&self, _point: LockPoint) {}
}

#[derive(Debug)]
pub struct IdentityLock {
    file: File,
    path: PathBuf,
}

impl IdentityLock {
    pub fn try_acquire(dir: &Path, name: &str) -> Result<Option<IdentityLock>, LockError> {
        Self::try_acquire_with(dir, name, &NoProbe)
    }

    pub fn try_acquire_with(
        dir: &Path,
        name: &str,
        probe: &dyn LockProbe,
    ) -> Result<Option<IdentityLock>, LockError> {
        if !plain_name(name) {
            return Err(LockError::InvalidName);
        }
        safe_open::ensure_dir(dir)?;
        let path = dir.join(format!("{name}{SUFFIX}"));
        for _ in 0..ATTEMPTS {
            let file = safe_open::open_lock_file(&path)?;
            probe.reached(LockPoint::Opened);
            if !try_flock(&file)? {
                return Ok(None);
            }
            if names_this_file(&file, &path) {
                return Ok(Some(IdentityLock { file, path }));
            }
        }
        Ok(None)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for IdentityLock {
    fn drop(&mut self) {
        if names_this_file(&self.file, &self.path) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn plain_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= MAX_NAME_LEN
        && bytes.next().is_some_and(|first| first.is_ascii_alphanumeric())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn names_this_file(file: &File, path: &Path) -> bool {
    let (Ok(held), Ok(named)) = (file.metadata(), fs::symlink_metadata(path)) else {
        return false;
    };
    named.file_type().is_file() && held.dev() == named.dev() && held.ino() == named.ino()
}

fn try_flock(file: &File) -> io::Result<bool> {
    loop {
        // SAFETY: the descriptor belongs to `file`, which is open for the duration of the call.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(true);
        }
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EWOULDBLOCK) => return Ok(false),
            Some(libc::EINTR) => {}
            _ => return Err(err),
        }
    }
}
