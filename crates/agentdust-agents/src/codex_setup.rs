use std::path::{Path, PathBuf};

use agentdust_core::apply::lock::IdentityLock;
use agentdust_core::manifest::{self, Entry, Manifest, Origin, Resource};
use agentdust_core::safe_open;
use agentdust_core::user_file::{self, Loaded, Snapshot};
use serde_json::Value;

use crate::diff::unified_diff;
use crate::hook_config::{self, HookState};
use crate::native_cli::CliRunner;

pub struct SetupEnv<'a> {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub exe: PathBuf,
    pub runner: &'a dyn CliRunner,
}

pub struct HooksOnlySetupEnv {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub exe: PathBuf,
}

struct RunEnv<'a> {
    config_dir: &'a Path,
    data_dir: &'a Path,
    exe: &'a Path,
    runner: Option<&'a dyn CliRunner>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Install,
    Check,
    Remove,
}

#[derive(Debug, PartialEq, Eq)]
enum Mcp {
    Absent,
    Matching,
    Conflict,
}

struct SetupState {
    path: PathBuf,
    target: String,
    config: Loaded,
    before: Loaded,
    previous: Option<Manifest>,
    next: Manifest,
    command: String,
    mcp_target: String,
    mcp_state: Option<Mcp>,
}

struct ChangePlan {
    after: String,
    complete: bool,
    next: Manifest,
    add_mcp: bool,
    remove_mcp: bool,
    file_changed: bool,
}

fn mcp(env: &RunEnv<'_>) -> Result<Mcp, String> {
    let runner = env.runner.ok_or("Codex CLI is unavailable")?;
    let result = runner
        .run(&["mcp", "get", "agentdust", "--json"])
        .map_err(|e| e.to_string())?;
    if result.code != Some(0) {
        return if result.stderr.trim() == "Error: No MCP server named 'agentdust' found." {
            Ok(Mcp::Absent)
        } else {
            Err("cannot check Codex MCP registration".into())
        };
    }
    let value: Value = serde_json::from_str(&result.stdout).map_err(|_| "invalid Codex MCP response")?;
    let transport = &value["transport"];
    let no_env = |value: &Value| value.is_null() || value.as_object().is_some_and(|v| v.is_empty());
    let matches = value["enabled"] == true
        && transport["type"] == "stdio"
        && transport["command"].as_str() == env.exe.to_str()
        && transport["args"] == serde_json::json!(["mcp"])
        && no_env(&transport["env"])
        && (transport["env_vars"].is_null() || transport["env_vars"] == serde_json::json!([]))
        && transport["cwd"].is_null();
    Ok(if matches { Mcp::Matching } else { Mcp::Conflict })
}

fn change_mcp(env: &RunEnv<'_>, add: bool) -> Result<(), String> {
    let exe = env.exe.to_str().ok_or("binary path is not UTF-8")?;
    let args: &[&str] = if add {
        &["mcp", "add", "agentdust", "--", exe, "mcp"]
    } else {
        &["mcp", "remove", "agentdust"]
    };
    let result = env
        .runner
        .ok_or("Codex CLI is unavailable")?
        .run(args)
        .map_err(|e| e.to_string())?;
    if result.code != Some(0) {
        return Err("Codex MCP configuration command failed".into());
    }
    let expected = if add { Mcp::Matching } else { Mcp::Absent };
    if mcp(env)? != expected {
        return Err("Codex MCP change could not be verified".into());
    }
    Ok(())
}

fn text(loaded: &Loaded) -> Result<Option<&str>, String> {
    loaded
        .bytes()
        .map(|b| std::str::from_utf8(b).map_err(|_| "configuration is not UTF-8".into()))
        .transpose()
}

fn restore(path: &Path, before: &Loaded, after: &str) -> Result<(), String> {
    let ours = Loaded::Present(Snapshot {
        bytes: after.as_bytes().to_vec(),
        mode: 0,
    });
    match before {
        Loaded::Missing => user_file::delete_if_unchanged(path, &ours),
        Loaded::Present(old) => user_file::replace(path, &ours, &old.bytes),
    }
    .map_err(|e| e.to_string())
}

