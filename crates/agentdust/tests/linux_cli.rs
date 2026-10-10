#![cfg(target_os = "linux")]

use std::fs;
use std::net::TcpListener;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Sandbox {
    root: std::path::PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("agentdust-linux-cli-{}-{nonce}", std::process::id()));
        fs::create_dir(&root).unwrap();
        Self { root }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_agentdust"))
            .args(args)
            .env("HOME", &self.root)
            .env("XDG_DATA_HOME", self.root.join("xdg-data"))
            .env_remove("AGENTDUST_DATA_DIR")
            .output()
            .unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn linux_exposes_version_and_read_only_doctor() {
    let sandbox = Sandbox::new();

    let version = sandbox.run(&["version"]);
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap().trim(),
        format!("agentdust {}", env!("CARGO_PKG_VERSION"))
    );

    for args in [&["doctor"][..], &["doctor", "--json"][..]] {
        let doctor = sandbox.run(args);
        assert!(
            doctor.status.success(),
            "{}",
            String::from_utf8_lossy(&doctor.stderr)
        );
        assert!(!sandbox.root.join("xdg-data/agentdust").exists());
    }

    let report = sandbox.run(&["doctor", "--json"]);
    let report: serde_json::Value = serde_json::from_slice(&report.stdout).unwrap();
    assert!(report.get("cgroup_unit_evidence").is_some());
    assert!(report.get("launchd").is_none());
    assert!(report.get("counts").is_some());
}

#[test]
fn linux_rejects_unavailable_commands_and_gates_hooks_only_setup() {
    let sandbox = Sandbox::new();

    for command in ["disk", "status"] {
        let output = sandbox.run(&[command]);
        assert_eq!(output.status.code(), Some(2), "{command}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("unavailable"));
    }

    let automatic = sandbox.run(&["auto", "status"]);
    assert!(automatic.status.success());
    let report: serde_json::Value = serde_json::from_slice(&automatic.stdout).unwrap();
    assert_eq!(report["enabled"], false);
    assert_eq!(report["worker_running"], false);
    assert!(!sandbox.root.join("xdg-data/agentdust").exists());

    let apply = sandbox.run(&["apply"]);
    assert!(!apply.status.success());
    assert!(String::from_utf8_lossy(&apply.stderr).contains("terminal"));
    assert!(!sandbox.root.join("xdg-data/agentdust").exists());

    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port_number = listener.local_addr().unwrap().port();
    let port_arg = port_number.to_string();
    let port = sandbox.run(&["port", &port_arg, "--json"]);
    let report: serde_json::Value = serde_json::from_slice(&port.stdout).unwrap();
    assert_eq!(report["protocol"], "tcp");
    assert_eq!(report["visibility"], "current_user_visible");
    match report["state"].as_str() {
        Some("listening") => assert!(port.status.success()),
        Some("unavailable") => {
            assert_eq!(port.status.code(), Some(1));
            assert_eq!(report["error"], "listener_inventory_unavailable");
        }
        state => panic!("unexpected port state: {state:?}"),
    }
    assert!(!sandbox.root.join("xdg-data/agentdust").exists());

    let metrics = sandbox.run(&["metrics", "--json"]);
    assert!(metrics.status.success());
    let report: serde_json::Value = serde_json::from_slice(&metrics.stdout).unwrap();
    assert!(report.get("event_count").is_some());
    assert!(!sandbox.root.join("xdg-data/agentdust").exists());

    let hook = sandbox.run(&["hook"]);
    assert!(hook.status.success());
    assert!(!sandbox.root.join("xdg-data/agentdust").exists());

    let check = sandbox.run(&["setup", "--check"]);
    assert!(!check.status.success());

    for args in [
        &["setup", "--remove", "--yes"][..],
        &["setup", "codex", "--remove", "--yes"][..],
    ] {
        let remove = sandbox.run(args);
        assert!(
            remove.status.success(),
            "{}",
            String::from_utf8_lossy(&remove.stderr)
        );
    }
    assert!(!sandbox.root.join("xdg-data/agentdust").exists());
}
