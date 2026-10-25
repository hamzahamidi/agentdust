mod support;
use agentdust_agents::codex_setup::{self, HooksOnlySetupEnv, Mode, SetupEnv};
use agentdust_agents::native_cli::{CliError, CliOutput, CliRunner};
use agentdust_core::manifest::{self, Entry, Manifest, Origin, Resource};
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
            .push(args.iter().copied().map(str::to_string).collect());
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
    for config in [
        "[hooks]\n",
        "hooks = false\n",
        "[hooks]\nstate = false\n",
        "[hooks]\nunknown = []\n",
        "[features]\nhooks = false\n",
        "[features]\nhooks = false\n[hooks.state]\n",
        "[[hooks.SessionStart]]\n[[hooks.SessionStart.hooks]]\ntype = 'command'\ncommand = 'my-script'\n[hooks.state]\n",
    ] {
        let dir = TempDir::new("codex-config-conflict");
        let runner = Codex::default();
        let env = env(&dir, &runner);
        fs::create_dir(&env.config_dir).unwrap();
        fs::write(env.config_dir.join("config.toml"), config).unwrap();
        assert!(codex_setup::run(&env, Mode::Install, &mut |_| panic!("must preflight")).is_err());
        assert!(!env.config_dir.join("hooks.json").exists());
        assert!(!env.data_dir.exists());
        assert_eq!(
            fs::read_to_string(env.config_dir.join("config.toml")).unwrap(),
            config
        );
        assert!(runner.calls.borrow().is_empty());
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
fn codex_trust_state_is_preserved_through_install_check_and_remove() {
    let dir = TempDir::new("codex-trust-state");
    let runner = Codex::default();
    let env = env(&dir, &runner);
    fs::create_dir(&env.config_dir).unwrap();
    let config =
        "[hooks.state.\"/tmp/hooks.json:session_start:0:0\"]\ntrusted_hash = 'sha256:abc'\ndisabled = true\n";
    let path = env.config_dir.join("config.toml");
    fs::write(&path, config).unwrap();
    for mode in [Mode::Install, Mode::Check, Mode::Install, Mode::Remove] {
        assert!(
            codex_setup::run(&env, mode, &mut |_| mode != Mode::Check)
                .unwrap()
                .1
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), config);
    }
    assert!(!runner.registered.get());
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

#[test]
fn hooks_only_setup_preserves_mcp_configuration_and_manifest_without_a_cli() {
    let dir = TempDir::new("codex-hooks-only");
    let config_dir = dir.join("codex");
    let data_dir = dir.join("data");
    fs::create_dir(&config_dir).unwrap();
    let config = "[mcp_servers.agentdust]\ncommand = '/opt/agentdust'\nargs = ['mcp']\n";
    fs::write(config_dir.join("config.toml"), config).unwrap();
    let mcp = Entry {
        resource: Resource::McpServer,
        target: config_dir.join("config.toml").to_string_lossy().into_owned(),
        key: "agentdust".into(),
        origin: Origin::PreExisting,
        command: "/opt/agentdust".into(),
        args: vec!["mcp".into()],
        hash: "00".repeat(32),
        created_hooks_key: false,
        created_event_key: false,
    };
    manifest::store(
        &data_dir,
        &Manifest {
            entries: vec![mcp.clone()],
            ..Manifest::new()
        },
    )
    .unwrap();
    let env = HooksOnlySetupEnv {
        config_dir: config_dir.clone(),
        data_dir: data_dir.clone(),
        exe: "/opt/agentdust".into(),
    };

    assert!(
        codex_setup::run_hooks_only(&env, Mode::Install, &mut |_| true)
            .unwrap()
            .1
    );
    assert!(
        codex_setup::run_hooks_only(&env, Mode::Check, &mut |_| panic!("check asks consent"))
            .unwrap()
            .1
    );
    assert_eq!(
        fs::read_to_string(config_dir.join("config.toml")).unwrap(),
        config
    );
    assert!(manifest::load(&data_dir).unwrap().unwrap().entries.contains(&mcp));
    assert!(
        codex_setup::run_hooks_only(&env, Mode::Remove, &mut |_| true)
            .unwrap()
            .1
    );
    assert_eq!(
        fs::read_to_string(config_dir.join("config.toml")).unwrap(),
        config
    );
    assert_eq!(manifest::load(&data_dir).unwrap().unwrap().entries, vec![mcp]);
}

#[test]
fn removal_without_an_ownership_manifest_preserves_user_files_and_creates_no_state() {
    let dir = TempDir::new("codex-remove-unowned");
    let runner = Codex::default();
    runner.registered.set(true);
    let env = env(&dir, &runner);
    fs::create_dir(&env.config_dir).unwrap();
    let hooks =
        "{\"hooks\":{\"SessionStart\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"user-hook\"}]}]}}\n";
    let config = "[mcp_servers.agentdust]\ncommand = '/opt/user-agentdust'\nargs = ['mcp']\n";
    fs::write(env.config_dir.join("hooks.json"), hooks).unwrap();
    fs::write(env.config_dir.join("config.toml"), config).unwrap();

    assert!(
        codex_setup::run(&env, Mode::Remove, &mut |_| panic!(
            "unowned setup asks for no changes"
        ))
        .unwrap()
        .1
    );
    assert!(
        codex_setup::run_hooks_only(
            &HooksOnlySetupEnv {
                config_dir: env.config_dir.clone(),
                data_dir: env.data_dir.clone(),
                exe: env.exe.clone(),
            },
            Mode::Remove,
            &mut |_| panic!("unowned setup asks for no changes"),
        )
        .unwrap()
        .1
    );
    assert!(runner.registered.get());
    assert!(runner.calls.borrow().is_empty());
    assert_eq!(
        fs::read_to_string(env.config_dir.join("hooks.json")).unwrap(),
        hooks
    );
    assert_eq!(
        fs::read_to_string(env.config_dir.join("config.toml")).unwrap(),
        config
    );
    assert!(!env.data_dir.exists());
}
