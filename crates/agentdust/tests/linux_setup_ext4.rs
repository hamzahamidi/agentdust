#![cfg(target_os = "linux")]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use agentdust_core::journal::volume::{SystemVolume, VolumeProbe};
use agentdust_core::journal::{Agent, Journal, Kind};

#[tokio::test]
#[ignore = "requires a writable ext4 test mount"]
async fn claude_codex_setup_hooks_and_mcp_work_on_ext4() {
    let root = test_root();
    let home = root.join("home");
    let data = root.join("data");
    let claude = root.join("claude");
    let codex = root.join("codex");
    let claude_bin = fake_claude_cli(&root);
    let codex_bin = fake_codex_cli(&root);
    fs::create_dir_all(&home).unwrap();
    fs::create_dir(&claude).unwrap();
    fs::create_dir(&codex).unwrap();
    fs::create_dir(&data).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).unwrap();
    let facts = SystemVolume.probe(&data).unwrap();
    assert_eq!(facts.name, "ext4");
    assert!(facts.supported);

    let claude_install = run_with_path(
        &["setup", "--yes"],
        &home,
        &data,
        Some(&claude),
        None,
        &claude_bin,
    );
    assert_success(&claude_install);
    let settings = fs::read_to_string(claude.join("settings.json")).unwrap();
    assert!(settings.contains("hook claude"));
    assert!(
        claude_install
            .stdout
            .windows(b"MCP server".len())
            .any(|part| part == b"MCP server")
    );

    let claude_check = run_with_path(
        &["setup", "--check"],
        &home,
        &data,
        Some(&claude),
        None,
        &claude_bin,
    );
    assert_success(&claude_check);
    let claude_hook = run_input(
        &["hook", "claude"],
        &home,
        &data,
        Some(&claude),
        None,
        br#"{"session_id":"claude-ext4","hook_event_name":"SessionStart"}"#,
    );
    assert_success(&claude_hook);

    let mcp_config = "[mcp_servers.other]\ncommand = '/opt/other'\nargs = ['mcp']\n";
    fs::write(codex.join("config.toml"), mcp_config).unwrap();
    let codex_install = run_with_path(
        &["setup", "codex", "--yes"],
        &home,
        &data,
        None,
        Some(&codex),
        &codex_bin,
    );
    assert_success(&codex_install);
    assert!(
        fs::read_to_string(codex.join("hooks.json"))
            .unwrap()
            .contains("hook codex")
    );
    assert!(codex.join("agentdust-command").is_file());
    assert_eq!(fs::read_to_string(codex.join("config.toml")).unwrap(), mcp_config);
    let codex_check = run_with_path(
        &["setup", "codex", "--check"],
        &home,
        &data,
        None,
        Some(&codex),
        &codex_bin,
    );
    assert_success(&codex_check);
    let codex_hook = run_input(
        &["hook", "codex"],
        &home,
        &data,
        None,
        Some(&codex),
        br#"{"session_id":"00000000-0000-4000-8000-000000000000","hook_event_name":"SessionStart"}"#,
    );
    assert_success(&codex_hook);

    let records = Journal::new(&data).read().unwrap().records;
    assert!(
        records
            .iter()
            .any(|record| record.agent == Agent::Claude && record.kind == Kind::SessionStart)
    );
    assert!(
        records
            .iter()
            .any(|record| record.agent == Agent::Codex && record.kind == Kind::SessionStart)
    );

    use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentdust"));
    command
        .arg("mcp")
        .env("HOME", &home)
        .env("AGENTDUST_DATA_DIR", &data)
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME");
    let service = ().serve(TokioChildProcess::new(command).unwrap()).await.unwrap();
    let tools = service.list_all_tools().await.unwrap();
    for name in [
        "agentdust_doctor",
        "agentdust_plan",
        "agentdust_apply",
        "agentdust_auto_status",
        "agentdust_metrics",
        "agentdust_port",
    ] {
        assert!(tools.iter().any(|tool| tool.name == name), "missing {name}");
    }
    assert!(tools.iter().all(|tool| tool.name != "agentdust_disk"));
    let doctor = service
        .call_tool(CallToolRequestParams::new("agentdust_doctor"))
        .await
        .unwrap();
    let doctor: serde_json::Value = serde_json::from_str(&doctor.content[0].as_text().unwrap().text).unwrap();
    assert!(doctor.get("cgroup_unit_evidence").is_some());
    let port_result = service
        .call_tool(
            CallToolRequestParams::new("agentdust_port")
                .with_arguments(serde_json::json!({"port": port}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    let port_result: serde_json::Value =
        serde_json::from_str(&port_result.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(port_result["state"], "listening");
    service.cancel().await.unwrap();

    let codex_remove = run_with_path(
        &["setup", "codex", "--remove", "--yes"],
        &home,
        &data,
        None,
        Some(&codex),
        &codex_bin,
    );
    assert_success(&codex_remove);
    assert_eq!(fs::read_to_string(codex.join("config.toml")).unwrap(), mcp_config);
    let claude_remove = run_with_path(
        &["setup", "--remove", "--yes"],
        &home,
        &data,
        Some(&claude),
        None,
        &claude_bin,
    );
    assert_success(&claude_remove);
    assert!(
        !fs::read_to_string(claude.join("settings.json"))
            .unwrap_or_default()
            .contains("hook claude")
    );
    assert!(!home.join(".agentdust-claude-state").join("server").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "requires an ext4 test mount remounted read only"]
fn read_only_ext4_refuses_setup_checks_and_hook_writes_but_allows_removal() {
    let mount = ext4_root();
    let data = mount.join(format!("agentdust-read-only-{}", std::process::id()));
    let facts = SystemVolume.probe(&mount).unwrap();
    assert_eq!(facts.name, "ext4");
    assert!(!facts.supported);

    let home = std::env::temp_dir().join(format!("agentdust-ro-home-{}", std::process::id()));
    fs::create_dir_all(&home).unwrap();
    let check = run(&["setup", "--check"], &home, &data, None, None);
    assert!(!check.status.success());
    assert!(String::from_utf8_lossy(&check.stderr).contains("requires writable ext4"));
    assert!(!data.exists());

    let hook = run_input(
        &["hook", "claude"],
        &home,
        &data,
        None,
        None,
        br#"{"session_id":"read-only","hook_event_name":"SessionStart"}"#,
    );
    assert_success(&hook);
    assert!(!data.exists());

    for args in [
        &["setup", "--remove", "--yes"][..],
        &["setup", "codex", "--remove", "--yes"][..],
    ] {
        let remove = run(args, &home, &data, None, Some(&home.join("codex")));
        assert_success(&remove);
    }
    let _ = fs::remove_dir_all(home);
}

fn run(args: &[&str], home: &Path, data: &Path, claude: Option<&Path>, codex: Option<&Path>) -> Output {
    command(args, home, data, claude, codex, None, None)
}

fn run_with_path(
    args: &[&str],
    home: &Path,
    data: &Path,
    claude: Option<&Path>,
    codex: Option<&Path>,
    bin: &Path,
) -> Output {
    command(args, home, data, claude, codex, None, Some(bin))
}

fn run_input(
    args: &[&str],
    home: &Path,
    data: &Path,
    claude: Option<&Path>,
    codex: Option<&Path>,
    input: &[u8],
) -> Output {
    command(args, home, data, claude, codex, Some(input), None)
}

fn command(
    args: &[&str],
    home: &Path,
    data: &Path,
    claude: Option<&Path>,
    codex: Option<&Path>,
    input: Option<&[u8]>,
    bin: Option<&Path>,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentdust"));
    command
        .args(args)
        .env("HOME", home)
        .env("AGENTDUST_DATA_DIR", data)
        .env_remove("XDG_DATA_HOME")
        .env_remove("CODEX_HOME")
        .env_remove("CLAUDE_CONFIG_DIR");
    if let Some(claude) = claude {
        command.env("CLAUDE_CONFIG_DIR", claude);
    }
    if let Some(codex) = codex {
        command.env("CODEX_HOME", codex);
    }
    if let Some(bin) = bin {
        command.env("PATH", format!("{}:/usr/bin:/bin", bin.display()));
    }
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    if let Some(input) = input {
        child.stdin.take().unwrap().write_all(input).unwrap();
    }
    child.wait_with_output().unwrap()
}

fn fake_codex_cli(root: &Path) -> PathBuf {
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let path = bin.join("codex");
    fs::write(
        &path,
        br#"#!/bin/sh
set -eu
case "$1:$2" in
  mcp:get)
    if [ -f "$CODEX_HOME/agentdust-command" ]; then
      command=$(cat "$CODEX_HOME/agentdust-command")
      printf '{"enabled":true,"transport":{"type":"stdio","command":"%s","args":["mcp"],"env":null,"env_vars":[],"cwd":null}}\n' "$command"
    else
      printf "%s\\n" "Error: No MCP server named 'agentdust' found." >&2
      exit 1
    fi
    ;;
  mcp:add)
    printf '%s' "$5" > "$CODEX_HOME/agentdust-command"
    ;;
  mcp:remove)
    rm -f "$CODEX_HOME/agentdust-command"
    ;;
  *)
    exit 2
    ;;
esac
"#,
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

fn fake_claude_cli(root: &Path) -> PathBuf {
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let path = bin.join("claude");
    fs::write(
        &path,
        br#"#!/bin/sh
set -eu
state="$HOME/.agentdust-claude-state"
mkdir -p "$state"
case "$1:$2" in
  mcp:get)
    if [ -f "$state/server" ]; then
      command=$(sed -n 1p "$state/server")
      args=$(sed -n 2p "$state/server")
      printf 'agentdust:\n  Scope: User config (available in all your projects)\n  Status: Failed to connect\n  Type: stdio\n  Command: %s\n  Args: %s\n  Environment:\n\n' "$command" "$args"
    else
      echo 'No MCP server named "agentdust". Run `claude mcp add` to add one.' >&2
      exit 1
    fi
    ;;
  mcp:add)
    shift 6
    command="$1"
    shift
    printf '%s\n%s\n' "$command" "$*" > "$state/server"
    echo "Added stdio MCP server agentdust to user config"
    ;;
  mcp:remove)
    rm -f "$state/server"
    echo "Removed MCP server agentdust from user config"
    ;;
  *)
    exit 2
    ;;
esac
"#,
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn test_root() -> PathBuf {
    let mount = ext4_root();
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let root = mount.join(format!("agentdust-linux-setup-{}-{nonce}", std::process::id()));
    fs::create_dir(&root).unwrap();
    root
}

fn ext4_root() -> PathBuf {
    PathBuf::from(std::env::var_os("AGENTDUST_EXT4_TEST_ROOT").unwrap())
}
