use std::fs::{self, File, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::safe_open::{self, Access, SafeOpenError};

static COUNTER: AtomicU32 = AtomicU32::new(0);

pub(crate) fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<(), SafeOpenError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the path has no file name"))?;
    let temp = parent.join(format!(
        ".{}.{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = safe_open::open_file(&temp, Access::Create)?;
    let written = write_and_sync(&mut file, bytes, mode);
    drop(file);
    let renamed = written.and_then(|()| fs::rename(&temp, path));
    if let Err(err) = renamed {
        let _ = fs::remove_file(&temp);
        return Err(err.into());
    }
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn write_and_sync(file: &mut File, bytes: &[u8], mode: u32) -> io::Result<()> {
    file.write_all(bytes)?;
    file.set_permissions(Permissions::from_mode(mode))?;
    file.sync_all()
}
