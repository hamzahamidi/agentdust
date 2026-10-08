use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use agentdust_core::digest::sha256_hex;
use thiserror::Error;

pub const SERVER_NAME: &str = "agentdust";
pub const SERVER_ARGS: &str = "mcp";
const OUTPUT_GRACE: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Error)]
pub enum CliError {
    #[error("could not start the claude CLI: {0}")]
    Spawn(String),
    #[error("the claude CLI did not finish within {:.1} seconds", .0.as_secs_f64())]
    TimedOut(Duration),
}

pub trait CliRunner {
    fn run(&self, args: &[&str]) -> Result<CliOutput, CliError>;
}

pub struct SystemRunner {
    program: PathBuf,
    config_dir: Option<PathBuf>,
    config_env: &'static str,
    timeout: Duration,
}

impl SystemRunner {
    pub fn new(program: PathBuf, config_dir: Option<PathBuf>, timeout: Duration) -> Self {
        Self {
            program,
            config_dir,
            config_env: "CLAUDE_CONFIG_DIR",
            timeout,
        }
    }
}

impl SystemRunner {
    pub fn for_codex(program: PathBuf, config_dir: PathBuf, timeout: Duration) -> Self {
        Self {
            program,
            config_dir: Some(config_dir),
            config_env: "CODEX_HOME",
            timeout,
        }
    }
}

struct Captured {
    bytes: Arc<Mutex<Vec<u8>>>,
    finished: Receiver<()>,
}

