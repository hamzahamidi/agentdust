use std::fs::{self, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use agentdust_core::journal::{self, Agent, JournalError, Kind, Record, SCHEMA_VERSION};

fn scratch_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "agentdust-test-{name}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn record(session: &str) -> Record {
    Record {
        v: SCHEMA_VERSION,
        kind: Kind::ShellStart,
        agent: Agent::Claude,
        session_id: session.to_owned(),
        subagent_id: None,
        tool_use_id: Some("toolu_1".to_owned()),
        wall_ts: 1,
        mono_ts: 2,
        boot: "boot".to_owned(),
        cwd_key: None,
        exe_base: None,
    }
}

#[test]
fn appended_records_read_back_in_order() {
    let dir = scratch_dir("roundtrip");
    journal::append(&dir, &record("a")).unwrap();
    journal::append(&dir, &record("b")).unwrap();
    let report = journal::read(&dir).unwrap();
    assert_eq!(report.records, vec![record("a"), record("b")]);
    assert_eq!(report.skipped_lines, 0);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn files_are_private_to_the_user() {
    let dir = scratch_dir("modes");
    journal::append(&dir, &record("a")).unwrap();
    let mode = |p: PathBuf| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(dir.clone()), 0o700);
    assert_eq!(mode(dir.join("journal.jsonl")), 0o600);
    assert_eq!(mode(dir.join("journal.lock")), 0o600);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn malformed_and_future_lines_are_skipped() {
    let dir = scratch_dir("skip");
    journal::append(&dir, &record("a")).unwrap();
    let mut future = serde_json::to_value(record("b")).unwrap();
    future["v"] = serde_json::json!(2);
    let mut text = fs::read_to_string(dir.join("journal.jsonl")).unwrap();
    text.push_str(&format!("{future}\n{{\"truncated\": "));
    fs::write(dir.join("journal.jsonl"), text).unwrap();
    let report = journal::read(&dir).unwrap();
    assert_eq!(report.records, vec![record("a")]);
    assert_eq!(report.skipped_lines, 2);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_held_lock_fails_within_the_budget() {
    let dir = scratch_dir("busy");
    journal::append(&dir, &record("a")).unwrap();
    let holder = OpenOptions::new()
        .write(true)
        .open(dir.join("journal.lock"))
        .unwrap();
    assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX) }, 0);
    let start = Instant::now();
    let result = journal::append(&dir, &record("b"));
    assert!(matches!(result, Err(JournalError::LockBusy)));
    assert!(start.elapsed().as_millis() < 200);
    drop(holder);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_symlinked_journal_is_refused() {
    let dir = scratch_dir("symlink");
    fs::create_dir_all(&dir).unwrap();
    let target = dir.join("elsewhere");
    fs::write(&target, b"").unwrap();
    symlink(&target, dir.join("journal.jsonl")).unwrap();
    assert!(matches!(
        journal::append(&dir, &record("a")),
        Err(JournalError::Io(_))
    ));
    assert_eq!(fs::read(&target).unwrap(), b"");
    fs::remove_dir_all(&dir).unwrap();
}
