#![cfg(target_os = "linux")]

use std::fs;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use agentdust_core::automatic::PolicyGuard;
use agentdust_core::journal::volume::{SystemVolume, VolumeProbe};
use agentdust_core::journal::{self, Agent, AgentIdentity, Kind, Record, SCHEMA_VERSION};
use agentdust_core::linux_port_live::LinuxListeners;
use agentdust_core::platform::Provider;
use agentdust_core::port::ListenerSource;
use agentdust_core::provider::{ProcessProvider, ProcessRead};
use agentdust_core::secret::load_or_create;
use agentdust_core::tag::SessionTag;
use agentdust_testkit::Fixture;
use serde_json::Value;

#[test]
fn procfs_listener_inventory_returns_the_exact_current_user_process() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let found = LinuxListeners.listeners(port).unwrap();
    assert!(
        found
            .iter()
            .any(|identity| identity.kernel.pid == std::process::id() as i32),
        "listener PID was not present: {found:?}"
    );
}

#[test]
#[ignore = "requires writable ext4 and the fixture listener"]
fn controlled_port_resolution_obeys_policy_and_records_survivors() {
    for survivor in [false, true] {
        check_resolution(survivor);
    }
}

fn check_resolution(survivor: bool) {
    let fixture =
        std::env::var_os("AGENTDUST_FIXTURE_LISTENER").expect("build fixture-listener and provide its path");
    let volume = PathBuf::from(std::env::var_os("AGENTDUST_EXT4_TEST_ROOT").unwrap());
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let root = volume.join(format!("agentdust-linux-port-{}-{nonce}", std::process::id()));
    let data = root.join("data");
    let home = root.join("home");
    let project = home.join("project");
    fs::create_dir_all(&data).unwrap();
    fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir_all(&project).unwrap();
    assert!(SystemVolume.probe(&data).unwrap().supported);

    let secret = load_or_create(&data).unwrap();
    let cwd_key = agentdust_core::cwd::cwd_key(&secret, project.to_str().unwrap()).unwrap();
    let tag = SessionTag::generate().unwrap();
    let mut owner = Fixture(
        Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let provider = Provider::new().unwrap();
    let ProcessRead::Present(owner_identity) = provider.read(owner.0.id() as i32).unwrap() else {
        panic!("missing owner")
    };
    journal::append(
        &data,
        &Record {
            v: SCHEMA_VERSION,
            kind: Kind::SessionStart,
            agent: Agent::Claude,
            session_id: "linux-port-fixture".into(),
            subagent_id: None,
            agent_identity: Some(AgentIdentity::from_process(&owner_identity).unwrap()),
            tool_use_id: None,
            wall_ts: 1,
            mono_ts: 1,
            boot: owner_identity.kernel.boot_session_uuid.clone(),
            session_tag_key: Some(tag.key(&secret)),
            cwd_key: Some(cwd_key.clone()),
            exe_base: None,
        },
    )
    .unwrap();

    let mut command = Command::new(fixture);
    if survivor {
        command.arg("--ignore-term");
    }
    let mut helper = Fixture(
        command
            .current_dir(&project)
            .env("AGENTDUST_SESSION", tag.as_str())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut line = String::new();
    BufReader::new(helper.0.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let port = line.trim().parse::<u16>().unwrap();
    let ProcessRead::Present(identity) = provider.read(helper.0.id() as i32).unwrap() else {
        panic!("missing listener")
    };

    let mut guard = PolicyGuard::acquire(&data).unwrap();
    guard.policy.enabled = true;
    guard.policy.projects.push(cwd_key.clone());
    guard.write().unwrap();
    drop(guard);

    let live = report(&data, &home, &project, port);
    assert_eq!(live["listeners"][0]["finding"]["class"], "owned-live");
    assert!(helper.0.try_wait().unwrap().is_none());
    owner.0.kill().unwrap();
    let _ = owner.0.wait();

    for mode in ["keep", "pause", "scope"] {
        let mut guard = PolicyGuard::acquire(&data).unwrap();
        guard.policy.enabled = true;
        guard.policy.projects = vec![cwd_key.clone()];
        guard.policy.keep.clear();
        match mode {
            "keep" => guard.policy.keep.push(identity.kernel.clone()),
            "pause" => guard.policy.enabled = false,
            _ => guard.policy.projects.clear(),
        }
        guard.write().unwrap();
        drop(guard);
        let skipped = report(&data, &home, &project, port);
        assert_eq!(skipped["listeners"][0]["result"], "skipped");
        assert!(helper.0.try_wait().unwrap().is_none());
    }

    let mut guard = PolicyGuard::acquire(&data).unwrap();
    guard.policy.enabled = true;
    guard.policy.projects = vec![cwd_key];
    guard.policy.keep.clear();
    guard.write().unwrap();
    drop(guard);
    let result = report(&data, &home, &project, port);
    assert_eq!(
        result["listeners"][0]["result"],
        if survivor { "survivor" } else { "terminated" }
    );
    if survivor {
        assert_eq!(result["state"], "listening");
        let again = report(&data, &home, &project, port);
        assert_eq!(again["listeners"][0]["result"], "handled_elsewhere");
        assert!(helper.0.try_wait().unwrap().is_none());
    } else {
        assert_eq!(result["state"], "no_visible_listener");
        helper.0.wait().unwrap();
    }
    let audit = fs::read_to_string(data.join(agentdust_core::apply::audit::AUDIT_FILE)).unwrap();
    assert_eq!(
        audit
            .lines()
            .filter(|line| serde_json::from_str::<Value>(line).unwrap()["phase"] == "attempt")
            .count(),
        1
    );
    drop(owner);
    drop(helper);
    fs::remove_dir_all(root).unwrap();
}

fn report(data: &Path, home: &Path, project: &Path, port: u16) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_agentdust"))
        .args(["port", &port.to_string(), "--resolve", "--json"])
        .env("HOME", home)
        .env("AGENTDUST_DATA_DIR", data)
        .current_dir(project)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "status: {}; stdout: {}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
