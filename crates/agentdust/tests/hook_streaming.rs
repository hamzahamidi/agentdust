#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::fs::{self, DirBuilder};
use std::io::{self, Read, Write};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use agentdust_core::journal::{self, Kind};

const MIB: usize = 1024 * 1024;
const MAX_RESIDENT_BYTES: usize = 64 * MIB;
const PREFIX: &[u8] = br#"{"session_id":"streaming","hook_event_name":"PostToolUse","tool_name":"Bash","tool_use_id":"toolu_streaming","tool_response":{"stdout":""#;
const SUFFIX: &[u8] = b"\"}}";
const SENTINEL: &[u8] = b"streamed-response-sentinel-";

fn private_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "agentdust-hook-stream-{name}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .unwrap();
    dir
}

#[test]
#[ignore = "runs the release hook with streamed 1, 10 and 100 MiB payloads"]
fn release_hook_streams_large_skipped_payloads_with_bounded_memory() {
    for megabytes in [1, 10, 100] {
        let data_dir = private_dir(&format!("hook-stream-{megabytes}"));
        let started = Instant::now();
        let (status, stdout, stderr, resident_bytes) = run_hook(&data_dir, megabytes * MIB);
        let elapsed = started.elapsed();

        assert!(status.success(), "{megabytes} MiB hook exited with {status}");
        assert!(stdout.is_empty(), "{megabytes} MiB hook printed to stdout");
        assert!(stderr.is_empty(), "{megabytes} MiB hook printed to stderr");
        assert!(
            resident_bytes < MAX_RESIDENT_BYTES,
            "{megabytes} MiB input reached {resident_bytes} resident bytes"
        );
        let report = journal::read(&data_dir).unwrap();
        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].kind, Kind::ShellEnd);
        let journal_text = fs::read_to_string(data_dir.join("journal.jsonl")).unwrap();
        assert!(!journal_text.contains(std::str::from_utf8(SENTINEL).unwrap()));
        println!(
            "release hook {megabytes} MiB: {:.0} ms, {resident_bytes} peak resident bytes",
            elapsed.as_secs_f64() * 1000.0
        );
        fs::remove_dir_all(data_dir).unwrap();
    }
}

// wait_with_usage must reap the child with wait4 to capture its peak RSS.
#[allow(clippy::zombie_processes)]
fn run_hook(data_dir: &Path, payload_bytes: usize) -> (ExitStatus, Vec<u8>, Vec<u8>, usize) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentdust"))
        .args(["hook", "claude"])
        .env("AGENTDUST_DATA_DIR", data_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id() as libc::pid_t;
    let mut stdin = child.stdin.take().unwrap();
    let writer = thread::spawn(move || {
        stdin.write_all(PREFIX).unwrap();
        let mut chunk = Vec::with_capacity(64 * 1024);
        while chunk.len() < 64 * 1024 {
            let remaining = 64 * 1024 - chunk.len();
            chunk.extend_from_slice(&SENTINEL[..remaining.min(SENTINEL.len())]);
        }
        let mut remaining = payload_bytes;
        while remaining != 0 {
            let count = remaining.min(chunk.len());
            stdin.write_all(&chunk[..count]).unwrap();
            remaining -= count;
        }
        stdin.write_all(SUFFIX).unwrap();
    });
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let stdout_reader = thread::spawn(move || {
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).unwrap();
        output
    });
    let stderr_reader = thread::spawn(move || {
        let mut output = Vec::new();
        stderr.read_to_end(&mut output).unwrap();
        output
    });

    let (status, resident_bytes) = wait_with_usage(pid);
    writer.join().unwrap();
    let stdout = stdout_reader.join().unwrap();
    let stderr = stderr_reader.join().unwrap();
    (status, stdout, stderr, resident_bytes)
}

fn wait_with_usage(pid: libc::pid_t) -> (ExitStatus, usize) {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let mut status = 0;
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
        // SAFETY: the process ID came from the child, and both pointers refer to writable locals.
        let waited = unsafe { libc::wait4(pid, &mut status, libc::WNOHANG, usage.as_mut_ptr()) };
        if waited == pid {
            // SAFETY: wait4 initialized usage when it returned this process ID.
            let usage = unsafe { usage.assume_init() };
            let peak = usize::try_from(usage.ru_maxrss).unwrap();
            #[cfg(target_os = "macos")]
            let resident_bytes = peak;
            #[cfg(target_os = "linux")]
            let resident_bytes = peak * 1024;
            return (ExitStatus::from_raw(status), resident_bytes);
        }
        if waited < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                panic!("wait4 failed: {error}");
            }
        } else if Instant::now() >= deadline {
            // SAFETY: pid identifies the child whose completion is being awaited.
            unsafe { libc::kill(pid, libc::SIGKILL) };
            // SAFETY: both output pointers refer to writable locals.
            unsafe { libc::wait4(pid, &mut status, 0, usage.as_mut_ptr()) };
            panic!("release hook did not exit within 90 seconds");
        }
        thread::sleep(Duration::from_millis(5));
    }
}
