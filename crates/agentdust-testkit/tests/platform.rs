#![cfg(target_os = "macos")]

use std::path::Path;
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use agentdust_core::darwin;
use agentdust_core::identity::KernelIdentity;

const SLEEPER: &str = env!("CARGO_BIN_EXE_fixture-sleeper");

struct Fixture(Child);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Orphan(Option<KernelIdentity>);

impl Orphan {
    fn adopt(pid: i32) -> Self {
        let identity = darwin::boot_session_uuid()
            .ok()
            .and_then(|boot| darwin::process_info(pid, &boot).ok().flatten())
            .map(|info| info.identity);
        Self(identity)
    }
}

impl Drop for Orphan {
    fn drop(&mut self) {
        let Some(identity) = &self.0 else { return };
        let alive = |pid| {
            matches!(
                darwin::process_info(pid, &identity.boot_session_uuid),
                Ok(Some(_))
            )
        };
        let unchanged = matches!(
            darwin::process_info(identity.pid, &identity.boot_session_uuid),
            Ok(Some(info)) if info.identity == *identity
        );
        if !unchanged {
            return;
        }
        // SAFETY: the kernel identity was just confirmed to be the sleeper this test spawned.
        unsafe { libc::kill(identity.pid, libc::SIGKILL) };
        let deadline = Instant::now() + Duration::from_secs(2);
        while alive(identity.pid) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
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

#[test]
fn a_named_environment_variable_is_read_from_another_process() {
    let child = spawn_sleeper(&[("AGENTDUST_SESSION", "tag-123")]);
    let value = darwin::env_var(child.0.id() as i32, "AGENTDUST_SESSION").unwrap();
    assert_eq!(value.as_deref(), Some(&b"tag-123"[..]));
    assert_eq!(
        darwin::env_var(child.0.id() as i32, "AGENTDUST_ABSENT").unwrap(),
        None
    );
}

#[test]
fn the_tag_survives_reparenting_to_launchd() {
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!(
            "AGENTDUST_SESSION=orphan-9 '{SLEEPER}' 30 </dev/null >/dev/null 2>&1 & echo $!"
        ))
        .output()
        .unwrap();
    let pid: i32 = String::from_utf8(output.stdout).unwrap().trim().parse().unwrap();
    let _orphan = Orphan::adopt(pid);
    let sleeper = Path::new(SLEEPER).canonicalize().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut ppid = 0;
    while Instant::now() < deadline {
        ppid = darwin::process_info(pid, &boot()).unwrap().unwrap().ppid;
        let execed =
            darwin::exe_path(pid).unwrap().and_then(|p| p.canonicalize().ok()) == Some(sleeper.clone());
        if ppid == 1 && execed {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let value = darwin::env_var(pid, "AGENTDUST_SESSION").unwrap();
    assert_eq!(ppid, 1);
    assert_eq!(value.as_deref(), Some(&b"orphan-9"[..]));
}

#[test]
fn launchd_arguments_are_not_readable() {
    assert_eq!(darwin::procargs2(1).unwrap(), None);
}
