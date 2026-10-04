#![allow(dead_code)]

use std::fs::{self, DirBuilder};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

pub const BIN: &str = env!("CARGO_BIN_EXE_agentdust");

const STUB: &str = r#"#!/bin/sh
state="$STUB_CLAUDE_STATE"
mode="${STUB_CLAUDE_MODE:-honest}"
printf '%s\n' "$*" >> "$state/calls.log"
[ "$1" = mcp ] || exit 64
case "$2" in
  get)
    if [ -f "$state/server" ]; then
      command=$(sed -n 1p "$state/server")
      args=$(sed -n 2p "$state/server")
      printf 'agentdust:\n  Scope: User config (available in all your projects)\n  Status: \342\234\230 Failed to connect\n  Type: stdio\n  Command: %s\n  Args: %s\n  Environment:\n\nTo remove this server, run: claude mcp remove agentdust -s user\n' "$command" "$args"
      exit 0
    fi
    echo 'No MCP server named "agentdust". Run `claude mcp add` to add one.' >&2
    exit 1
    ;;
  add)
    if [ "$mode" = add-fails ]; then echo "Error: cannot write the configuration" >&2; exit 1; fi
    if [ -f "$state/server" ]; then echo "MCP server agentdust already exists in user config" >&2; exit 1; fi
    if [ "$mode" = liar ]; then echo "Added stdio MCP server agentdust to user config"; exit 0; fi
    shift 6
    command="$1"
    shift
    printf '%s\n%s\n' "$command" "$*" > "$state/server"
    echo "Added stdio MCP server agentdust to user config"
    exit 0
    ;;
  remove)
    if [ -f "$state/server" ]; then rm "$state/server"; echo "Removed MCP server agentdust from user config"; exit 0; fi
    echo 'No MCP server named "agentdust" in user scope' >&2
    exit 1
    ;;
esac
exit 64
"#;

pub struct Sandbox {
    pub root: PathBuf,
    pub home: PathBuf,
    pub claude: PathBuf,
    pub data: PathBuf,
    pub stubs: PathBuf,
    pub state: PathBuf,
}

impl Sandbox {
    pub fn new(name: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "agentdust-setup-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let sandbox = Self {
            home: root.join("home"),
            claude: root.join("claude"),
            data: root.join("data"),
            stubs: root.join("stubs"),
            state: root.join("state"),
            root,
        };
        for dir in [&sandbox.home, &sandbox.claude, &sandbox.stubs, &sandbox.state] {
            DirBuilder::new().recursive(true).mode(0o755).create(dir).unwrap();
        }
        sandbox
    }

    pub fn with_stub(self) -> Self {
        let path = self.stubs.join("claude");
        fs::write(&path, STUB).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        self
    }

    pub fn settings(&self) -> PathBuf {
        self.claude.join("settings.json")
    }

    pub fn manifest(&self) -> PathBuf {
        self.data.join("manifest.json")
    }

    pub fn command(&self, args: &[&str], mode: &str) -> Command {
        let mut command = Command::new(BIN);
        command
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("CLAUDE_CONFIG_DIR", &self.claude)
            .env("AGENTDUST_DATA_DIR", &self.data)
            .env("PATH", format!("{}:/usr/bin:/bin", self.stubs.display()))
            .env("STUB_CLAUDE_STATE", &self.state)
            .env("STUB_CLAUDE_MODE", mode)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.command(args, "honest").output().unwrap()
    }

    pub fn run_mode(&self, args: &[&str], mode: &str) -> Output {
        self.command(args, mode).output().unwrap()
    }

    pub fn run_without_cli(&self, args: &[&str]) -> Output {
        let mut command = self.command(args, "honest");
        command.env("PATH", "/usr/bin:/bin");
        command.output().unwrap()
    }

    pub fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.state.join("calls.log"))
            .map(|text| text.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    pub fn server(&self) -> Option<(String, String)> {
        let text = fs::read_to_string(self.state.join("server")).ok()?;
        let mut lines = text.lines();
        Some((lines.next()?.to_owned(), lines.next()?.to_owned()))
    }

    pub fn write_settings(&self, text: &str, mode: u32) {
        fs::write(self.settings(), text).unwrap();
        fs::set_permissions(self.settings(), fs::Permissions::from_mode(mode)).unwrap();
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.claude, fs::Permissions::from_mode(0o755));
        if let Ok(entries) = fs::read_dir(&self.claude) {
            for entry in entries.flatten() {
                let _ = fs::set_permissions(entry.path(), fs::Permissions::from_mode(0o600));
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

pub fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

pub fn hook_command() -> String {
    format!("{} hook claude", agentdust_agents::hook_config::shell_quote(BIN))
}

pub fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}
