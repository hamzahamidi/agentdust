use std::fs::{self, DirBuilder};
use std::io::{self, Read};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::Path;

use thiserror::Error;

use crate::atomic::write_atomic;
use crate::safe_open::{self, SafeOpenError};

const NEW_FILE_MODE: u32 = 0o600;
const NEW_DIR_MODE: u32 = 0o700;
const PERMISSION_BITS: u32 = 0o7777;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub bytes: Vec<u8>,
    pub mode: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loaded {
    Missing,
    Present(Snapshot),
}

impl Loaded {
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Missing => None,
            Self::Present(snapshot) => Some(&snapshot.bytes),
        }
    }
}

#[derive(Debug, Error)]
pub enum UserFileError {
    #[error("refused to edit the file: {0}")]
    Refused(SafeOpenError),
    #[error("the file changed after it was read")]
    Changed,
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl From<SafeOpenError> for UserFileError {
    fn from(err: SafeOpenError) -> Self {
        match err {
            SafeOpenError::Io(err) => Self::Io(err),
            refused => Self::Refused(refused),
        }
    }
}

pub fn load(path: &Path) -> Result<Loaded, UserFileError> {
    let mut file = match safe_open::open_user_file(path) {
        Ok(file) => file,
        Err(SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => return Ok(Loaded::Missing),
        Err(err) => return Err(err.into()),
    };
    let mode = file.metadata()?.mode() & PERMISSION_BITS;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(Loaded::Present(Snapshot { bytes, mode }))
}

pub fn replace(path: &Path, expected: &Loaded, bytes: &[u8]) -> Result<(), UserFileError> {
    let current = load(path)?;
    if current.bytes() != expected.bytes() {
        return Err(UserFileError::Changed);
    }
    let mode = match &current {
        Loaded::Missing => {
            if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
                DirBuilder::new()
                    .recursive(true)
                    .mode(NEW_DIR_MODE)
                    .create(parent)?;
            }
            NEW_FILE_MODE
        }
        Loaded::Present(snapshot) => snapshot.mode,
    };
    write_atomic(path, bytes, mode)?;
    Ok(())
}

pub fn delete_if_unchanged(path: &Path, expected: &Loaded) -> Result<(), UserFileError> {
    let current = load(path)?;
    if current.bytes() != expected.bytes() {
        return Err(UserFileError::Changed);
    }
    match current {
        Loaded::Missing => Ok(()),
        Loaded::Present(_) => Ok(fs::remove_file(path)?),
    }
}
