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
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentdust"))
        .args(["hook", "claude"])
        .env("AGENTDUST_DATA_DIR", data_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

pub fn pre_tool_use(session: &str, tool_use_id: &str) -> Vec<u8> {
    format!(
        r#"{{"session_id":"{session}","hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"{tool_use_id}","tool_input":{{"command":"npm run dev"}}}}"#
    )
    .into_bytes()
}