impl Captured {
    fn text(self) -> String {
        let _ = self.finished.recv_timeout(OUTPUT_GRACE);
        let bytes = self.bytes.lock().map(|bytes| bytes.clone()).unwrap_or_default();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

fn drain(pipe: Option<impl Read + Send + 'static>) -> Captured {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let (sender, finished) = mpsc::channel();
    let shared = Arc::clone(&bytes);
    thread::spawn(move || {
        if let Some(mut pipe) = pipe {
            let mut chunk = [0u8; 8192];
            while let Ok(read) = pipe.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                if let Ok(mut bytes) = shared.lock() {
                    bytes.extend_from_slice(&chunk[..read]);
                }
            }
        }
        let _ = sender.send(());
    });
    Captured { bytes, finished }
}

impl CliRunner for SystemRunner {
    fn run(&self, args: &[&str]) -> Result<CliOutput, CliError> {
        let mut command = Command::new(&self.program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = &self.config_dir {
            command.env(self.config_env, dir);
        }
        let mut child = command.spawn().map_err(|err| CliError::Spawn(err.to_string()))?;
        let stdout = drain(child.stdout.take());
        let stderr = drain(child.stderr.take());
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(err) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(CliError::Spawn(err.to_string()));
                }
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CliError::TimedOut(self.timeout));
            }
            thread::sleep(POLL);
        };
        Ok(CliOutput {
            code: status.code(),
            stdout: stdout.text(),
            stderr: stderr.text(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServer {
    pub scope: Option<String>,
    pub transport: Option<String>,
    pub command: Option<String>,
    pub args: String,
    pub env_entries: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpLookup {
    NotFound,
    Found(McpServer),
    Unknown(String),
}

#[derive(Debug, Error)]
pub enum McpError {
    #[error("claude mcp add failed: {0}")]
    AddFailed(String),
    #[error("claude mcp remove failed: {0}")]
    RemoveFailed(String),
    #[error("{0}")]
    Unverified(String),
    #[error(transparent)]
    Cli(#[from] CliError),
}

pub fn add_command(exe: &str) -> Vec<String> {
    [
        "mcp",
        "add",
        "--scope",
        "user",
        SERVER_NAME,
        "--",
        exe,
        SERVER_ARGS,
    ]
    .map(str::to_owned)
    .to_vec()
}

pub fn remove_command() -> Vec<String> {
    ["mcp", "remove", SERVER_NAME, "--scope", "user"]
        .map(str::to_owned)
        .to_vec()
}

pub fn mcp_hash(command: &str, args: &str) -> String {
    sha256_hex(format!("agentdust-mcp-v1\0{SERVER_NAME}\0{command}\0{args}").as_bytes())
}

fn first_line(output: &CliOutput) -> String {
    [&output.stderr, &output.stdout]
        .into_iter()
        .flat_map(|text| text.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map_or_else(
            || match output.code {
                Some(code) => format!("exit status {code}"),
                None => "the process was killed by a signal".to_owned(),
            },
            str::to_owned,
        )
}

pub fn parse_get(output: &CliOutput) -> McpLookup {
    match output.code {
        Some(0) => {}
        Some(1)
            if output.stderr.contains("No MCP server named")
                || output.stdout.contains("No MCP server named") =>
        {
            return McpLookup::NotFound;
        }
        _ => return McpLookup::Unknown(first_line(output)),
    }
    let mut server = McpServer {
        scope: None,
        transport: None,
        command: None,
        args: String::new(),
        env_entries: 0,
    };
    let mut in_environment = false;
    for line in output.stdout.lines() {
        let trimmed = line.trim_start();
        if in_environment {
            if line.len() - trimmed.len() >= 4 && !trimmed.is_empty() {
                server.env_entries += 1;
                continue;
            }
            in_environment = false;
        }
        if let Some(value) = trimmed.strip_prefix("Scope:") {
            server.scope = Some(value.trim().to_owned());
        } else if let Some(value) = trimmed.strip_prefix("Type:") {
            server.transport = Some(value.trim().to_owned());
        } else if let Some(value) = trimmed.strip_prefix("Command:") {
            server.command = Some(value.trim().to_owned());
        } else if let Some(value) = trimmed.strip_prefix("Args:") {
            server.args = value.trim().to_owned();
        } else if trimmed.starts_with("Environment:") {
            in_environment = true;
        }
    }
    if server.command.is_none() && server.transport.is_none() {
        return McpLookup::Unknown(
            "the server details could not be read from the claude CLI output".to_owned(),
        );
    }
    McpLookup::Found(server)
}

pub fn matches_desired(server: &McpServer, exe: &str, args: &str) -> bool {
    server
        .scope
        .as_deref()
        .is_none_or(|scope| scope.starts_with("User"))
        && server
            .transport
            .as_deref()
            .is_none_or(|transport| transport == "stdio")
        && server.command.as_deref() == Some(exe)
        && server.args == args
        && server.env_entries == 0
}

pub fn lookup(runner: &dyn CliRunner) -> McpLookup {
    match runner.run(&["mcp", "get", SERVER_NAME]) {
        Ok(output) => parse_get(&output),
        Err(err) => McpLookup::Unknown(err.to_string()),
    }
}

fn run_owned(runner: &dyn CliRunner, args: &[String]) -> Result<CliOutput, CliError> {
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    runner.run(&borrowed)
}

pub fn describe_server(server: &McpServer) -> String {
    format!(
        "command {} with arguments \"{}\" in {}",
        server.command.as_deref().unwrap_or("(none)"),
        server.args,
        server.scope.as_deref().unwrap_or("an unknown scope")
    )
}

pub fn register(runner: &dyn CliRunner, exe: &str) -> Result<(), McpError> {
    let output = run_owned(runner, &add_command(exe))?;
    if output.code != Some(0) {
        return Err(McpError::AddFailed(first_line(&output)));
    }
    match lookup(runner) {
        McpLookup::Found(server) if matches_desired(&server, exe, SERVER_ARGS) => Ok(()),
        McpLookup::Found(server) => Err(McpError::Unverified(format!(
            "the claude CLI reported success, but `claude mcp get {SERVER_NAME}` shows {}, not {exe} {SERVER_ARGS} in the user scope",
            describe_server(&server)
        ))),
        McpLookup::NotFound => Err(McpError::Unverified(format!(
            "the claude CLI reported success, but `claude mcp get {SERVER_NAME}` finds no such server"
        ))),
        McpLookup::Unknown(reason) => Err(McpError::Unverified(format!(
            "the claude CLI reported success, but the registration could not be read back: {reason}"
        ))),
    }
}

pub fn unregister_if_ours(runner: &dyn CliRunner, exe: &str) -> Result<(), McpError> {
    match lookup(runner) {
        McpLookup::NotFound => Ok(()),
        McpLookup::Unknown(reason) => Err(McpError::Unverified(format!(
            "the registration could not be read, so it was left alone: {reason}"
        ))),
        McpLookup::Found(server) if !matches_desired(&server, exe, SERVER_ARGS) => {
            Err(McpError::RemoveFailed(format!(
                "the registration is not the one setup created ({})",
                describe_server(&server)
            )))
        }
        McpLookup::Found(_) => {
            let output = run_owned(runner, &remove_command())?;
            if output.code != Some(0) {
                return Err(McpError::RemoveFailed(first_line(&output)));
            }
            match lookup(runner) {
                McpLookup::NotFound => Ok(()),
                _ => Err(McpError::Unverified(format!(
                    "the claude CLI reported success, but `claude mcp get {SERVER_NAME}` still finds the server"
                ))),
            }
        }
    }
}
