use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KernelIdentity {
    pub boot_session_uuid: String,
    pub pid: i32,
    pub start_time_us: u64,
    pub uid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub identity: KernelIdentity,
    pub ppid: i32,
    pub pgid: i32,
}
