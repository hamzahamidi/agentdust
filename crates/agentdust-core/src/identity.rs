use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KernelIdentity {
    pub boot_session_uuid: String,
    pub pid: i32,
    pub start_time_us: u64,
    pub uid: u32,
}

#[derive(Debug, Clone, Eq, Serialize, Deserialize)]
pub struct IdentityEvidence {
    pub exe_path: PathBuf,
}

impl PartialEq for IdentityEvidence {
    fn eq(&self, other: &Self) -> bool {
        let Self { exe_path } = self;
        exe_path.as_os_str() == other.exe_path.as_os_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub kernel: KernelIdentity,
    pub evidence: IdentityEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub identity: KernelIdentity,
    pub ppid: i32,
    pub pgid: i32,
}
