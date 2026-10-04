use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agentdust_core::journal::Record;
use thiserror::Error;

use crate::frame::{EncodeError, Framing, encode};
use crate::maintenance::{self, MaintenanceError, RetainReport, Retention, Rotation};
use crate::probe::{NoProbe, Point, Probe};
use crate::store::{
    self, ACTIVE, LOCK, LockError, LockMode, MAINT, ReadLock, ReadOutcome, count_files, create_private_dir,
    lock_within, open_active, open_lock,
};

pub const APPEND_LOCK_BUDGET: Duration = Duration::from_millis(20);
pub const MAX_ATTEMPTS: u32 = 3;
const APPEND_LOCK_PAUSE: Duration = Duration::from_millis(1);
const MAINTENANCE_LOCK_PAUSE: Duration = Duration::from_micros(100);

#[derive(Debug, Error)]
pub enum AppendError {
    #[error("the journal lock stayed busy beyond the write budget")]
    Busy,
    #[error("the active file was replaced under {attempts} attempts in a row")]
    Stale { attempts: u32 },
    #[error("a write returned {written} of {expected} bytes")]
    ShortWrite { written: usize, expected: usize },
    #[error(transparent)]
    Encode(#[from] EncodeError),
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl From<LockError> for AppendError {
    fn from(err: LockError) -> Self {
        match err {
            LockError::Busy => AppendError::Busy,
            LockError::Io(err) => AppendError::Io(err),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Appended {
    pub attempts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub maintenance_budget: Duration,
    pub sync: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            maintenance_budget: Duration::from_millis(500),
            sync: true,
        }
    }
}

pub trait Journal: Send + Sync {
    fn append_probed(&self, record: &Record, probe: &dyn Probe) -> Result<Appended, AppendError>;
    fn read_probed(&self, probe: &dyn Probe) -> io::Result<ReadOutcome>;
    fn rotate(&self, now_ms: u64) -> Result<Rotation, MaintenanceError>;
    fn retain(&self, plan: &Retention) -> Result<RetainReport, MaintenanceError>;
    fn file_count(&self) -> io::Result<usize>;

    fn append(&self, record: &Record) -> Result<Appended, AppendError> {
        self.append_probed(record, &NoProbe)
    }

    fn read_all(&self) -> io::Result<ReadOutcome> {
        self.read_probed(&NoProbe)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Candidate {
    Flock,
    Append,
    Recheck,
    Shared,
}

impl Candidate {
    pub const ALL: [Candidate; 4] = [
        Candidate::Flock,
        Candidate::Append,
        Candidate::Recheck,
        Candidate::Shared,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Candidate::Flock => "A",
            Candidate::Append => "C",
            Candidate::Recheck => "C2",
            Candidate::Shared => "D",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.label().eq_ignore_ascii_case(label))
    }

    pub fn describe(self) -> &'static str {
        match self {
            Candidate::Flock => "one journal.jsonl behind an exclusive flock on journal.lock, 20 ms budget",
            Candidate::Append => "one journal.jsonl, O_APPEND, one write(2) per record, no lock",
            Candidate::Recheck => {
                "as C, then fstat the descriptor against stat of the path and write again if they differ"
            }
            Candidate::Shared => {
                "as C, inside a shared flock on journal.lock that only the rotator takes exclusively"
            }
        }
    }

    pub fn framing(self) -> Framing {
        match self {
            Candidate::Flock => Framing::Line,
            _ => Framing::Record,
        }
    }

    pub fn open(self, dir: &Path) -> Box<dyn Journal> {
        self.open_with(dir, Options::default())
    }

    pub fn open_with(self, dir: &Path, options: Options) -> Box<dyn Journal> {
        Box::new(Store {
            dir: dir.to_path_buf(),
            kind: self,
            options,
        })
    }
}

struct Store {
    dir: PathBuf,
    kind: Candidate,
    options: Options,
}

impl Journal for Store {
    fn append_probed(&self, record: &Record, probe: &dyn Probe) -> Result<Appended, AppendError> {
        let frame = encode(record, self.kind.framing())?;
        create_private_dir(&self.dir)?;
        match self.kind {
            Candidate::Flock => self.append_flock(&frame, probe),
            Candidate::Append => self.append_plain(&frame, probe),
            Candidate::Recheck => self.append_recheck(&frame, probe),
            Candidate::Shared => self.append_shared(&frame, probe),
        }
    }

    fn read_probed(&self, probe: &dyn Probe) -> io::Result<ReadOutcome> {
        let lock = match self.kind {
            Candidate::Flock => ReadLock::Whole,
            Candidate::Shared => ReadLock::Opening,
            Candidate::Append | Candidate::Recheck => ReadLock::None,
        };
        store::read_snapshot(&self.dir, lock, probe)
    }

    fn rotate(&self, now_ms: u64) -> Result<Rotation, MaintenanceError> {
        if !self.dir.exists() {
            return Ok(Rotation::Empty);
        }
        let _run = self.run_guard()?;
        maintenance::rotate(&self.dir, now_ms, self.options.sync, &|| self.swap_guard())
    }

    fn retain(&self, plan: &Retention) -> Result<RetainReport, MaintenanceError> {
        if !self.dir.exists() {
            return Ok(RetainReport::default());
        }
        let _run = self.run_guard()?;
        maintenance::retain(&self.dir, plan, self.kind.framing(), self.options.sync, &|| {
            self.swap_guard()
        })
    }

    fn file_count(&self) -> io::Result<usize> {
        count_files(&self.dir)
    }
}

impl Store {
    fn append_flock(&self, frame: &[u8], probe: &dyn Probe) -> Result<Appended, AppendError> {
        let lock = open_lock(&self.dir, LOCK)?;
        lock_within(&lock, LockMode::Exclusive, APPEND_LOCK_BUDGET, APPEND_LOCK_PAUSE)?;
        let mut file = open_active(&self.dir)?;
        probe.reached(Point::AfterOpen);
        probe.reached(Point::BeforeWrite);
        file.write_all(frame)?;
        probe.reached(Point::AfterWrite);
        Ok(Appended { attempts: 1 })
    }

    fn append_plain(&self, frame: &[u8], probe: &dyn Probe) -> Result<Appended, AppendError> {
        let file = open_active(&self.dir)?;
        probe.reached(Point::AfterOpen);
        probe.reached(Point::BeforeWrite);
        write_once(&file, frame)?;
        probe.reached(Point::AfterWrite);
        Ok(Appended { attempts: 1 })
    }

    fn append_recheck(&self, frame: &[u8], probe: &dyn Probe) -> Result<Appended, AppendError> {
        for attempt in 1..=MAX_ATTEMPTS {
            let file = open_active(&self.dir)?;
            probe.reached(Point::AfterOpen);
            probe.reached(Point::BeforeWrite);
            write_once(&file, frame)?;
            probe.reached(Point::AfterWrite);
            if still_active(&file, &self.dir.join(ACTIVE))? {
                return Ok(Appended { attempts: attempt });
            }
        }
        Err(AppendError::Stale {
            attempts: MAX_ATTEMPTS,
        })
    }

    fn append_shared(&self, frame: &[u8], probe: &dyn Probe) -> Result<Appended, AppendError> {
        let lock = open_lock(&self.dir, LOCK)?;
        lock_within(&lock, LockMode::Shared, APPEND_LOCK_BUDGET, APPEND_LOCK_PAUSE)?;
        let file = open_active(&self.dir)?;
        probe.reached(Point::AfterOpen);
        probe.reached(Point::BeforeWrite);
        write_once(&file, frame)?;
        probe.reached(Point::AfterWrite);
        Ok(Appended { attempts: 1 })
    }

    fn run_guard(&self) -> Result<File, MaintenanceError> {
        let name = if self.kind == Candidate::Flock {
            LOCK
        } else {
            MAINT
        };
        let file = open_lock(&self.dir, name)?;
        lock_within(
            &file,
            LockMode::Exclusive,
            self.options.maintenance_budget,
            MAINTENANCE_LOCK_PAUSE,
        )?;
        Ok(file)
    }

    fn swap_guard(&self) -> Result<Option<File>, MaintenanceError> {
        if self.kind != Candidate::Shared {
            return Ok(None);
        }
        let file = open_lock(&self.dir, LOCK)?;
        lock_within(
            &file,
            LockMode::Exclusive,
            self.options.maintenance_budget,
            MAINTENANCE_LOCK_PAUSE,
        )?;
        Ok(Some(file))
    }
}

fn write_once(mut file: &File, frame: &[u8]) -> Result<(), AppendError> {
    loop {
        match file.write(frame) {
            Ok(written) if written == frame.len() => return Ok(()),
            Ok(written) => {
                return Err(AppendError::ShortWrite {
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
