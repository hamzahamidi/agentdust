#[cfg(target_os = "macos")]
pub use crate::darwin::{DarwinLive as Live, DarwinProvider as Provider, DarwinSource as Source};
#[cfg(target_os = "macos")]
pub use crate::inventory::LaunchctlList as ManagedProcesses;

#[cfg(target_os = "linux")]
pub use crate::linux::SystemdCgroupEvidence as ManagedProcesses;
#[cfg(target_os = "linux")]
pub use crate::linux::{LinuxLive as Live, LinuxProvider as Provider, LinuxSource as Source};

#[cfg(target_os = "macos")]
pub fn boot_identity() -> std::io::Result<String> {
    crate::darwin::boot_session_uuid()
}

#[cfg(target_os = "linux")]
pub fn boot_identity() -> std::io::Result<String> {
    crate::linux::boot_session_uuid()
}

#[cfg(target_os = "macos")]
pub fn current_uid() -> u32 {
    crate::darwin::current_uid()
}

#[cfg(target_os = "linux")]
pub fn current_uid() -> u32 {
    crate::linux::current_uid()
}

#[cfg(target_os = "macos")]
pub fn list_pids() -> std::io::Result<Vec<i32>> {
    crate::darwin::list_pids()
}

#[cfg(target_os = "linux")]
pub fn list_pids() -> std::io::Result<Vec<i32>> {
    crate::linux::list_pids()
}
