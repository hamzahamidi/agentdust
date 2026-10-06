use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::safe_open::SafeOpenError;

mod append;
mod fields;
mod frame;
mod generations;
mod maintenance;
mod retain;
pub mod retention;
mod store;
pub mod volume;

pub use append::{Appended, FrameWriter, MAX_ATTEMPTS, SystemWriter};
pub use fields::{
    AgentIdentity, CwdKey, ExeBase, FieldError, MAX_AGENT_IDENTITY_LEN, MAX_CWD_KEY_LEN, MAX_EXE_BASE_LEN,
    SESSION_TAG_KEY_LEN, SessionTagKey,
};
pub use frame::{Class, MAX_FRAME_LEN, RS, decode, encode, scan};
pub use generations::{Generation, generation_path, generation_stamp, list_generations};
pub use maintenance::{
    COMPACT_TMP, CORRUPT_PREFIX, MAINT_FILE, MaintenanceError, MaintenancePoint, MaintenanceProbe,
    NoMaintenanceProbe, PruneReport, Rotation,
};
pub use retain::RetainReport;
pub use volume::{FixedVolume, FsFacts, SystemVolume, VolumeProbe};

pub const SCHEMA_VERSION: u32 = 2;
pub const MIN_READABLE_SCHEMA_VERSION: u32 = 1;
pub const ACTIVE_FILE: &str = "journal.jsonl";
pub const RECORD_KEYS: [&str; 13] = [
    "v",
    "kind",
    "agent",
    "session_id",
    "subagent_id",
    "agent_identity",
    "tool_use_id",
    "wall_ts",
    "mono_ts",
    "boot",
    "session_tag_key",
    "cwd_key",
    "exe_base",
];
pub const AGENT_IDENTITY_KEYS: [&str; 4] = ["pid", "start_time_us", "uid", "exe_base"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    SessionStart,
    SessionEnd,
    SubagentStart,
    SubagentStop,
    SubagentAttributionUnknown,
    ShellStart,
    ShellEnd,
    Sample,
    ServerStart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Agent {
    Claude,
    Codex,
    Cursor,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Record {
    pub v: u32,
    pub kind: Kind,
    pub agent: Agent,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_identity: Option<AgentIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    pub wall_ts: u64,
    pub mono_ts: u64,
    pub boot: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_tag_key: Option<SessionTagKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd_key: Option<CwdKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exe_base: Option<ExeBase>,
}

#[derive(Debug, Error)]
pub enum JournalError {
    #[error("refused to open the journal: {0}")]
    Refused(SafeOpenError),
    #[error("the journal directory is on a volume that is not supported: {}", .0.describe())]
    UnsupportedFilesystem(FsFacts),
    #[error("the frame is {len} bytes and the limit is {max}")]
    TooLarge { len: usize, max: usize },
    #[error("the record has schema version {found} and this build writes version {SCHEMA_VERSION}")]
    WrongVersion { found: u32 },
    #[error("one write call took {written} of {expected} bytes")]
    ShortWrite { written: usize, expected: usize },
    #[error("the active file was replaced under {attempts} attempts in a row")]
    Stale { attempts: u32 },
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Encode(#[from] serde_json::Error),
}

impl From<SafeOpenError> for JournalError {
    fn from(err: SafeOpenError) -> Self {
        match err {
            SafeOpenError::Io(err) => Self::Io(err),
            refused => Self::Refused(refused),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReadReport {
    pub records: Vec<Record>,
    pub malformed_lines: usize,
    pub torn_frames: usize,
    pub truncated_last_line: bool,
    pub newer_version_lines: usize,
    pub unknown_kind_lines: usize,
    pub unsupported_version: bool,
    pub duplicates_removed: usize,
    pub unsafe_files: usize,
    pub filesystem: Option<FsFacts>,
}

impl ReadReport {
    pub fn skipped_lines(&self) -> usize {
        self.malformed_lines + self.torn_frames + self.newer_version_lines + self.unknown_kind_lines
    }

    pub fn on_unsupported_filesystem(&self) -> bool {
        self.filesystem.as_ref().is_some_and(|facts| !facts.supported)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadPoint {
    ActiveOpened,
    GenerationsListed,
}

pub trait ReadProbe: Sync {
    fn reached(&self, point: ReadPoint);
}

pub struct NoProbe;

impl ReadProbe for NoProbe {
    fn reached(&self, _point: ReadPoint) {}
}

#[derive(Clone, Copy)]
pub struct Journal<'a> {
    dir: &'a Path,
    volume: &'a dyn VolumeProbe,
    probe: &'a dyn MaintenanceProbe,
}

impl<'a> Journal<'a> {
    pub fn new(dir: &'a Path) -> Self {
        Self::with_volume(dir, &SystemVolume)
    }

    pub fn with_volume(dir: &'a Path, volume: &'a dyn VolumeProbe) -> Self {
        Self {
            dir,
            volume,
            probe: &NoMaintenanceProbe,
        }
    }

    pub fn with_probe(self, probe: &'a dyn MaintenanceProbe) -> Self {
        Self { probe, ..self }
    }

    pub fn append(&self, record: &Record) -> Result<Appended, JournalError> {
        self.append_with(record, &mut SystemWriter)
    }

    pub fn append_with(
        &self,
        record: &Record,
        writer: &mut dyn FrameWriter,
    ) -> Result<Appended, JournalError> {
        append::append(self.dir, self.volume, record, writer)
    }

    pub fn read(&self) -> Result<ReadReport, JournalError> {
        self.read_observed(&NoProbe)
    }

    pub fn read_observed(&self, probe: &dyn ReadProbe) -> Result<ReadReport, JournalError> {
        store::read(self.dir, self.volume, probe)
    }

    pub fn rotate(&self, now_ms: u64) -> Result<Rotation, MaintenanceError> {
        maintenance::rotate(self.dir, self.volume, self.probe, now_ms)
    }

    pub fn retain(
        &self,
        policy: &retention::Policy,
        now_ms: u64,
        current_boot: &str,
    ) -> Result<RetainReport, MaintenanceError> {
        maintenance::retain(self.dir, self.volume, self.probe, policy, now_ms, current_boot)
    }

    pub fn prune(
        &self,
        policy: &retention::Policy,
        now_ms: u64,
        current_boot: &str,
    ) -> Result<PruneReport, MaintenanceError> {
        maintenance::prune(self.dir, self.volume, self.probe, policy, now_ms, current_boot)
    }

    pub fn status(&self) -> io::Result<FsFacts> {
        volume::locate(self.volume, self.dir)
    }
}

pub fn status(dir: &Path) -> io::Result<FsFacts> {
    Journal::new(dir).status()
}

pub fn append(dir: &Path, record: &Record) -> Result<Appended, JournalError> {
    Journal::new(dir).append(record)
}

pub fn read(dir: &Path) -> Result<ReadReport, JournalError> {
    Journal::new(dir).read()
}

pub fn rotate(dir: &Path, now_ms: u64) -> Result<Rotation, MaintenanceError> {
    Journal::new(dir).rotate(now_ms)
}

pub fn retain(
    dir: &Path,
    policy: &retention::Policy,
    now_ms: u64,
    current_boot: &str,
) -> Result<RetainReport, MaintenanceError> {
    Journal::new(dir).retain(policy, now_ms, current_boot)
}

pub fn prune(
    dir: &Path,
    policy: &retention::Policy,
    now_ms: u64,
    current_boot: &str,
) -> Result<PruneReport, MaintenanceError> {
    Journal::new(dir).prune(policy, now_ms, current_boot)
}
