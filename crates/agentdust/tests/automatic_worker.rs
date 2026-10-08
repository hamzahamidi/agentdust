#![cfg(target_os = "macos")]

mod setup_support;

use std::fs;
use std::process::{Command, Stdio};
use std::time::Duration;

use agentdust_core::automatic::{self, PolicyGuard};
use agentdust_core::darwin::DarwinProvider;
use agentdust_core::journal::{self, Agent, AgentIdentity, Kind, Record, SCHEMA_VERSION};
use agentdust_core::provider::{ProcessProvider, ProcessRead};
use agentdust_core::revalidate::Revalidation;
use agentdust_core::secret::load_or_create;
use agentdust_core::tag::SessionTag;
use agentdust_testkit::Fixture;
use agentdust_testkit::harness::Harness;
use agentdust_testkit::spec::ProcSpec;
use agentdust_testkit::wait_until;
use serde_json::Value;
use setup_support::Sandbox;

const WAIT: Duration = Duration::from_secs(45);

fn worker(sandbox: &Sandbox) -> Fixture {
    Fixture(
        Command::new(setup_support::BIN)
            .args(["auto", "worker"])
            .env("AGENTDUST_DATA_DIR", &sandbox.data)
            .env("HOME", &sandbox.home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    )
}

#[test]
#[ignore = "uses the explicitly built fixture sleeper and starts only an isolated test worker"]
fn normal_exit_cleans_one_helper_preserves_keep_and_never_retries_a_survivor_after_restart() {
    check_exit(false, Agent::Claude);
}

#[test]
#[ignore = "uses the explicitly built fixture sleeper and starts only an isolated test worker"]
fn abrupt_exit_cleans_one_helper_preserves_keep_and_never_retries_a_survivor_after_restart() {
    check_exit(true, Agent::Claude);
}

#[test]
#[ignore = "uses the explicitly built fixture sleeper and starts only an isolated test worker"]
fn codex_normal_host_exit_cleans_one_helper_and_preserves_keep_and_restart_receipts() {
    check_exit(false, Agent::Codex);
}

#[test]
#[ignore = "uses the explicitly built fixture sleeper and starts only an isolated test worker"]
fn codex_abrupt_host_exit_cleans_one_helper_and_preserves_keep_and_restart_receipts() {
    check_exit(true, Agent::Codex);
}

fn check_exit(abrupt: bool, agent: Agent) {
    let fixture =
        std::env::var_os("AGENTDUST_FIXTURE_SLEEPER").expect("build fixture-sleeper and provide its path");
    let sandbox = Sandbox::new("auto-worker");
    let secret = load_or_create(&sandbox.data).unwrap();
    let key = agentdust_core::cwd::cwd_key(&secret, sandbox.home.to_str().unwrap()).unwrap();
    let tag = SessionTag::generate().unwrap();
    let session = "12345678-1234-1234-1234-123456789abc";
    let (marker_name, marker, tag_key) = if agent == Agent::Codex {
        (
            "CODEX_SESSION_ID",
            session,
            agentdust_core::tag::codex_key_of(&secret, session.as_bytes()).unwrap(),
        )
    } else {
        ("AGENTDUST_SESSION", tag.as_str(), tag.key(&secret))
    };
    let mut harness = Harness::new(fixture).unwrap();
    let target_tree = harness
        .spawn_tree(&ProcSpec::new().seconds(120).spawn(1).env(marker_name, marker))
        .unwrap();
    let kept_tree = harness
        .spawn_tree(&ProcSpec::new().seconds(120).spawn(1).env(marker_name, marker))
        .unwrap();
    let survivor_tree = harness
        .spawn_tree(
            &ProcSpec::new()
                .seconds(120)
                .spawn(1)
                .ignore_term()
                .env(marker_name, marker),
        )
        .unwrap();
    harness.orphan(target_tree.parent).unwrap();
    harness.orphan(kept_tree.parent).unwrap();
    harness.orphan(survivor_tree.parent).unwrap();
    let target = target_tree.children[0];
    let kept = kept_tree.children[0];
    let survivor = survivor_tree.children[0];
    let unrelated = harness.spawn(&ProcSpec::new().seconds(120)).unwrap();
    let mut owner = Command::new("/bin/cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let ProcessRead::Present(identity) = DarwinProvider::new().unwrap().read(owner.id() as i32).unwrap()
    else {
        panic!("owner identity is present");
    };
    let start = Record {
        v: SCHEMA_VERSION,
        kind: Kind::SessionStart,
        agent,
        session_id: session.into(),
        subagent_id: None,
        agent_identity: Some(AgentIdentity::from_process(&identity).unwrap()),
        tool_use_id: None,
        wall_ts: 1,
        mono_ts: 1,
        boot: identity.kernel.boot_session_uuid.clone(),
        session_tag_key: Some(tag_key),
        cwd_key: Some(key.clone()),
        exe_base: None,
    };
    journal::append(&sandbox.data, &start).unwrap();
    let mut policy = PolicyGuard::acquire(&sandbox.data).unwrap();
    policy.policy.enabled = true;
    policy.policy.projects.push(key);
    policy.policy.keep.push(harness.identity(kept).kernel.clone());
    policy.write().unwrap();
    drop(policy);
    let running = worker(&sandbox);
    assert!(wait_until(
        || automatic::status(&sandbox.data).ok().is_some_and(|status| {
            status["last_report"]["review"].as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item["pid"] == harness.pid(target) && item["class"] == "owned-live")
            })
        }),
        WAIT
    ));
    assert_eq!(harness.revalidate(target), Revalidation::Match);
    assert!(!sandbox.data.join("audit.log").exists());
    if abrupt {
        owner.kill().unwrap();
        owner.wait().unwrap();
    } else {
        journal::append(
            &sandbox.data,
            &Record {
                kind: Kind::SessionEnd,
                mono_ts: 2,
                wall_ts: 2,
                ..start
            },
        )
        .unwrap();
        drop(owner.stdin.take());
        assert!(owner.wait().unwrap().success());
    }
    assert!(wait_until(
        || automatic::status(&sandbox.data).ok().is_some_and(|status| {
            status["last_report"]["recent_results"]
                .as_array()
                .is_some_and(|items| {
                    items
                        .iter()
                        .any(|item| item["pid"] == harness.pid(survivor) && item["result"] == "survivor")
                })
        }),
        WAIT
    ));
    assert_eq!(harness.revalidate(target), Revalidation::Gone);
    assert_eq!(harness.revalidate(kept), Revalidation::Match);
    assert_eq!(harness.revalidate(survivor), Revalidation::Match);
    assert_eq!(harness.revalidate(unrelated), Revalidation::Match);
    let attempts = || {
        fs::read_to_string(sandbox.data.join("audit.log"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|line| line["phase"] == "attempt")
            .collect::<Vec<_>>()
    };
    let before = attempts();
    assert_eq!(before.len(), 2);
    assert!(before.iter().all(|line| {
        [harness.pid(target), harness.pid(survivor)].contains(&(line["pid"].as_i64().unwrap() as i32))
    }));
    drop(running);
    let old_report = automatic::status(&sandbox.data).unwrap()["last_report"]["wall_ms"]
        .as_u64()
        .unwrap();
    let restarted = worker(&sandbox);
    assert!(wait_until(
        || automatic::status(&sandbox.data).ok().is_some_and(|status| {
            status["last_report"]["wall_ms"]
                .as_u64()
                .is_some_and(|wall| wall > old_report)
        }),
        WAIT
    ));
    assert_eq!(attempts().len(), 2);
    assert_eq!(harness.revalidate(survivor), Revalidation::Match);
    drop(restarted);
    harness.shutdown().unwrap();
}
