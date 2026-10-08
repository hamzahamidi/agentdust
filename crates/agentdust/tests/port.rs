#![cfg(target_os = "macos")]
mod setup_support;

use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::process::{Command, Stdio};

use agentdust_core::automatic::PolicyGuard;
use agentdust_core::darwin::DarwinProvider;
use agentdust_core::journal::{self, Agent, AgentIdentity, Kind, Record, SCHEMA_VERSION};
use agentdust_core::provider::{ProcessProvider, ProcessRead};
use agentdust_core::tag::SessionTag;
use agentdust_testkit::Fixture;
use serde_json::Value;
use setup_support::{Sandbox, code};

#[test]
fn rejects_invalid_ports_and_flags_without_state_changes() {
    let sandbox = Sandbox::new("port-arguments");
    for args in [
        vec!["port"],
        vec!["port", "0"],
        vec!["port", "65536"],
        vec!["port", "3000", "--kill"],
        vec!["port", "3000", "--json", "--json"],
    ] {
        assert_eq!(code(&sandbox.run(&args)), 2);
        assert!(!sandbox.data.exists());
    }
}

#[test]
fn native_listener_diagnosis_and_resolution_protect_the_test_process() {
    let sandbox = Sandbox::new("port-protected");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port().to_string();
    for resolve in [false, true] {
        let args = if resolve {
            vec!["port", &port, "--json", "--resolve"]
        } else {
            vec!["port", &port, "--json"]
        };
        let output = sandbox.run(&args);
        assert_eq!(code(&output), 0, "{}", String::from_utf8_lossy(&output.stderr));
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["state"], "listening");
        let found = report["listeners"]
            .as_array()
            .unwrap()
            .iter()
            .find(|value| value["pid"] == std::process::id())
            .unwrap();
        assert_eq!(found["finding"]["class"], "managed");
        assert_eq!(found["result"], "report_only");
        assert!(std::net::TcpStream::connect(listener.local_addr().unwrap()).is_ok());
    }
}

