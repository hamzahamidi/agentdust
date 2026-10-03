use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

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

pub fn run_hook(data_dir: &Path, input: &[u8]) -> Output {
    run_hook_with(
        &["hook", "claude"],
        &[("AGENTDUST_DATA_DIR", data_dir.as_os_str())],
        None,
        input,
    )
}

pub fn run_hook_with(args: &[&str], envs: &[(&str, &OsStr)], cwd: Option<&Path>, input: &[u8]) -> Output {
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

pub fn pre_tool_use(session: &str, tool_use_id: &str) -> Vec<u8> {
    format!(
        r#"{{"session_id":"{session}","hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"{tool_use_id}","tool_input":{{"command":"npm run dev"}}}}"#
    )
    .into_bytes()
}
