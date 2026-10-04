use std::ffi::CString;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use thiserror::Error;

use crate::digest::to_hex;
use crate::entropy;
use crate::journal::volume::{self, FsFacts, SystemVolume, VolumeProbe};
use crate::safe_open::{self, Access, SafeOpenError};

pub const SECRET_FILE: &str = "install.secret";
pub const SECRET_LEN: usize = 32;
const TEMP_NAME_LEN: usize = 8;

pub struct Secret([u8; SECRET_LEN]);

impl Secret {
    pub fn from_bytes(bytes: [u8; SECRET_LEN]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; SECRET_LEN] {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("Secret(redacted)")
    }
}

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("refused to open the install secret: {0}")]
    Refused(SafeOpenError),
    #[error("the install secret has {found} bytes and must have {SECRET_LEN}")]
    WrongSize { found: u64 },
    #[error("the data directory is on a volume that is not supported: {}", .0.describe())]
    UnsupportedFilesystem(FsFacts),
    #[error("the operating system gave no random bytes: {0}")]
    Entropy(io::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl From<SafeOpenError> for SecretError {
    fn from(err: SafeOpenError) -> Self {
        match err {
            SafeOpenError::Io(err) => Self::Io(err),
            refused => Self::Refused(refused),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallPoint {
    Absent,
    TempSynced,
}

pub trait InstallProbe: Sync {
    fn reached(&self, point: InstallPoint);
}

pub struct NoProbe;

impl InstallProbe for NoProbe {
    fn reached(&self, _point: InstallPoint) {}
}

pub fn load_or_create(dir: &Path) -> Result<Secret, SecretError> {
    load_or_create_with(dir, &SystemVolume, &NoProbe)
}

pub fn load_or_create_with(
    dir: &Path,
    volume: &dyn VolumeProbe,
    probe: &dyn InstallProbe,
) -> Result<Secret, SecretError> {
    let facts = volume::locate(volume, dir)?;
    if !facts.supported {
        return Err(SecretError::UnsupportedFilesystem(facts));
    }
    safe_open::ensure_dir(dir)?;
    let path = dir.join(SECRET_FILE);
    match read_secret(&path) {
        Err(SecretError::Io(err)) if err.kind() == io::ErrorKind::NotFound => {}
        found => return found,
    }
    probe.reached(InstallPoint::Absent);
    install(dir, &path, probe)?;
    read_secret(&path)
}

pub fn load_existing(dir: &Path) -> Result<Secret, SecretError> {
    safe_open::check_dir(dir)?;
    read_secret(&dir.join(SECRET_FILE))
}

fn read_secret(path: &Path) -> Result<Secret, SecretError> {
    let mut file = safe_open::open_file(path, Access::Read)?;
    let len = file.metadata()?.len();
    if len != SECRET_LEN as u64 {
        return Err(SecretError::WrongSize { found: len });
    }
    let mut bytes = [0u8; SECRET_LEN];
    file.read_exact(&mut bytes)?;
    Ok(Secret(bytes))
}

fn install(dir: &Path, path: &Path, probe: &dyn InstallProbe) -> Result<(), SecretError> {
    let drawn = random_bytes::<{ SECRET_LEN + TEMP_NAME_LEN }>()?;
    let (secret, name) = drawn.split_at(SECRET_LEN);
    let temp = dir.join(format!("{SECRET_FILE}.{}.tmp", to_hex(name)));
    let mut file = safe_open::open_file(&temp, Access::Create)?;
    let written = file.write_all(secret).and_then(|()| file.sync_all());
    drop(file);
    if written.is_ok() {
        probe.reached(InstallPoint::TempSynced);
    }
    match written.and_then(|()| rename_exclusive(&temp, path)) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&temp);
            match err.kind() {
                io::ErrorKind::AlreadyExists => Ok(()),
                _ => Err(err.into()),
            }
        }
    }
}

fn random_bytes<const N: usize>() -> Result<[u8; N], SecretError> {
    entropy::bytes::<N>().map_err(SecretError::Entropy)
}

#[cfg(target_os = "macos")]
fn rename_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    let (from, to) = (c_path(from)?, c_path(to)?);
    // SAFETY: both pointers are NUL terminated paths that outlive the call.
    let status = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    status_to_result(status)
}

#[cfg(target_os = "linux")]
fn rename_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    let (from, to) = (c_path(from)?, c_path(to)?);
    // SAFETY: both pointers are NUL terminated paths that outlive the call.
    let status = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    status_to_result(status)
}

fn status_to_result(status: libc::c_int) -> io::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))
}
