#![cfg(target_os = "macos")]

use std::path::Path;
use std::process::{Child, Command};
use std::thread;
use std::time::Duration;

use agentdust_core::darwin;

const SLEEPER: &str = env!("CARGO_BIN_EXE_fixture-sleeper");

struct Fixture(Child);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_sleeper(env: &[(&str, &str)]) -> Fixture {
    let mut command = Command::new(SLEEPER);
    command.arg("30");
    for (key, value) in env {
        command.env(key, value);
    }
    let child = command.spawn().unwrap();
    thread::sleep(Duration::from_millis(100));
    Fixture(child)
}

fn boot() -> String {
    darwin::boot_session_uuid().unwrap()
}

#[test]
fn boot_session_uuid_is_a_uuid() {
    let uuid = boot();
    assert_eq!(uuid.len(), 36, "{uuid}");
    assert_eq!(uuid.matches('-').count(), 4, "{uuid}");
}

#[test]
fn identity_of_this_process_is_stable() {
    let pid = std::process::id() as i32;
    let first = darwin::process_info(pid, &boot()).unwrap().unwrap();
    let second = darwin::process_info(pid, &boot()).unwrap().unwrap();
    assert_eq!(first.identity, second.identity);
    assert_eq!(first.identity.uid, unsafe { libc::getuid() });
    assert!(first.identity.start_time_us > 1_767_225_600_000_000);
}

#[test]
fn a_missing_pid_has_no_identity() {
    assert_eq!(darwin::process_info(i32::MAX, &boot()).unwrap(), None);
    assert_eq!(darwin::exe_path(i32::MAX).unwrap(), None);
}

#[test]
fn a_child_reports_its_parent_and_group() {
    let child = spawn_sleeper(&[]);
    let pid = child.0.id() as i32;
    let info = darwin::process_info(pid, &boot()).unwrap().unwrap();
    assert_eq!(info.ppid, std::process::id() as i32);
    assert_eq!(info.pgid, unsafe { libc::getpgrp() });
}

#[test]
fn a_child_reports_its_executable_path() {
    let child = spawn_sleeper(&[]);
    let path = darwin::exe_path(child.0.id() as i32).unwrap().unwrap();
    assert_eq!(
        path.canonicalize().unwrap(),
        Path::new(SLEEPER).canonicalize().unwrap()
    );
}
