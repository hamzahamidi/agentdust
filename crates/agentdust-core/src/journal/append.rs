use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use super::volume::{self, VolumeProbe};
use super::{ACTIVE_FILE, JournalError, Record, frame};
use crate::safe_open::{self, Access, SafeOpenError};

pub const MAX_ATTEMPTS: u32 = 3;
const OPEN_ROUNDS: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Appended {
    pub attempts: u32,
}

pub trait FrameWriter {
    fn write(&mut self, file: &File, frame: &[u8]) -> io::Result<usize>;

    fn created(&mut self, _path: &Path) {}
}

pub struct SystemWriter;

impl FrameWriter for SystemWriter {
    fn write(&mut self, mut file: &File, frame: &[u8]) -> io::Result<usize> {
        file.write(frame)
    }
}

pub(super) fn append(
    dir: &Path,
    volume: &dyn VolumeProbe,
    record: &Record,
    writer: &mut dyn FrameWriter,
) -> Result<Appended, JournalError> {
    let frame = frame::encode(record)?;
    let facts = volume::locate(volume, dir)?;
    if !facts.supported {
        return Err(JournalError::UnsupportedFilesystem(facts));
    }
    safe_open::ensure_dir(dir)?;
    let path = dir.join(ACTIVE_FILE);
    for attempt in 1..=MAX_ATTEMPTS {
        let file = open_for_append(&path, writer)?;
        write_once(writer, &file, &frame)?;
        if still_active(&file, &path)? {
            return Ok(Appended { attempts: attempt });
        }
    }
    Err(JournalError::Stale {
        attempts: MAX_ATTEMPTS,
    })
}

fn open_for_append(path: &Path, writer: &mut dyn FrameWriter) -> Result<File, SafeOpenError> {
    for _ in 0..OPEN_ROUNDS {
        match safe_open::open_file(path, Access::Append) {
            Err(SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => {
                match safe_open::open_file(path, Access::Create) {
                    Ok(_) => writer.created(path),
                    Err(SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(err) => return Err(err),
                }
            }
            opened => return opened,
        }
    }
    safe_open::open_file(path, Access::Append)
}

fn write_once(writer: &mut dyn FrameWriter, file: &File, frame: &[u8]) -> Result<(), JournalError> {
    loop {
        match writer.write(file, frame) {
            Ok(written) if written == frame.len() => return Ok(()),
            Ok(written) => {
                return Err(JournalError::ShortWrite {
                    written,
                    expected: frame.len(),
                });
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err.into()),
        }
    }
}

fn still_active(file: &File, path: &Path) -> io::Result<bool> {
    let opened = file.metadata()?;
    if opened.nlink() == 0 {
        return Ok(false);
    }
    match fs::symlink_metadata(path) {
        Ok(named) => Ok(named.dev() == opened.dev() && named.ino() == opened.ino()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}
