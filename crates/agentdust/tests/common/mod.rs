#![allow(dead_code)]

pub mod chain;
pub mod sleeper;

use std::ffi::OsStr;
use std::fs::{self, DirBuilder};
use std::io::Write;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub const HANG_GUARD: Duration = Duration::from_secs(60);

pub fn scratch_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "agentdust-bin-{name}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    dir
}

pub fn private_dir(name: &str) -> PathBuf {
    let dir = scratch_dir(name);
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .unwrap();
    dir
}

pub fn make_fifo(path: &Path) {
    assert!(Command::new("mkfifo").arg(path).status().unwrap().success());
}

pub fn run_hook_within(data_dir: &Path, input: &[u8], limit: Duration) -> Option<Output> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentdust"))
        .args(["hook", "claude"])
        .env("AGENTDUST_DATA_DIR", data_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let deadline = Instant::now() + limit;
    loop {
        if child.try_wait().unwrap().is_some() {
            return Some(child.wait_with_output().unwrap());
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

pub fn run_hook_guarded(envs: &[(&str, &OsStr)], input: &[u8]) -> Option<Output> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentdust"))
        .args(["hook", "claude"])
        .envs(envs.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let deadline = Instant::now() + HANG_GUARD;
    loop {
        if child.try_wait().unwrap().is_some() {
            return Some(child.wait_with_output().unwrap());
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

pub fn run_hook(data_dir: &Path, input: &[u8]) -> Output {
    run_hook_with(
        ["hook", "claude"],
        &[("AGENTDUST_DATA_DIR", data_dir.as_os_str())],
        None,
        input,
    )
}

pub fn run_hook_with<A: AsRef<OsStr>>(
    args: impl IntoIterator<Item = A>,
    envs: &[(&str, &OsStr)],
    cwd: Option<&Path>,
    input: &[u8],
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentdust"));
    command
        .args(args)
        .envs(envs.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = command.spawn().unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

pub const FIXTURE_CWD: &str = "/agentdust-fixture/project";

pub fn pre_tool_use(session: &str, tool_use_id: &str) -> Vec<u8> {
    pre_tool_use_with(session, tool_use_id, &format!(r#","cwd":"{FIXTURE_CWD}""#))
}

pub fn pre_tool_use_with(session: &str, tool_use_id: &str, extra_fields: &str) -> Vec<u8> {
    format!(
        r#"{{"session_id":"{session}","hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"{tool_use_id}","tool_input":{{"command":"npm run dev"}}{extra_fields}}}"#
    )
    .into_bytes()
}

pub fn run_hooks_together(data_dir: &Path, inputs: &[Vec<u8>]) -> Vec<Output> {
    let mut children: Vec<_> = inputs
        .iter()
        .map(|_| {
            Command::new(env!("CARGO_BIN_EXE_agentdust"))
                .args(["hook", "claude"])
                .env("AGENTDUST_DATA_DIR", data_dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for (child, input) in children.iter_mut().zip(inputs) {
        child.stdin.take().unwrap().write_all(input).unwrap();
    }
    children
        .into_iter()
        .map(|child| child.wait_with_output().unwrap())
        .collect()
}