pub fn run(env: &SetupEnv, mode: Mode, ask: &mut dyn FnMut(&str) -> bool) -> Result<(String, bool), String> {
    run_inner(
        &RunEnv {
            config_dir: &env.config_dir,
            data_dir: &env.data_dir,
            exe: &env.exe,
            runner: Some(env.runner),
        },
        mode,
        ask,
        true,
    )
}

pub fn run_hooks_only(
    env: &HooksOnlySetupEnv,
    mode: Mode,
    ask: &mut dyn FnMut(&str) -> bool,
) -> Result<(String, bool), String> {
    run_inner(
        &RunEnv {
            config_dir: &env.config_dir,
            data_dir: &env.data_dir,
            exe: &env.exe,
            runner: None,
        },
        mode,
        ask,
        false,
    )
}

fn run_inner(
    env: &RunEnv<'_>,
    mode: Mode,
    ask: &mut dyn FnMut(&str) -> bool,
    include_mcp: bool,
) -> Result<(String, bool), String> {
    let Some(mut setup) = load_setup(env, mode, include_mcp)? else {
        return Ok((
            "Nothing to remove: no AgentDust setup ownership record exists.\n".into(),
            true,
        ));
    };
    if mode == Mode::Check {
        return check_setup(&setup, include_mcp);
    }
    if setup.mcp_state == Some(Mcp::Conflict) {
        return Err("a different or disabled Codex agentdust MCP entry exists; it is preserved".into());
    }
    let plan = plan_changes(env, mode, include_mcp, &mut setup)?;
    if is_current(&setup, &plan) {
        return Ok((current_message(include_mcp), plan.complete));
    }
    let summary = change_summary(&setup, &plan, include_mcp)?;
    if !ask(&summary) {
        return Ok(("Cancelled. Nothing was changed.\n".into(), false));
    }
    apply_changes(env, &setup, &plan)?;
    Ok((success_message(mode, include_mcp), plan.complete))
}

fn load_setup(env: &RunEnv<'_>, mode: Mode, include_mcp: bool) -> Result<Option<SetupState>, String> {
    let path = env.config_dir.join("hooks.json");
    let target = path.to_str().ok_or("hook path is not UTF-8")?.to_owned();
    let config_path = env.config_dir.join("config.toml");
    let config = user_file::load(&config_path).map_err(|e| e.to_string())?;
    check_codex_config(&config)?;
    let before = user_file::load(&path).map_err(|e| e.to_string())?;
    let previous = manifest::load(env.data_dir).map_err(|e| e.to_string())?;
    if mode == Mode::Remove && previous.is_none() {
        return Ok(None);
    }
    let next = previous.clone().unwrap_or_default();
    let command = format!(
        "{} hook codex",
        hook_config::shell_quote(env.exe.to_str().ok_or("binary path is not UTF-8")?)
    );
    let mcp_state = if include_mcp { Some(mcp(env)?) } else { None };
    Ok(Some(SetupState {
        path,
        target,
        config,
        before,
        previous,
        next,
        command,
        mcp_target: config_path.to_string_lossy().into_owned(),
        mcp_state,
    }))
}

fn check_codex_config(config: &Loaded) -> Result<(), String> {
    let Some(config) = text(config)? else {
        return Ok(());
    };
    let parsed: toml::Value = toml::from_str(config).map_err(|_| "Codex config.toml is invalid")?;
    if parsed.get("hooks").is_some_and(|hooks| {
        !hooks
            .as_table()
            .is_some_and(|table| table.len() == 1 && table.get("state").is_some_and(toml::Value::is_table))
    }) {
        return Err(
            "Codex has inline hooks in config.toml. Move them to hooks.json before using AgentDust setup."
                .into(),
        );
    }
    if parsed
        .get("features")
        .and_then(|features| features.get("hooks"))
        .and_then(toml::Value::as_bool)
        == Some(false)
    {
        return Err("Codex hooks are disabled in config.toml".into());
    }
    Ok(())
}

