mod support;
use agentdust_agents::codex_setup::{self, Mode, SetupEnv};
use agentdust_agents::native_cli::{CliError, CliOutput, CliRunner};
use agentdust_core::manifest;
use serde_json::json;
use std::cell::{Cell, RefCell};
use std::fs;
use std::os::unix::fs::symlink;
use support::TempDir;

#[derive(Default)]
struct Codex {
    registered: Cell<bool>,
    fail_add: Cell<bool>,
    calls: RefCell<Vec<Vec<String>>>,
}
impl CliRunner for Codex {
    fn run(&self, args: &[&str]) -> Result<CliOutput, CliError> {
        self.calls
            .borrow_mut()
            .push(args.iter().map(|s| s.to_string()).collect());
        let mut output = CliOutput {
            code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        };
        match args.get(1) {
            Some(&"get") if self.registered.get() => output.stdout = json!({"enabled": true, "transport": {"type": "stdio", "command": "/opt/homebrew/bin/agentdust", "args": ["mcp"], "env": null, "env_vars": [], "cwd": null}}).to_string(),
            Some(&"get") => { output.code = Some(1); output.stderr = "Error: No MCP server named 'agentdust' found.\n".into(); },
            Some(&"add") if self.fail_add.get() => output.code = Some(1),
            Some(&"add") => self.registered.set(true),
            Some(&"remove") => self.registered.set(false),
            _ => panic!("unexpected command"),
        }
        Ok(output)
    }
}
fn env<'a>(dir: &TempDir, runner: &'a Codex) -> SetupEnv<'a> {
    SetupEnv {
        config_dir: dir.join("codex"),
        data_dir: dir.join("data"),
        exe: "/opt/homebrew/bin/agentdust".into(),
        runner,
    }
}
#[test]
fn codex_setup_install_check_idempotence_and_remove_preserve_user_entries() {
    let dir = TempDir::new("codex-setup");
    let runner = Codex::default();
    let env = env(&dir, &runner);
    fs::create_dir(&env.config_dir).unwrap();
    let user = "{\n  \"description\": \"user hooks\", \"hooks\": {\"Stop\": [{\"hooks\": [{\"type\":\"command\",\"command\":\"my-script\"}]}]}\n}\n";
    fs::write(env.config_dir.join("hooks.json"), user).unwrap();
    assert!(codex_setup::run(&env, Mode::Install, &mut |_| true).unwrap().1);
    assert!(runner.registered.get());
    let installed = fs::read(env.config_dir.join("hooks.json")).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&installed).unwrap();
    assert_eq!(value["hooks"]["SessionEnd"][0]["hooks"][0]["timeout"], 3);
    assert_eq!(manifest::load(&env.data_dir).unwrap().unwrap().entries.len(), 7);
    assert!(
        codex_setup::run(&env, Mode::Check, &mut |_| panic!("check asks consent"))
            .unwrap()
            .1
    );
    assert!(
        codex_setup::run(&env, Mode::Install, &mut |_| panic!("idempotent asks consent"))
            .unwrap()
            .1
    );
    assert_eq!(installed, fs::read(env.config_dir.join("hooks.json")).unwrap());
    assert!(codex_setup::run(&env, Mode::Remove, &mut |_| true).unwrap().1);
    assert!(!runner.registered.get());
    let after: serde_json::Value =
        serde_json::from_slice(&fs::read(env.config_dir.join("hooks.json")).unwrap()).unwrap();
    assert_eq!(after, serde_json::from_str::<serde_json::Value>(user).unwrap());
}
#[test]
fn codex_decline_and_failed_mcp_registration_leave_hooks_unchanged() {
    for fail in [false, true] {
        let dir = TempDir::new("codex-rollback");
        let runner = Codex::default();
        runner.fail_add.set(fail);
        let env = env(&dir, &runner);
        let result = codex_setup::run(&env, Mode::Install, &mut |_| fail);
        if fail {
            assert!(result.is_err());
        } else {
            assert!(!result.unwrap().1);
        }
        assert!(!env.config_dir.join("hooks.json").exists());
        assert!(manifest::load(&env.data_dir).unwrap().is_none());
    }
}
#[test]
fn codex_inline_hooks_disabled_hooks_and_symlinks_are_preserved() {
    for config in ["[hooks]\n", "[features]\nhooks = false\n"] {
        let dir = TempDir::new("codex-config-conflict");
        let runner = Codex::default();
        let env = env(&dir, &runner);
        fs::create_dir(&env.config_dir).unwrap();
        fs::write(env.config_dir.join("config.toml"), config).unwrap();
        assert!(codex_setup::run(&env, Mode::Install, &mut |_| panic!("must preflight")).is_err());
        assert!(!env.config_dir.join("hooks.json").exists());
        assert!(!env.data_dir.exists());
    }
    let dir = TempDir::new("codex-symlink");
    let runner = Codex::default();
    let env = env(&dir, &runner);
    fs::create_dir(&env.config_dir).unwrap();
    fs::write(dir.join("outside"), "{}").unwrap();
    symlink(dir.join("outside"), env.config_dir.join("hooks.json")).unwrap();
    assert!(codex_setup::run(&env, Mode::Install, &mut |_| panic!("must preflight")).is_err());
    assert_eq!(fs::read_to_string(dir.join("outside")).unwrap(), "{}");
}
#[test]
fn preexisting_codex_mcp_registration_survives_removal() {
    let dir = TempDir::new("codex-preexisting");
    let runner = Codex::default();
    runner.registered.set(true);
    let env = env(&dir, &runner);
    codex_setup::run(&env, Mode::Install, &mut |_| true).unwrap();
    codex_setup::run(&env, Mode::Remove, &mut |_| true).unwrap();
    assert!(runner.registered.get());
}
