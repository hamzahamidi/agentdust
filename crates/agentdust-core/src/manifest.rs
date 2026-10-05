use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::atomic::write_atomic;
use crate::safe_open::{self, Access, SafeOpenError};

pub const MANIFEST_FILE: &str = "manifest.json";
pub const MANIFEST_VERSION: u32 = 1;
const FILE_MODE: u32 = 0o600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resource {
    Hook,
    McpServer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Created,
    PreExisting,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub resource: Resource,
    pub target: String,
    pub key: String,
    pub origin: Origin,
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    pub hash: String,
    #[serde(default)]
    pub created_hooks_key: bool,
    #[serde(default)]
    pub created_event_key: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    pub entries: Vec<Entry>,
    #[serde(default)]
    pub agent_executables: BTreeMap<String, String>,
}

impl Manifest {
    pub fn new() -> Self {
        Self {
            version: MANIFEST_VERSION,
            entries: Vec::new(),
            agent_executables: BTreeMap::new(),
        }
    }
}

impl Default for Manifest {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("refused to open the manifest: {0}")]
    Refused(SafeOpenError),
    #[error("the manifest has version {found} and this build reads version {MANIFEST_VERSION}")]
    UnknownVersion { found: u64 },
    #[error("the manifest cannot be read: {0}")]
    Corrupt(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl From<SafeOpenError> for ManifestError {
    fn from(err: SafeOpenError) -> Self {
        match err {
            SafeOpenError::Io(err) => Self::Io(err),
            refused => Self::Refused(refused),
        }
    }
}

pub fn load(dir: &Path) -> Result<Option<Manifest>, ManifestError> {
    match safe_open::check_dir(dir) {
        Ok(()) => {}
        Err(SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    }
    let mut file = match safe_open::open_file(&dir.join(MANIFEST_FILE), Access::Read) {
        Ok(file) => file,
        Err(SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    parse(&bytes).map(Some)
}

fn parse(bytes: &[u8]) -> Result<Manifest, ManifestError> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|err| ManifestError::Corrupt(err.to_string()))?;
    let version = value.get("version").and_then(Value::as_u64).ok_or_else(|| {
        ManifestError::Corrupt("the version is missing or is not a whole number".to_owned())
    })?;
    if version != u64::from(MANIFEST_VERSION) {
        return Err(ManifestError::UnknownVersion { found: version });
    }
    serde_json::from_value(value).map_err(|err| ManifestError::Corrupt(err.to_string()))
}

pub fn store(dir: &Path, manifest: &Manifest) -> Result<(), ManifestError> {
    safe_open::ensure_dir(dir)?;
    let path = dir.join(MANIFEST_FILE);
    match safe_open::open_file(&path, Access::Read) {
        Ok(_) => {}
        Err(SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    let mut text =
        serde_json::to_vec_pretty(manifest).map_err(|err| ManifestError::Corrupt(err.to_string()))?;
    text.push(b'\n');
    write_atomic(&path, &text, FILE_MODE)?;
    Ok(())
}

pub fn delete(dir: &Path) -> Result<(), ManifestError> {
    let path = dir.join(MANIFEST_FILE);
    match safe_open::open_file(&path, Access::Read) {
        Ok(_) => Ok(fs::remove_file(&path)?),
        Err(SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}
