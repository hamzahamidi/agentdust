#![cfg(target_os = "linux")]

use std::fs;
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
fn linux_rejects_mutating_commands_without_creating_state() {
    let sandbox = Sandbox::new();

    for command in [
        "apply", "auto", "disk", "hook", "mcp", "metrics", "port", "setup", "status",
    ] {
        let output = sandbox.run(&[command]);
        assert_eq!(output.status.code(), Some(2), "{command}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("read-only"));
    }
    assert!(!sandbox.root.join("xdg-data/agentdust").exists());
}