#[tokio::test]
async fn mcp_port_defaults_to_diagnosis_and_rejects_policy_arguments() {
    use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
    let sandbox = Sandbox::new("port-mcp");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut command = tokio::process::Command::new(setup_support::BIN);
    command
        .arg("mcp")
        .env("HOME", &sandbox.home)
        .env("AGENTDUST_DATA_DIR", &sandbox.data);
    let service = ().serve(TokioChildProcess::new(command).unwrap()).await.unwrap();
    let tools = service.list_all_tools().await.unwrap();
    let tool = tools.iter().find(|tool| tool.name == "agentdust_port").unwrap();
    assert_eq!(tool.annotations.as_ref().unwrap().destructive_hint, Some(true));
    for args in [
        serde_json::json!({"port":0}),
        serde_json::json!({"port":port,"enable":true}),
        serde_json::json!({"port":port,"resolve":"yes"}),
    ] {
        assert!(
            service
                .call_tool(
                    CallToolRequestParams::new("agentdust_port")
                        .with_arguments(args.as_object().unwrap().clone())
                )
                .await
                .is_err()
        );
    }
    let result = service
        .call_tool(
            CallToolRequestParams::new("agentdust_port")
                .with_arguments(serde_json::json!({"port":port}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    let value: Value = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(value["state"], "listening");
    assert!(!sandbox.data.exists());
    service.cancel().await.unwrap();
}

#[test]
#[ignore = "requires the explicitly built isolated TCP listener fixture"]
fn controlled_listener_live_keep_pause_scope_cleanup_and_survivor_receipt() {
    for survivor in [false, true] {
        check_listener(survivor);
    }
}

fn report(sandbox: &Sandbox, port: u16, resolve: bool) -> Value {
    let port = port.to_string();
    let args = if resolve {
        vec!["port", &port, "--resolve", "--json"]
    } else {
        vec!["port", &port, "--json"]
    };
    let output = sandbox.run(&args);
    assert_eq!(code(&output), 0, "{}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}

fn check_listener(survivor: bool) {
    let fixture =
        std::env::var_os("AGENTDUST_FIXTURE_LISTENER").expect("build fixture-listener and provide its path");
    let sandbox = Sandbox::new("port-owned");
    let secret = agentdust_core::secret::load_or_create(&sandbox.data).unwrap();
    let tag = SessionTag::generate().unwrap();
    let mut owner = Fixture(
        Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let provider = DarwinProvider::new().unwrap();
    let ProcessRead::Present(owner_identity) = provider.read(owner.0.id() as i32).unwrap() else {
        panic!("missing owner")
    };
    let key = agentdust_core::cwd::cwd_key(&secret, sandbox.home.to_str().unwrap()).unwrap();
    journal::append(
        &sandbox.data,
        &Record {
            v: SCHEMA_VERSION,
            kind: Kind::SessionStart,
            agent: Agent::Claude,
            session_id: "port-fixture".into(),
            subagent_id: None,
            agent_identity: Some(AgentIdentity::from_process(&owner_identity).unwrap()),
            tool_use_id: None,
            wall_ts: 1,
            mono_ts: 1,
            boot: owner_identity.kernel.boot_session_uuid.clone(),
            session_tag_key: Some(tag.key(&secret)),
            cwd_key: Some(key.clone()),
            exe_base: None,
        },
    )
    .unwrap();
    let mut command = Command::new(fixture);
    if survivor {
        command.arg("--ignore-term");
    } else {
        command.arg("--ipv6");
    }
    let mut helper = Fixture(
        command
            .env_remove("CODEX_SESSION_ID")
            .env_remove("CODEX_THREAD_ID")
            .env("AGENTDUST_SESSION", tag.as_str())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut line = String::new();
    BufReader::new(helper.0.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let port = line.trim().parse().unwrap();
    let ProcessRead::Present(identity) = provider.read(helper.0.id() as i32).unwrap() else {
        panic!("missing helper")
    };
    let mut guard = PolicyGuard::acquire(&sandbox.data).unwrap();
    guard.policy.enabled = true;
    guard.policy.projects.push(key.clone());
    guard.write().unwrap();
    drop(guard);
    let live = report(&sandbox, port, true);
    assert_eq!(live["listeners"][0]["finding"]["class"], "owned-live");
    assert!(helper.0.try_wait().unwrap().is_none());
    owner.0.kill().unwrap();
    owner.0.wait().unwrap();
    for mode in ["keep", "pause", "scope"] {
        let mut guard = PolicyGuard::acquire(&sandbox.data).unwrap();
        guard.policy.keep.clear();
        guard.policy.enabled = true;
        guard.policy.projects = vec![key.clone()];
        match mode {
            "keep" => guard.policy.keep.push(identity.kernel.clone()),
            "pause" => guard.policy.enabled = false,
            _ => guard.policy.projects.clear(),
        };
        guard.write().unwrap();
        drop(guard);
        let value = report(&sandbox, port, true);
        assert_eq!(value["state"], "listening");
        assert_eq!(value["listeners"][0]["result"], "skipped");
        assert!(helper.0.try_wait().unwrap().is_none());
    }
    let mut guard = PolicyGuard::acquire(&sandbox.data).unwrap();
    guard.policy.enabled = true;
    guard.policy.projects = vec![key];
    guard.write().unwrap();
    drop(guard);
    let result = report(&sandbox, port, true);
    assert_eq!(
        result["listeners"][0]["result"],
        if survivor { "survivor" } else { "terminated" }
    );
    if survivor {
        assert_eq!(result["state"], "listening");
        let again = report(&sandbox, port, true);
        assert_eq!(again["listeners"][0]["result"], "handled_elsewhere");
        assert!(helper.0.try_wait().unwrap().is_none());
    } else {
        assert_eq!(result["state"], "no_visible_listener");
        helper.0.wait().unwrap();
    }
    let audit = std::fs::read_to_string(sandbox.data.join(agentdust_core::apply::audit::AUDIT_FILE)).unwrap();
    assert_eq!(
        audit
            .lines()
            .filter(|line| serde_json::from_str::<Value>(line).unwrap()["phase"] == "attempt")
            .count(),
        1
    );
}