fn check_setup(setup: &SetupState, include_mcp: bool) -> Result<(String, bool), String> {
    let states = hook_config::inspect(
        text(&setup.before)?,
        &setup.target,
        &setup.next.entries,
        &setup.command,
    )
    .map_err(|e| e.to_string())?;
    let ok = setup.mcp_state != Some(Mcp::Conflict)
        && (!include_mcp || setup.mcp_state == Some(Mcp::Matching))
        && states
            .iter()
            .all(|hook| matches!(hook.state, HookState::Installed | HookState::PreExisting));
    let mut output = format!("Codex hooks: {}\n", setup.path.display());
    for hook in states {
        output.push_str(&format!("{}: {:?}\n", hook.event, hook.state));
    }
    if let Some(state) = &setup.mcp_state {
        output.push_str(&format!("Codex MCP: {state:?}\n"));
    } else {
        output.push_str("Codex MCP: not configured by hooks-only setup\n");
    }
    output.push_str(
        "Hook execution requires trust in Codex /hooks. Configuration checks do not verify trust.\n",
    );
    Ok((output, ok))
}

fn plan_changes(
    env: &RunEnv<'_>,
    mode: Mode,
    include_mcp: bool,
    setup: &mut SetupState,
) -> Result<ChangePlan, String> {
    let (after, entries, complete) = if mode == Mode::Install {
        let installed = hook_config::install_with_timeout(
            text(&setup.before)?,
            &setup.target,
            &setup.next.entries,
            &setup.command,
            3,
        )
        .map_err(|e| e.to_string())?;
        if installed
            .states
            .iter()
            .any(|s| matches!(s.state, HookState::Modified | HookState::Stale))
        {
            return Err(
                "AgentDust Codex hooks were modified or use another binary; they are preserved".into(),
            );
        }
        (installed.text, installed.entries, true)
    } else {
        let removed = hook_config::remove(text(&setup.before)?, &setup.target, &setup.next.entries)
            .map_err(|e| e.to_string())?;
        (removed.text, removed.kept, removed.drift.is_empty())
    };
    setup
        .next
        .entries
        .retain(|entry| !(entry.resource == Resource::Hook && entry.target == setup.target));
    setup.next.entries.extend(entries);
    let owned_mcp = setup
        .next
        .entries
        .iter()
        .find(|entry| {
            entry.resource == Resource::McpServer
                && entry.target == setup.mcp_target
                && entry.key == "agentdust"
        })
        .cloned();
    let add_mcp = mode == Mode::Install && setup.mcp_state == Some(Mcp::Absent);
    let remove_mcp = mode == Mode::Remove
        && setup.mcp_state == Some(Mcp::Matching)
        && owned_mcp.as_ref().is_some_and(|entry| {
            entry.origin == Origin::Created
                && entry.command == env.exe.to_string_lossy()
                && entry.args == ["mcp"]
        });
    if include_mcp {
        setup.next.entries.retain(|entry| {
            !(entry.resource == Resource::McpServer
                && entry.target == setup.mcp_target
                && entry.key == "agentdust")
        });
    }
    if include_mcp && mode == Mode::Install {
        setup.next.entries.push(Entry {
            resource: Resource::McpServer,
            target: setup.mcp_target.clone(),
            key: "agentdust".into(),
            origin: if add_mcp {
                Origin::Created
            } else {
                owned_mcp
                    .as_ref()
                    .map_or(Origin::PreExisting, |entry| entry.origin)
            },
            command: env.exe.to_string_lossy().into_owned(),
            args: vec!["mcp".into()],
            hash: hook_config::group_hash(&serde_json::json!({"command": env.exe, "args": ["mcp"]})),
            created_hooks_key: false,
            created_event_key: false,
        });
    }
    let file_changed = setup.before.bytes().unwrap_or_default() != after.as_bytes();
    Ok(ChangePlan {
        after,
        complete,
        next: setup.next.clone(),
        add_mcp,
        remove_mcp,
        file_changed,
    })
}

