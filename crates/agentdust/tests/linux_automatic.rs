#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agentdust_core::automatic::{self, PolicyGuard};
use agentdust_core::journal::volume::{SystemVolume, VolumeProbe};
use agentdust_core::journal::{self, Agent, AgentIdentity, Kind, Record, SCHEMA_VERSION};
use agentdust_core::platform::Provider;
use agentdust_core::provider::{ProcessProvider, ProcessRead};
use agentdust_core::secret::load_or_create;
use agentdust_core::tag::SessionTag;
use agentdust_testkit::Fixture;
use agentdust_testkit::harness::Harness;
use agentdust_testkit::spec::ProcSpec;
use agentdust_testkit::wait_until;

const WAIT: Duration = Duration::from_secs(45);

#[test]
#[ignore = "requires writable ext4, a user systemd scope, and the fixture sleeper"]
fn claude_owner_exit_cleans_one_proven_helper_and_preserves_keeps() {
    check(Agent::Claude);
}

#[test]
#[ignore = "requires writable ext4, a user systemd scope, and the fixture sleeper"]
fn codex_owner_exit_cleans_one_proven_helper_and_preserves_keeps() {
    check(Agent::Codex);
}

fn check(agent: Agent) {
    let fixture =
        std::env::var_os("AGENTDUST_FIXTURE_SLEEPER").expect("build fixture-sleeper and provide its path");
    let mount = PathBuf::from(std::env::var_os("AGENTDUST_EXT4_TEST_ROOT").unwrap());
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let root = mount.join(format!("agentdust-linux-auto-{}-{nonce}", std::process::id()));
    let data = root.join("data");
    let home = root.join("home");
    let project = home.join("project");
    fs::create_dir_all(&data).unwrap();
    fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir_all(&project).unwrap();
    assert!(SystemVolume.probe(&data).unwrap().supported);

    let secret = load_or_create(&data).unwrap();
    let key = agentdust_core::cwd::cwd_key(&secret, project.to_str().unwrap()).unwrap();
    let session = "12345678-1234-1234-1234-123456789abc";
    let (marker, value, tag_key) = if agent == Agent::Codex {
        (
            "CODEX_SESSION_ID",
            session.to_owned(),
            agentdust_core::tag::codex_key_of(&secret, session.as_bytes()).unwrap(),
        )
    } else {
        let tag = SessionTag::generate().unwrap();
        (
            agentdust_core::tag::ENV_NAME,
            tag.as_str().to_owned(),
            tag.key(&secret),
        )
    };

    let mut harness = Harness::new(fixture).unwrap();
    let target_tree = harness
        .spawn_tree(&ProcSpec::new().seconds(120).spawn(1).env(marker, &value))
        .unwrap();
    let kept_tree = harness
        .spawn_tree(&ProcSpec::new().seconds(120).spawn(1).env(marker, &value))
        .unwrap();
    harness.orphan(target_tree.parent).unwrap();
    harness.orphan(kept_tree.parent).unwrap();
    let target = target_tree.children[0];
    let kept = kept_tree.children[0];
    let unrelated = harness.spawn(&ProcSpec::new().seconds(120)).unwrap();

    let mut owner = Fixture(
        Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let provider = Provider::new().unwrap();
    let ProcessRead::Present(identity) = provider.read(owner.0.id() as i32).unwrap() else {
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
    journal::append(&data, &start).unwrap();
    let mut policy = PolicyGuard::acquire(&data).unwrap();
    policy.policy.enabled = true;
    policy.policy.projects.push(key);
    policy.policy.keep.push(harness.identity(kept).kernel.clone());
    policy.write().unwrap();
    drop(policy);

    let mut worker = Fixture(
        Command::new(env!("CARGO_BIN_EXE_agentdust"))
            .args(["auto", "worker"])
            .env("HOME", &home)
            .env("AGENTDUST_DATA_DIR", &data)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    assert!(wait_until(
        || automatic::status(&data).ok().is_some_and(|status| {
            status["last_report"]["review"].as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item["pid"] == harness.pid(target) && item["class"] == "owned-live")
            })
        }),
        WAIT
    ));
    assert_eq!(
        harness.revalidate(target),
        agentdust_core::revalidate::Revalidation::Match
    );

    journal::append(
        &data,
        &Record {
            kind: Kind::SessionEnd,
            mono_ts: 2,
            wall_ts: 2,
            ..start
        },
    )
    .unwrap();
    drop(owner.0.stdin.take());
    assert!(owner.0.wait().unwrap().success());

    assert!(wait_until(
        || automatic::status(&data).ok().is_some_and(|status| {
            status["last_report"]["recent_results"]
                .as_array()
                .is_some_and(|items| {
                    items.iter().any(|item| {
                        item["pid"] == harness.pid(target)
                            && item["result"] == "terminated"
                            && item["recorded_wall_ms"].as_u64().is_some()
                    })
                })
        }),
        WAIT
    ));
    assert_eq!(
        harness.revalidate(target),
        agentdust_core::revalidate::Revalidation::Gone
    );
    assert_eq!(
        harness.revalidate(kept),
        agentdust_core::revalidate::Revalidation::Match
    );
    assert_eq!(
        harness.revalidate(unrelated),
        agentdust_core::revalidate::Revalidation::Match
    );
    let attempts = fs::read_to_string(data.join("audit.log")).unwrap();
    let attempts: Vec<serde_json::Value> = attempts
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|entry| entry["phase"] == "attempt")
        .collect();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["pid"], harness.pid(target));

    let _ = worker.0.kill();
    let _ = worker.0.wait();
    harness.shutdown().unwrap();
    fs::remove_dir_all(root).unwrap();
}
