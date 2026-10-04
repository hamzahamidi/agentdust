use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use super::generations::list_generations;
use super::{ACTIVE_FILE, JournalError, ReadPoint, ReadProbe, ReadReport, Record, VolumeProbe, decode};
use crate::safe_open::{self, Access, SafeOpenError};

pub(super) fn read(
    dir: &Path,
    volume: &dyn VolumeProbe,
    probe: &dyn ReadProbe,
) -> Result<ReadReport, JournalError> {
    match safe_open::check_dir(dir) {
        Ok(()) => {}
        Err(SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(ReadReport::default());
        }
        Err(err) => return Err(err.into()),
    }
    let active = match safe_open::open_file(&dir.join(ACTIVE_FILE), Access::Read) {
        Ok(file) => Some(file),
        Err(SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(err.into()),
    };
    probe.reached(ReadPoint::ActiveOpened);
    let generations = list_generations(dir)?;
    probe.reached(ReadPoint::GenerationsListed);
    let mut report = ReadReport::default();
    let mut held = HashSet::new();
    let mut files = Vec::new();
    if let Some(file) = active {
        held.insert(identity(&file)?);
        files.push(file);
    }
    for generation in generations {
        match safe_open::open_file(&generation.path, Access::Read) {
            Ok(file) => {
                if held.insert(identity(&file)?) {
                    files.push(file);
                }
            }
            Err(SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => {}
            Err(SafeOpenError::Io(err)) => return Err(err.into()),
            Err(_) => report.unsafe_files += 1,
        }
    }
    for file in files {
        let decoded = decode(file)?;
        report.records.extend(decoded.records);
        report.malformed_lines += decoded.malformed_lines;
        report.torn_frames += decoded.torn_frames;
        report.truncated_last_line |= decoded.truncated_last_line;
        report.newer_version_lines += decoded.newer_version_lines;
        report.unknown_kind_lines += decoded.unknown_kind_lines;
        report.unsupported_version |= decoded.unsupported_version;
    }
    report.filesystem = volume.probe(dir).ok();
    order(&mut report);
    Ok(report)
}

fn identity(file: &File) -> io::Result<(u64, u64)> {
    let meta = file.metadata()?;
    Ok((meta.dev(), meta.ino()))
}

fn order(report: &mut ReadReport) {
    let records = std::mem::take(&mut report.records);
    let rank = boot_rank(&records);
    let mut keyed: Vec<(usize, u64, u64, Record)> = records
        .into_iter()
        .map(|record| (rank[record.boot.as_str()], record.mono_ts, record.wall_ts, record))
        .collect();
    keyed.sort_unstable();
    let before = keyed.len();
    keyed.dedup();
    report.duplicates_removed = before - keyed.len();
    report.records = keyed.into_iter().map(|(_, _, _, record)| record).collect();
}

fn boot_rank(records: &[Record]) -> HashMap<String, usize> {
    let mut first_wall: HashMap<&str, u64> = HashMap::new();
    for record in records {
        first_wall
            .entry(record.boot.as_str())
            .and_modify(|first| *first = (*first).min(record.wall_ts))
            .or_insert(record.wall_ts);
    }
    let mut boots: Vec<(u64, &str)> = first_wall.into_iter().map(|(boot, wall)| (wall, boot)).collect();
    boots.sort_unstable();
    boots
        .into_iter()
        .enumerate()
        .map(|(position, (_, boot))| (boot.to_owned(), position))
        .collect()
}
