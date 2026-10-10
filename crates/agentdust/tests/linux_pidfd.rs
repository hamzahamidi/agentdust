#![cfg(target_os = "linux")]

use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command};

use agentdust_core::apply::signal::{SignalResult, Signaller};
use agentdust_core::linux::{LinuxPidfdSignaller, LinuxProvider};
use agentdust_core::provider::{ProcessProvider, ProcessRead};

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn linux_apply_signals_the_revalidated_process_through_its_pidfd() {
    let mut child = ChildGuard(Command::new("/bin/sleep").arg("30").spawn().unwrap());
    let provider = LinuxProvider::new().unwrap();
    let identity = match provider.read(child.0.id() as i32).unwrap() {
        ProcessRead::Present(identity) => identity,
        other => panic!("child identity unavailable: {other:?}"),
    };

    let signaller = LinuxPidfdSignaller::new().unwrap();
    let signal = signaller.sigterm_identity(&identity);
    let required = std::env::var_os("AGENTDUST_REQUIRE_SYSTEMD_PIDFD").is_some();
    if !required && (signal == SignalResult::Refused || signal == SignalResult::Failed(libc::ENOSYS)) {
        assert!(
            matches!(provider.read(child.0.id() as i32).unwrap(), ProcessRead::Present(fresh) if fresh == identity)
        );
        assert!(child.0.try_wait().unwrap().is_none());
        return;
    }
    assert_eq!(
        signal,
        SignalResult::Delivered,
        "pidfd cleanup requires authoritative systemd unit membership"
    );
    let status = child.0.wait().unwrap();
    assert_eq!(status.signal(), Some(libc::SIGTERM));
}
