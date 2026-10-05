use std::io;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalResult {
    Delivered,
    NoSuchProcess,
    Refused,
    Failed(i32),
}

pub trait Signaller: Send + Sync {
    fn sigterm(&self, pid: i32) -> SignalResult;
}

pub const fn signalable(pid: i32) -> bool {
    pid > 1
}

pub struct KillSignaller;

impl Signaller for KillSignaller {
    fn sigterm(&self, pid: i32) -> SignalResult {
        if !signalable(pid) {
            return SignalResult::Refused;
        }
        // SAFETY: pid is above 1, so kill addresses exactly one process and never a group or every process.
        if unsafe { libc::kill(pid, libc::SIGTERM) } == 0 {
            return SignalResult::Delivered;
        }
        match io::Error::last_os_error().raw_os_error() {
            Some(libc::ESRCH) => SignalResult::NoSuchProcess,
            other => SignalResult::Failed(other.unwrap_or(0)),
        }
    }
}
