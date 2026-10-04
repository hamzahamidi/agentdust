#![allow(dead_code)]

use std::cell::RefCell;
use std::fs::{self, DirBuilder};
use std::ops::Deref;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use agentdust_agents::native_cli::{CliError, CliOutput, CliRunner};

pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "agentdust-agents-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .unwrap();
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Honest,
    Liar,
    AddFails,
    AddsSomethingElse,
    UnreadableGet,
    GetBroken,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Registered {
    pub command: String,
    pub args: Vec<String>,
    pub scope: &'static str,
}

pub struct FakeClaude {
    pub mode: Mode,
    pub state: RefCell<Option<Registered>>,
    pub calls: RefCell<Vec<Vec<String>>>,
}

impl FakeClaude {
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            state: RefCell::new(None),
            calls: RefCell::new(Vec::new()),
        }
    }

    pub fn with(mode: Mode, command: &str, args: &[&str]) -> Self {
        let fake = Self::new(mode);
        *fake.state.borrow_mut() = Some(Registered {
            command: command.to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            scope: "User config (available in all your projects)",
        });
        fake
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.borrow().iter().map(|call| call.join(" ")).collect()
    }

    fn get(&self) -> CliOutput {
        match (&*self.state.borrow(), self.mode) {
            (_, Mode::UnreadableGet) => out(Some(1), "", "Error: could not read configuration\n"),
            (_, Mode::GetBroken) => out(Some(0), "Something entirely different\n", ""),
            (None, _) => out(
                Some(1),
                "",
                "No MCP server named \"agentdust\". Run `claude mcp add` to add one.\n",
            ),
            (Some(server), _) => out(Some(0), &render(server), ""),
        }
    }
}

fn out(code: Option<i32>, stdout: &str, stderr: &str) -> CliOutput {
    CliOutput {
        code,
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
    }
}

pub fn render(server: &Registered) -> String {
    format!(
        "agentdust:\n  Scope: {}\n  Status: \u{2718} Failed to connect\n  Issue: CONNECTION_CLOSED: Connection closed\n  Type: stdio\n  Command: {}\n  Args: {}\n  Environment:\n\nTo remove this server, run: claude mcp remove agentdust -s user\n",
        server.scope,
        server.command,
        server.args.join(" ")
    )
}

impl CliRunner for FakeClaude {
    fn run(&self, args: &[&str]) -> Result<CliOutput, CliError> {
        self.calls
            .borrow_mut()
            .push(args.iter().map(|arg| (*arg).to_owned()).collect());
        match args {
            ["mcp", "get", "agentdust"] => Ok(self.get()),
            [
                "mcp",
                "add",
                "--scope",
                "user",
                "agentdust",
                "--",
                command,
                rest @ ..,
            ] => {
                if self.state.borrow().is_some() {
                    return Ok(out(
                        Some(1),
                        "",
                        "MCP server agentdust already exists in user config\n",
                    ));
                }
                match self.mode {
                    Mode::AddFails => return Ok(out(Some(1), "", "Error: cannot write the configuration\n")),
                    Mode::Liar => {}
                    Mode::AddsSomethingElse => {
                        *self.state.borrow_mut() = Some(Registered {
                            command: "/somewhere/else".to_owned(),
                            args: rest.iter().map(|arg| (*arg).to_owned()).collect(),
                            scope: "User config (available in all your projects)",
                        });
                    }
                    _ => {
                        *self.state.borrow_mut() = Some(Registered {
                            command: (*command).to_owned(),
                            args: rest.iter().map(|arg| (*arg).to_owned()).collect(),
                            scope: "User config (available in all your projects)",
                        });
                    }
                }
                Ok(out(
                    Some(0),
                    "Added stdio MCP server agentdust to user config\n",
                    "",
                ))
            }
            ["mcp", "remove", "agentdust", "--scope", "user"] => {
                if self.state.borrow_mut().take().is_some() {
                    Ok(out(
                        Some(0),
                        "Removed MCP server agentdust from user config\n",
                        "",
                    ))
                } else {
                    Ok(out(
                        Some(1),
                        "",
                        "No MCP server named \"agentdust\" in user scope\n",
                    ))
                }
            }
            other => Ok(out(Some(2), "", &format!("unexpected arguments {other:?}\n"))),
        }
    }
}
