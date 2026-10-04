use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::time::Duration;

use agentdust_core::apply::signal::{KillSignaller, SignalResult, Signaller, signalable};
use agentdust_core::apply::timer::{SystemTimer, Timer};

#[test]
fn only_a_pid_above_one_may_be_signalled() {
    for pid in [i32::MIN, -2, -1, 0, 1] {
        assert!(!signalable(pid), "{pid}");
    }
    for pid in [2, 3, 99_998, i32::MAX] {
        assert!(signalable(pid), "{pid}");
    }
}

#[test]
fn the_real_signaller_refuses_pid_1() {
    assert_eq!(KillSignaller.sigterm(1), SignalResult::Refused);
}

#[test]
fn the_real_signaller_reports_a_pid_that_cannot_exist_as_gone() {
    assert_eq!(KillSignaller.sigterm(i32::MAX), SignalResult::NoSuchProcess);
}

#[test]
fn the_real_signaller_sends_sigterm_to_one_child_of_the_test() {
    let mut child = Command::new("sleep")
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id() as i32;
    assert_eq!(KillSignaller.sigterm(pid), SignalResult::Delivered);
    let status = child.wait().unwrap();
    assert_eq!(status.signal(), Some(libc::SIGTERM));
}

#[test]
fn the_system_timer_never_goes_backwards_and_sleeps_at_least_as_asked() {
    let timer = SystemTimer::new();
    let before = timer.now();
    timer.sleep(Duration::from_millis(20));
    let after = timer.now();
    assert!(after >= before + Duration::from_millis(20));
}
