mod classifier_support;
mod inventory_support;
mod scratch;

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use agentdust_core::classifier::Policy;
use agentdust_core::cwd::{CwdRelation, RelationContext};
use agentdust_core::doctor::{Sessions, Unavailable, diagnose};
use agentdust_core::finding::{HumanDisplay, LiveText};
use agentdust_core::identity::KernelIdentity;
use agentdust_core::inventory::{Launchd, LiveDetails, Snapshot, Tag};
use classifier_support::{alive, degraded, gone, launchd, old, snapshot_with, the_tool};
use inventory_support::{Edit, HOUR, MINUTE, NOW, SELF_PID, UID, raw};
use scratch::private_dir;
use serde_json::Value;

#[derive(Default)]
struct ScriptedLive {
    cwds: HashMap<i32, PathBuf>,
    commands: HashMap<i32, Vec<Vec<u8>>>,
    calls: RefCell<Vec<String>>,
}

impl ScriptedLive {
    fn with(mut self, pid: i32, cwd: &str, command: &[&str]) -> Self {
        self.cwds.insert(pid, PathBuf::from(cwd));
        self.commands
            .insert(pid, command.iter().map(|arg| arg.as_bytes().to_vec()).collect());
        self
    }
}

impl LiveDetails for ScriptedLive {
    fn command(&self, identity: &KernelIdentity) -> Option<Vec<Vec<u8>>> {
        self.calls.borrow_mut().push(format!("command {}", identity.pid));
        self.commands.get(&identity.pid).cloned()
    }

    fn cwd(&self, identity: &KernelIdentity) -> Option<PathBuf> {
        self.calls.borrow_mut().push(format!("cwd {}", identity.pid));
        self.cwds.get(&identity.pid).cloned()
    }
}

fn nowhere() -> RelationContext {
    RelationContext {
        reference: None,
        home: None,
        temp_roots: Vec::new(),
    }
}

fn policy() -> Policy {
    Policy::new(UID, SELF_PID)
}

fn sessions() -> Sessions {
    Sessions {
        unavailable: None,
        scopes: vec![gone(900, &[1]), alive(901, &[2])],
        records: 3,
        skipped_lines: 0,
    }
}

fn world() -> Snapshot {
    let mut processes = the_tool();
    processes.extend([
        old(300),
        raw(301).tagged(1).started(NOW - 3 * HOUR),
        raw(302).tagged(2).started(NOW - 90 * MINUTE),
        raw(303),
        raw(304).exe("/usr/bin/x"),
        raw(305).tagged(9),
    ]);
    snapshot_with(processes, launchd(&[]))
}

fn live() -> ScriptedLive {
    ScriptedLive::default()
        .with(300, "/nowhere/a", &["node", "tunnel.js", "--token=hunter2"])
        .with(301, "/nowhere/b", &["node", "dev.js"])
        .with(302, "/nowhere/c", &["node", "agent-child.js"])
}

