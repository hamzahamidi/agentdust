use std::io;
use std::path::PathBuf;

use crate::identity::{IdentityEvidence, KernelIdentity, ProcessIdentity};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessRead {
    Present(ProcessIdentity),
    Gone,
    PathUnreadable(KernelIdentity),
}

// Every field of one returned identity must come from the same process.
pub trait ProcessProvider {
    fn read(&self, pid: i32) -> io::Result<ProcessRead>;
}

impl ProcessRead {
    pub fn from_samples(
        before: Option<KernelIdentity>,
        path: Option<PathBuf>,
        after: Option<KernelIdentity>,
    ) -> Self {
        let (Some(before), Some(after)) = (before, after) else {
            return Self::Gone;
        };
        if before != after {
            return Self::PathUnreadable(after);
        }
        match path {
            Some(exe_path) => Self::Present(ProcessIdentity {
                kernel: after,
                evidence: IdentityEvidence { exe_path },
            }),
            None => Self::PathUnreadable(after),
        }
    }
}
