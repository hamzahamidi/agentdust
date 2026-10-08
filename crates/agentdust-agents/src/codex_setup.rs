use std::path::{Path, PathBuf};

use agentdust_core::apply::lock::IdentityLock;
use agentdust_core::manifest::{self, Entry, Origin, Resource};
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

fn mcp(env: &SetupEnv) -> Result<Mcp, String> {
    let result = env
        .runner
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

fn change_mcp(env: &SetupEnv, add: bool) -> Result<(), String> {
    let exe = env.exe.to_str().ok_or("binary path is not UTF-8")?;
    let args: &[&str] = if add {
        &["mcp", "add", "agentdust", "--", exe, "mcp"]
    } else {
        &["mcp", "remove", "agentdust"]
    };
    let result = env.runner.run(args).map_err(|e| e.to_string())?;
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
    let path = env.config_dir.join("hooks.json");
    let target = path.to_str().ok_or("hook path is not UTF-8")?;
    let config = user_file::load(&env.config_dir.join("config.toml")).map_err(|e| e.to_string())?;
    if let Some(config) = text(&config)? {
        let parsed: toml::Value = toml::from_str(config).map_err(|_| "Codex config.toml is invalid")?;
        if parsed.get("hooks").is_some_and(|hooks| {
            !hooks.as_table().is_some_and(|table| {
                table.len() == 1 && table.get("state").is_some_and(toml::Value::is_table)
            })
        }) {
            return Err("Codex has inline hooks in config.toml. Move them to hooks.json before using AgentDust setup.".into());
        }
        if parsed
            .get("features")
            .and_then(|f| f.get("hooks"))
            .and_then(toml::Value::as_bool)
            == Some(false)
        {
            return Err("Codex hooks are disabled in config.toml".into());
        }
    }
    let before = user_file::load(&path).map_err(|e| e.to_string())?;
    let previous = manifest::load(&env.data_dir).map_err(|e| e.to_string())?;
    let mut next = previous.clone().unwrap_or_default();
    let command = format!(
        "{} hook codex",
        hook_config::shell_quote(env.exe.to_str().ok_or("binary path is not UTF-8")?)
    );
    let state = mcp(env)?;
    let states =
        hook_config::inspect(text(&before)?, target, &next.entries, &command).map_err(|e| e.to_string())?;
    if mode == Mode::Check {
        let ok = state == Mcp::Matching
            && states
                .iter()
                .all(|s| matches!(s.state, HookState::Installed | HookState::PreExisting));
        let mut out = format!("Codex hooks: {}\n", path.display());
        for hook in states {
            out.push_str(&format!("{}: {:?}\n", hook.event, hook.state));
        }
        out.push_str(&format!("Codex MCP: {state:?}\nHook execution requires trust in Codex /hooks. Configuration checks do not verify trust.\n"));
        return Ok((out, ok));
    }
    if state == Mcp::Conflict {
        return Err("a different or disabled Codex agentdust MCP entry exists; it is preserved".into());
    }
    let mcp_target = env.config_dir.join("config.toml").to_string_lossy().into_owned();
    let owned_mcp = next
        .entries
        .iter()
        .find(|e| e.resource == Resource::McpServer && e.target == mcp_target && e.key == "agentdust")
        .cloned();
    let (after, entries, complete) = if mode == Mode::Install {
        let installed = hook_config::install_with_timeout(text(&before)?, target, &next.entries, &command, 3)
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
        let removed =
            hook_config::remove(text(&before)?, target, &next.entries).map_err(|e| e.to_string())?;
        (removed.text, removed.kept, removed.drift.is_empty())
    };
    next.entries
        .retain(|e| !(e.resource == Resource::Hook && e.target == target));
    next.entries.extend(entries);
    let add = mode == Mode::Install && state == Mcp::Absent;
    let remove = mode == Mode::Remove
        && state == Mcp::Matching
        && owned_mcp.as_ref().is_some_and(|e| {
            e.origin == Origin::Created && e.command == env.exe.to_string_lossy() && e.args == ["mcp"]
        });
    next.entries
        .retain(|e| !(e.resource == Resource::McpServer && e.target == mcp_target && e.key == "agentdust"));
    if mode == Mode::Install {
        next.entries.push(Entry {
            resource: Resource::McpServer,
            target: mcp_target,
            key: "agentdust".into(),
            origin: if add {
                Origin::Created
            } else {
                owned_mcp.as_ref().map_or(Origin::PreExisting, |e| e.origin)
            },
            command: env.exe.to_string_lossy().into_owned(),
            args: vec!["mcp".into()],
            hash: hook_config::group_hash(&serde_json::json!({"command": env.exe, "args": ["mcp"]})),
            created_hooks_key: false,
            created_event_key: false,
        });
    }
    let file_changed = before.bytes().unwrap_or_default() != after.as_bytes();
    if !file_changed && !add && !remove && previous.as_ref() == Some(&next) {
        return Ok((
            "Codex configuration is current. Review new hooks in Codex /hooks before they run.\n".into(),
            complete,
        ));
    }
    let mut plan = format!(
        "Codex hook changes in {}\n{}",
        path.display(),
        unified_diff(
            text(&before)?.unwrap_or_default(),
            &after,
            "hooks.json",
            "hooks.json"
        )
    );
    if add {
        plan.push_str("Registers agentdust mcp with the Codex CLI.\n");
    }
    if remove {
        plan.push_str("Removes the setup-owned Codex MCP entry.\n");
    }
    if !ask(&plan) {
        return Ok(("Cancelled. Nothing was changed.\n".into(), false));
    }
    safe_open::ensure_dir(&env.data_dir).map_err(|e| e.to_string())?;
    let _lock = IdentityLock::try_acquire(&env.data_dir.join("setup-locks"), "codex")
        .map_err(|e| e.to_string())?
        .ok_or("Codex setup is busy")?;
    if manifest::load(&env.data_dir).map_err(|e| e.to_string())? != previous {
        return Err("setup manifest changed after it was read".into());
    }
    if user_file::load(&env.config_dir.join("config.toml")).map_err(|e| e.to_string())? != config {
        return Err("Codex configuration changed after it was read".into());
    }
    if file_changed {
        user_file::replace(&path, &before, after.as_bytes()).map_err(|e| e.to_string())?;
    }
    let applied = (|| {
        if add || remove {
            change_mcp(env, add)?;
        }
        manifest::store(&env.data_dir, &next).map_err(|e| e.to_string())
    })();
    if let Err(reason) = applied {
        let mut failures = Vec::new();
        if add
            && mcp(env) == Ok(Mcp::Matching)
            && let Err(e) = change_mcp(env, false)
        {
            failures.push(e);
        }
        if remove
            && mcp(env) == Ok(Mcp::Absent)
            && let Err(e) = change_mcp(env, true)
        {
            failures.push(e);
        }
        if file_changed && let Err(e) = restore(&path, &before, &after) {
            failures.push(e);
        }
        return Err(format!("{reason}; rollback failures: {failures:?}"));
    }
    Ok((if mode == Mode::Install {
        "Codex hooks and MCP configured. Review and trust the six AgentDust hooks in Codex /hooks, then start a new chat. Automatic cleanup waits until all recorded Codex host processes are gone.\n"
    } else { "Setup-owned Codex entries removed. Modified hooks are preserved.\n" }.into(), complete))
}
