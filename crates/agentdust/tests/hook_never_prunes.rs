mod common;

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use agentdust_core::journal::{self, Agent, Kind, Record};
use common::{HANG_GUARD, pre_tool_use, private_dir, run_hook, run_hook_with, run_hook_within};

const EARLIER_BOOT: &str = "an-earlier-boot";

fn old_record(kind: Kind, session: &str, wall_ts: u64) -> Record {
    Record {
        v: 1,
        kind,
        agent: Agent::Claude,
        session_id: session.to_owned(),
        subagent_id: None,
        tool_use_id: None,
        wall_ts,
        mono_ts: wall_ts,
        boot: EARLIER_BOOT.to_owned(),
        cwd_key: None,
        agent_identity: None,
        session_tag_key: None,
        exe_base: None,
    }
}

fn seeded(name: &str) -> PathBuf {
    let dir = private_dir(name);
    for n in 1..=20 {
        let session = format!("old-{n}");
        journal::append(&dir, &old_record(Kind::SessionStart, &session, n)).unwrap();
        journal::append(&dir, &old_record(Kind::SessionEnd, &session, n + 1)).unwrap();
    }
    fs::rename(dir.join("journal.jsonl"), dir.join("journal.100.jsonl")).unwrap();
    for n in 1..=5 {
        journal::append(&dir, &old_record(Kind::ShellStart, "active", 100 + n)).unwrap();
    }
    dir
}

fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().into_string().unwrap(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}

#[test]
fn a_hook_never_rotates_compacts_deletes_or_takes_the_maintenance_lock() {
    let dir = seeded("never-prunes");
    let before = files(&dir);
    for n in 0..6 {
        let output = run_hook(&dir, &pre_tool_use("live", &format!("toolu_{n}")));
        assert!(output.status.success());
        assert!(output.stdout.is_empty() && output.stderr.is_empty());
    }
    let after = files(&dir);

    assert!(
        !after.contains_key("journal.maint"),
        "a hook took the maintenance lock"
    );
    assert_eq!(
        after.keys().map(String::as_str).collect::<Vec<_>>(),
        ["install.secret", "journal.100.jsonl", "journal.jsonl"]
    );
    assert_eq!(after["journal.100.jsonl"], before["journal.100.jsonl"]);
    assert!(after["journal.jsonl"].starts_with(&before["journal.jsonl"]));
    let appended = journal::read(&dir).unwrap().records.len() - 45;
    assert_eq!(appended, 6);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_hook_appends_while_another_process_holds_the_maintenance_lock() {
    let dir = seeded("lock-held");
    let path = dir.join("journal.maint");
    let held = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    held.lock().unwrap();

    let output = run_hook_within(&dir, &pre_tool_use("live", "toolu_held"), HANG_GUARD)
        .expect("the hook waited for the maintenance lock");

    assert!(output.status.success());
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    let report = journal::read(&dir).unwrap();
    assert_eq!(report.records.len(), 46);
    assert!(
        report
            .records
            .iter()
            .any(|record| record.tool_use_id.as_deref() == Some("toolu_held"))
    );
    drop(held);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_binary_has_no_maintenance_subcommand() {
    let dir = seeded("no-maintenance");
    let before = files(&dir);
    let names = [
        "prune",
        "rotate",
        "compact",
        "maintenance",
        "retain",
        "retention",
        "gc",
        "journal",
    ];
    for name in names {
        let output = Command::new(env!("CARGO_BIN_EXE_agentdust"))
            .arg(name)
            .env("AGENTDUST_DATA_DIR", &dir)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{name}");
        assert!(output.stdout.is_empty(), "{name}");
        let usage = String::from_utf8(output.stderr).unwrap();
        assert!(usage.starts_with("usage: agentdust"), "{name}");
        for word in names {
            assert!(!usage.contains(word), "the usage line mentions {word}");
        }
    }
    assert_eq!(files(&dir), before);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_hook_given_a_maintenance_word_does_nothing() {
    let dir = seeded("hook-maintenance");
    let before = files(&dir);
    for args in [["hook", "prune"], ["hook", "rotate"], ["hook", "journal"]] {
        let output = run_hook_with(args, &[("AGENTDUST_DATA_DIR", dir.as_os_str())], None, b"{}");
        assert!(output.status.success(), "{args:?}");
        assert!(output.stdout.is_empty() && output.stderr.is_empty(), "{args:?}");
    }
    assert_eq!(files(&dir), before);
    fs::remove_dir_all(&dir).unwrap();
}