fn is_current(setup: &SetupState, plan: &ChangePlan) -> bool {
    !plan.file_changed && !plan.add_mcp && !plan.remove_mcp && setup.previous.as_ref() == Some(&plan.next)
}

fn current_message(include_mcp: bool) -> String {
    if include_mcp {
        "Codex configuration is current. Review new hooks in Codex /hooks before they run.\n".into()
    } else {
        "Codex hooks are current. MCP configuration was left untouched.\n".into()
    }
}

fn change_summary(setup: &SetupState, plan: &ChangePlan, include_mcp: bool) -> Result<String, String> {
    let mut summary = format!(
        "Codex hook changes in {}\n{}",
        setup.path.display(),
        unified_diff(
            text(&setup.before)?.unwrap_or_default(),
            &plan.after,
            "hooks.json",
            "hooks.json"
        )
    );
    if !include_mcp {
        summary.push_str("Linux hooks-only setup leaves MCP configuration untouched.\n");
    }
    if plan.add_mcp {
        summary.push_str("Registers agentdust mcp with the Codex CLI.\n");
    }
    if plan.remove_mcp {
        summary.push_str("Removes the setup-owned Codex MCP entry.\n");
    }
    Ok(summary)
}

fn apply_changes(env: &RunEnv<'_>, setup: &SetupState, plan: &ChangePlan) -> Result<(), String> {
    safe_open::ensure_dir(env.data_dir).map_err(|e| e.to_string())?;
    let _lock = IdentityLock::try_acquire(&env.data_dir.join("setup-locks"), "codex")
        .map_err(|e| e.to_string())?
        .ok_or("Codex setup is busy")?;
    if manifest::load(env.data_dir).map_err(|e| e.to_string())? != setup.previous {
        return Err("setup manifest changed after it was read".into());
    }
    if user_file::load(&env.config_dir.join("config.toml")).map_err(|e| e.to_string())? != setup.config {
        return Err("Codex configuration changed after it was read".into());
    }
    if plan.file_changed {
        user_file::replace(&setup.path, &setup.before, plan.after.as_bytes()).map_err(|e| e.to_string())?;
    }
    if let Err(reason) = apply_manifest_and_mcp(env, plan) {
        let failures = rollback_changes(env, setup, plan);
        return Err(format!("{reason}; rollback failures: {failures:?}"));
    }
    Ok(())
}

fn apply_manifest_and_mcp(env: &RunEnv<'_>, plan: &ChangePlan) -> Result<(), String> {
    if plan.add_mcp || plan.remove_mcp {
        change_mcp(env, plan.add_mcp)?;
    }
    manifest::store(env.data_dir, &plan.next).map_err(|e| e.to_string())
}

fn rollback_changes(env: &RunEnv<'_>, setup: &SetupState, plan: &ChangePlan) -> Vec<String> {
    let mut failures = Vec::new();
    if plan.add_mcp
        && mcp(env) == Ok(Mcp::Matching)
        && let Err(error) = change_mcp(env, false)
    {
        failures.push(error);
    }
    if plan.remove_mcp
        && mcp(env) == Ok(Mcp::Absent)
        && let Err(error) = change_mcp(env, true)
    {
        failures.push(error);
    }
    if plan.file_changed
        && let Err(error) = restore(&setup.path, &setup.before, &plan.after)
    {
        failures.push(error);
    }
    failures
}

fn success_message(mode: Mode, include_mcp: bool) -> String {
    if mode == Mode::Install {
        if include_mcp {
            "Codex hooks and MCP configured. Review and trust the six AgentDust hooks in Codex /hooks, then start a new chat. Automatic cleanup waits until all recorded Codex host processes are gone.\n".into()
        } else {
            "Codex hooks configured. MCP configuration was left untouched. Review and trust the six AgentDust hooks in Codex /hooks, then start a new chat.\n".into()
        }
    } else {
        "Setup-owned Codex hooks removed. MCP configuration was left untouched. Modified hooks are preserved.\n".into()
    }
}
