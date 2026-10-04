use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generation {
    pub stamp: u64,
    pub path: PathBuf,
}

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