fn value(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

#[test]
fn every_process_is_counted_by_class() {
    let diagnosis = diagnose(&world(), &sessions(), &policy(), &live(), &nowhere());
    assert_eq!(diagnosis.processes_seen, 9);
    let counts = &diagnosis.counts;
    assert_eq!(
        (
            counts.managed,
            counts.owned_live,
            counts.owned_ended,
            counts.likely_owned,
            counts.suspect,
            counts.unknown
        ),
        (4, 1, 1, 0, 1, 2)
    );
}

#[test]
fn only_owned_and_suspect_findings_are_listed_actionable_ones_first() {
    let diagnosis = diagnose(&world(), &sessions(), &policy(), &live(), &nowhere());
    let listed: Vec<(i32, String)> = diagnosis
        .findings
        .iter()
        .map(|finding| (finding.pid, finding.class.to_string()))
        .collect();
    assert_eq!(
        listed,
        [
            (301, "owned-ended".to_owned()),
            (300, "suspect".to_owned()),
            (302, "owned-live".to_owned())
        ]
    );
}

#[test]
fn findings_of_one_class_are_ordered_by_pid() {
    let mut processes = the_tool();
    processes.extend([old(320), old(310), old(330)]);
    let snapshot = snapshot_with(processes, launchd(&[]));
    let diagnosis = diagnose(
        &snapshot,
        &sessions(),
        &policy(),
        &ScriptedLive::default(),
        &nowhere(),
    );
    let pids: Vec<i32> = diagnosis.findings.iter().map(|finding| finding.pid).collect();
    assert_eq!(pids, [310, 320, 330]);
}

#[test]
fn the_working_directory_is_read_only_for_listed_findings() {
    let live = live();
    diagnose(&world(), &sessions(), &policy(), &live, &nowhere());
    let mut calls = live.calls.borrow().clone();
    calls.sort();
    assert_eq!(calls, ["cwd 300", "cwd 301", "cwd 302"]);
}

#[test]
fn the_cwd_relation_comes_from_the_live_directory() {
    let root = fs::canonicalize({
        let dir = private_dir("doctor-relation");
        fs::create_dir_all(&dir).unwrap();
        dir
    })
    .unwrap();
    for dir in ["repo_a/.git", "repo_a/src", "repo_b/.git", "home", "scratch"] {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    let context = RelationContext {
        reference: Some(root.join("repo_a/src")),
        home: Some(root.join("home")),
        temp_roots: vec![root.join("scratch")],
    };
    let at = |tail: &str| root.join(tail).to_str().unwrap().to_owned();
    let live = ScriptedLive::default()
        .with(300, &at("repo_a/src"), &["x"])
        .with(301, &at("repo_b"), &["x"])
        .with(302, &at("home"), &["x"]);
    let diagnosis = diagnose(&world(), &sessions(), &policy(), &live, &context);
    let relation = |pid: i32| {
        diagnosis
            .findings
            .iter()
            .find(|finding| finding.pid == pid)
            .unwrap()
            .cwd_relation
    };
    assert_eq!(relation(300), CwdRelation::SameRepo);
    assert_eq!(relation(301), CwdRelation::OtherRepo);
    assert_eq!(relation(302), CwdRelation::Home);
    let unreadable = diagnose(
        &world(),
        &sessions(),
        &policy(),
        &ScriptedLive::default(),
        &context,
    );
    assert!(
        unreadable
            .findings
            .iter()
            .all(|f| f.cwd_relation == CwdRelation::Other)
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn tags_that_no_usable_session_explains_are_counted() {
    let mut processes = the_tool();
    processes.extend([
        raw(310).tagged(9),
        raw(311).tag(Tag::Unkeyed),
        raw(312).tag(Tag::Unreadable),
        raw(313).tagged(3),
        raw(314),
    ]);
    let mut sessions = sessions();
    sessions.scopes.push(degraded(&[3]));
    let snapshot = snapshot_with(processes, launchd(&[]));
    let diagnosis = diagnose(
        &snapshot,
        &sessions,
        &policy(),
        &ScriptedLive::default(),
        &nowhere(),
    );
    assert_eq!(diagnosis.unexplained_tags, 4);
}

#[test]
fn unavailable_provenance_removes_owned_classes_and_says_why() {
    let mut sessions = sessions();
    sessions.unavailable = Some(Unavailable::UnsupportedVersion);
    sessions.scopes.clear();
    let diagnosis = diagnose(&world(), &sessions, &policy(), &live(), &nowhere());
    assert_eq!(diagnosis.owned, Some(Unavailable::UnsupportedVersion));
    assert_eq!(diagnosis.counts.owned_ended, 0);
    assert_eq!(diagnosis.counts.owned_live, 0);
    let classes: Vec<String> = diagnosis.findings.iter().map(|f| f.class.to_string()).collect();
    assert_eq!(classes, ["suspect"]);
}

#[test]
fn a_missing_launchd_list_is_reported_and_nothing_is_offered() {
    let snapshot = Snapshot {
        launchd: Launchd::Unavailable,
        ..world()
    };
    let diagnosis = diagnose(&snapshot, &sessions(), &policy(), &live(), &nowhere());
    assert!(!diagnosis.launchd_available);
    assert_eq!(diagnosis.counts.owned_ended, 0);
    assert_eq!(diagnosis.counts.suspect, 0);
    let classes: Vec<String> = diagnosis.findings.iter().map(|f| f.class.to_string()).collect();
    assert_eq!(classes, ["owned-live"]);
}

#[test]
fn sessions_are_summarised_by_state_and_by_whether_the_agent_is_known() {
    let mut sessions = sessions();
    sessions.scopes.push(degraded(&[3]));
    sessions.skipped_lines = 2;
    let diagnosis = diagnose(&world(), &sessions, &policy(), &live(), &nowhere());
    let summary = &diagnosis.sessions;
    assert_eq!(
        (
            summary.records,
            summary.skipped_lines,
            summary.active,
            summary.ended,
            summary.unknown,
            summary.degraded
        ),
        (3, 2, 2, 1, 0, 1)
    );
}

#[test]
fn the_json_has_a_fixed_shape() {
    let diagnosis = diagnose(&world(), &sessions(), &policy(), &live(), &nowhere());
    let text = diagnosis.to_json();
    assert!(text.starts_with(
        "{\"version\":1,\"owned_classes\":{\"available\":true,\"reason\":null},\"launchd\":{\"available\":true},\
         \"sessions\":{\"records\":3,\"skipped_lines\":0,\"active\":1,\"ended\":1,\"unknown\":0,\"degraded\":0},\
         \"unexplained_tags\":1,\
         \"counts\":{\"managed\":4,\"owned-live\":1,\"owned-ended\":1,\"likely-owned\":0,\"suspect\":1,\"unknown\":2},\
         \"findings\":[{\"item_id\":"
    ), "{text}");
    let parsed = value(&text);
    let findings = parsed["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 3);
    assert_eq!(findings[0]["class"], "owned-ended");
    assert_eq!(findings[0]["pid"], 301);
    assert_eq!(
        findings[0]["evidence"],
        serde_json::json!(["owned.tag", "owned.agent_gone"])
    );
    assert!(!text.contains('\n'));
}

#[test]
fn the_json_says_when_owned_classes_are_unavailable() {
    let mut sessions = sessions();
    sessions.unavailable = Some(Unavailable::UnsupportedFilesystem("nfs".to_owned()));
    sessions.scopes.clear();
    let parsed = value(&diagnose(&world(), &sessions, &policy(), &live(), &nowhere()).to_json());
    assert_eq!(parsed["owned_classes"]["available"], false);
    assert_eq!(parsed["owned_classes"]["reason"], "unsupported_filesystem");
    let mut sessions = Sessions {
        unavailable: Some(Unavailable::UnsupportedVersion),
        ..sessions
    };
    sessions.scopes.clear();
    let parsed = value(&diagnose(&world(), &sessions, &policy(), &live(), &nowhere()).to_json());
    assert_eq!(parsed["owned_classes"]["reason"], "unsupported_version");
}

#[test]
fn a_report_without_findings_has_an_empty_list() {
    let snapshot = snapshot_with(the_tool(), launchd(&[]));
    let diagnosis = diagnose(&snapshot, &sessions(), &policy(), &live(), &nowhere());
    let parsed = value(&diagnosis.to_json());
    assert_eq!(parsed["findings"], serde_json::json!([]));
    assert!(diagnosis.render(&live()).contains("findings: none"));
}

fn planted() -> Snapshot {
    let mut processes = the_tool();
    processes.push(
        old(300)
            .exe("/Users/alice-sentinel/work/repo-sentinel/bin/tool-bin")
            .started(NOW - 40 * MINUTE),
    );
    snapshot_with(processes, launchd(&[]))
}

#[test]
fn neither_a_path_nor_a_command_reaches_the_json() {
    let live = ScriptedLive::default().with(
        300,
        "/Users/alice-sentinel/work/repo-sentinel",
        &[
            "/Users/alice-sentinel/bin/tool-bin",
            "--api-key=command-sentinel",
            "https://example.test/repo-sentinel",
        ],
    );
    let diagnosis = diagnose(&planted(), &sessions(), &policy(), &live, &nowhere());
    let json = diagnosis.to_json();
    assert!(json.contains("tool-bin"));
    for secret in [
        "alice-sentinel",
        "repo-sentinel",
        "command-sentinel",
        "example.test",
        "/Users",
        "work",
    ] {
        assert!(!json.contains(secret), "{secret} in {json}");
    }
}

#[test]
fn the_terminal_text_shows_the_redacted_command_and_the_directory_only_for_findings() {
    let live = ScriptedLive::default().with(
        300,
        "/Users/alice-sentinel/work/repo-sentinel",
        &["tool-bin", "--api-key=command-sentinel", "serve"],
    );
    let diagnosis = diagnose(&planted(), &sessions(), &policy(), &live, &nowhere());
    let text = diagnosis.render(&live);
    assert!(text.contains("command: tool-bin --api-key=*** serve"), "{text}");
    assert!(
        text.contains("directory: /Users/alice-sentinel/work/repo-sentinel"),
        "{text}"
    );
    assert!(!text.contains("command-sentinel"));
    let reads: Vec<String> = live.calls.borrow().clone();
    assert!(reads.iter().all(|call| call.ends_with(" 300")), "{reads:?}");
}

fn block(pid: i32, diagnosis: &agentdust_core::doctor::Diagnosis, live: &ScriptedLive) -> String {
    let finding = diagnosis.findings.iter().find(|f| f.pid == pid).unwrap();
    let args = live.commands.get(&pid).cloned();
    let text = LiveText::new(args.as_deref(), live.cwds.get(&pid).map(PathBuf::as_path));
    HumanDisplay::terminal(finding, &text)
}

#[test]
fn the_terminal_text_has_a_fixed_layout() {
    let live = live();
    let diagnosis = diagnose(&world(), &sessions(), &policy(), &live, &nowhere());
    let expected = format!(
        "agentdust doctor\n\
         processes: 9 seen\n\
         journal: 3 records, 2 sessions (active 1, ended 1, unknown 0), 0 without a known agent\n\
         launchd: PID list read\n\
         classes: managed 4, owned-live 1, owned-ended 1, likely-owned 0, suspect 1, unknown 2\n\
         tags without a usable session: 1\n\
         findings: 3\n\n{}\n\n{}\n\n{}\n",
        block(301, &diagnosis, &live),
        block(300, &diagnosis, &live),
        block(302, &diagnosis, &live),
    );
    assert_eq!(diagnosis.render(&live), expected);
}

#[test]
fn the_terminal_text_names_each_reason_owned_classes_are_unavailable() {
    let reasons = [
        Unavailable::UnsupportedVersion,
        Unavailable::UnsupportedFilesystem("nfs".to_owned()),
        Unavailable::SecretUnavailable,
        Unavailable::JournalRefused,
        Unavailable::JournalUnreadable,
    ];
    for reason in reasons {
        let mut sessions = sessions();
        sessions.unavailable = Some(reason.clone());
        sessions.scopes.clear();
        let text = diagnose(&world(), &sessions, &policy(), &live(), &nowhere()).render(&live());
        let line = format!("journal: owned classes are unavailable: {}", reason.describe());
        assert!(text.lines().any(|l| l == line), "{text}");
        assert!(!text.contains("owned-ended 1"));
    }
}

#[test]
fn the_terminal_text_says_when_the_launchd_list_is_missing() {
    let snapshot = Snapshot {
        launchd: Launchd::Unavailable,
        ..world()
    };
    let text = diagnose(&snapshot, &sessions(), &policy(), &live(), &nowhere()).render(&live());
    assert!(
        text.lines()
            .any(|l| l == "launchd: PID list unavailable, so no process can be offered for cleanup")
    );
}

#[test]
fn skipped_journal_lines_are_reported() {
    let mut sessions = sessions();
    sessions.skipped_lines = 2;
    let text = diagnose(&world(), &sessions, &policy(), &live(), &nowhere()).render(&live());
    assert!(
        text.contains("0 without a known agent, 2 lines skipped"),
        "{text}"
    );
}

#[test]
fn hostile_process_text_cannot_change_the_layout() {
    let live = ScriptedLive::default().with(
        300,
        "/tmp/\nfindings: 0",
        &["x\n\u{1b}[2Jlaunchd: PID list read", "\u{202e}"],
    );
    let diagnosis = diagnose(&planted(), &sessions(), &policy(), &live, &nowhere());
    let text = diagnosis.render(&live);
    assert!(!text.contains('\u{1b}'));
    assert!(!text.contains('\u{202e}'));
    assert_eq!(text.lines().filter(|l| l.starts_with("findings:")).count(), 1);
    assert_eq!(text.lines().filter(|l| l.starts_with("launchd:")).count(), 1);
}
