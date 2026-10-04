use std::fs::{DirBuilder, File, Metadata, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::Path;

use thiserror::Error;

const FILE_MODE: u32 = 0o600;
const DIR_MODE: u32 = 0o700;
const PERMISSION_BITS: u32 = 0o7777;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Append,
    Create,
}

#[derive(Debug, Error)]
pub enum SafeOpenError {
    #[error("the path is a symbolic link")]
    Symlink,
    #[error("the path is not a regular file")]
    NotRegular,
    #[error("the path is not a directory")]
    NotDirectory,
    #[error("the file has {links} hard links")]
    HardLinked { links: u64 },
    #[error("owned by uid {found}, expected uid {expected}")]
    ForeignOwner { found: u32, expected: u32 },
    #[error("mode {mode:o} is looser than {allowed:o}")]
    LooseMode { mode: u32, allowed: u32 },
    #[error(transparent)]
    Io(#[from] io::Error),
}

enum Shape {
    File,
    UserFile,
    Dir,
}

pub fn open_file(path: &Path, access: Access) -> Result<File, SafeOpenError> {
    open_file_as(path, access, current_uid())
}

pub fn open_file_as(path: &Path, access: Access, owner: u32) -> Result<File, SafeOpenError> {
    let file = open_without_following(path, access)?;
    verify(&file.metadata()?, Shape::File, owner)?;
    Ok(file)
}

pub fn open_user_file(path: &Path) -> Result<File, SafeOpenError> {
    open_user_file_as(path, current_uid())
}

pub fn open_user_file_as(path: &Path, owner: u32) -> Result<File, SafeOpenError> {
    let file = open_without_following(path, Access::Read)?;
    verify(&file.metadata()?, Shape::UserFile, owner)?;
    Ok(file)
}

pub fn check_dir(path: &Path) -> Result<(), SafeOpenError> {
    check_dir_as(path, current_uid())
}

pub fn check_dir_as(path: &Path, owner: u32) -> Result<(), SafeOpenError> {
    open_dir_as(path, owner).map(drop)
}

pub fn open_dir(path: &Path) -> Result<File, SafeOpenError> {
    open_dir_as(path, current_uid())
}

pub fn open_dir_as(path: &Path, owner: u32) -> Result<File, SafeOpenError> {
    let dir = open_without_following(path, Access::Read)?;
    verify(&dir.metadata()?, Shape::Dir, owner)?;
    Ok(dir)
}

pub fn ensure_dir(path: &Path) -> Result<(), SafeOpenError> {
    match check_dir(path) {
        Err(SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => {
            DirBuilder::new().recursive(true).mode(DIR_MODE).create(path)?;
            check_dir(path)
        }
        checked => checked,
    }
}

fn open_without_following(path: &Path, access: Access) -> Result<File, SafeOpenError> {
    let mut options = OpenOptions::new();
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    match access {
        Access::Read => options.read(true),
        Access::Append => options.append(true),
        Access::Create => options.write(true).create_new(true).mode(FILE_MODE),
    };
    options.open(path).map_err(|err| match err.raw_os_error() {
        Some(libc::ELOOP) => SafeOpenError::Symlink,
        Some(libc::ENXIO | libc::EISDIR) => SafeOpenError::NotRegular,
        _ => SafeOpenError::Io(err),
    })
}

fn verify(metadata: &Metadata, shape: Shape, owner: u32) -> Result<(), SafeOpenError> {
    let allowed = match shape {
        Shape::File | Shape::UserFile => {
            if !metadata.file_type().is_file() {
                return Err(SafeOpenError::NotRegular);
            }
            if metadata.nlink() > 1 {
                return Err(SafeOpenError::HardLinked {
                    links: metadata.nlink(),
                });
            }
            if matches!(shape, Shape::UserFile) {
                PERMISSION_BITS
            } else {
                FILE_MODE
            }
        }
        Shape::Dir => {
            if !metadata.file_type().is_dir() {
                return Err(SafeOpenError::NotDirectory);
            }
            DIR_MODE
        }
    };
    if metadata.uid() != owner {
        return Err(SafeOpenError::ForeignOwner {
            found: metadata.uid(),
            expected: owner,
        });
    }
    let mode = metadata.mode() & PERMISSION_BITS;
    if mode & !allowed != 0 {
        return Err(SafeOpenError::LooseMode { mode, allowed });
    }
    Ok(())
}

fn current_uid() -> u32 {
    // SAFETY: geteuid takes no arguments and cannot fail.
    unsafe { libc::geteuid() }
}
