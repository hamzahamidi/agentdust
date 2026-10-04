use std::ffi::OsStr;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::parent_id;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use agentdust_core::darwin::DarwinProvider;
use agentdust_core::identity::ProcessIdentity;
use agentdust_core::provider::{ProcessProvider, ProcessRead};
use serde_json::{Value, json};

use super::{HANG_GUARD, scratch_dir};

const PLAN: &str = "AGENTDUST_TEST_CHAIN_PLAN";
const INDEX: &str = "AGENTDUST_TEST_CHAIN_INDEX";
const HOP_ARGS: [&str; 4] = ["relay_hop", "--exact", "--ignored", "--nocapture"];
const POLL: Duration = Duration::from_millis(5);

pub struct Chain {
    root: PathBuf,
    owned: bool,
    hops: Vec<Value>,
}

pub struct Outcome {
    pub status: Option<i64>,
    pub stdout: String,
    pub stderr: String,
    pub hops: Vec<ProcessIdentity>,
}

impl Outcome {
    pub fn assert_silent_success(&self) {
        assert_eq!(self.status, Some(0), "stderr: {}", self.stderr);
        assert_eq!(self.stdout, "");
        assert_eq!(self.stderr, "");
    }
}

impl Chain {
    pub fn new(label: &str) -> Self {
        let root = scratch_dir(label);
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&root)
            .unwrap();
        Self {
            root,
            owned: true,
            hops: Vec::new(),
        }
    }

    pub fn within(parent: &Path, label: &str) -> Self {
        let root = parent.join(label);
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&root)
            .unwrap();
        Self {
            root,
            owned: false,
            hops: Vec::new(),
        }
    }

    pub fn claude(self) -> Self {
        self.hop("claude", None, false)
    }

    pub fn relay(self) -> Self {
        self.hop("relay", None, false)
    }

    pub fn node(self, script: &str) -> Self {
        self.hop("node", Some(script), false)
    }

    pub fn orphan(self) -> Self {
        assert!(!self.hops.is_empty(), "the first hop has the test as its parent");
        self.hop("relay", None, true)
    }

    fn hop(mut self, name: &str, script: Option<&str>, orphaned: bool) -> Self {
        let dir = self.root.join(format!("hop{}", self.hops.len()));
        DirBuilder::new().mode(0o700).create(&dir).unwrap();
        let path = dir.join(name);
        fs::copy(std::env::current_exe().unwrap(), &path).unwrap();
        self.hops
            .push(json!({"path": path, "script": script, "orphaned": orphaned}));
        self
    }

    pub fn run(
        &self,
        data_dir: &Path,
        input: &[u8],
        env_file: Option<&Path>,
        envs: &[(&str, &OsStr)],
        hook_cwd: Option<&Path>,
    ) -> Outcome {
        let (input_path, report, done) = (
            self.root.join("input"),
            self.root.join("report"),
            self.root.join("done"),
        );
        let _ = fs::remove_file(&report);
        let _ = fs::remove_file(&done);
        fs::write(&input_path, input).unwrap();
        let envs: Vec<Value> = envs
            .iter()
            .map(|(name, value)| json!([name, value.to_string_lossy()]))
            .collect();
        let plan = json!({
            "hops": self.hops,
            "hook": env!("CARGO_BIN_EXE_agentdust"),
            "data_dir": data_dir,
            "input": input_path,
            "env_file": env_file,
            "report": report,
            "done": done,
            "envs": envs,
            "cwd": hook_cwd,
        });
        let mut first = spawn_hop(&plan, 0);
        let deadline = Instant::now() + HANG_GUARD;
        while !done.exists() {
            assert!(Instant::now() < deadline, "the chain did not finish");
            thread::sleep(POLL);
        }
        let _ = first.wait();
        let result: Value = serde_json::from_slice(&fs::read(&done).unwrap()).unwrap();
        assert!(result.get("error").is_none(), "{result}");
        let hops = fs::read_to_string(&report)
            .unwrap_or_default()
            .lines()
            .map(|line| {
                let value: Value = serde_json::from_str(line).unwrap();
                serde_json::from_value(value["identity"].clone()).unwrap()
            })
            .collect();
        Outcome {
            status: result["status"].as_i64(),
            stdout: result["stdout"].as_str().unwrap().to_owned(),
            stderr: result["stderr"].as_str().unwrap().to_owned(),
            hops,
        }
    }
}

impl Drop for Chain {
    fn drop(&mut self) {
        if self.owned {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn spawn_hop(plan: &Value, index: usize) -> Child {
    let hop = &plan["hops"][index];
    let mut command = Command::new(hop["path"].as_str().unwrap());
    if let Some(script) = hop["script"].as_str() {
        command.arg(script);
    }
    command
        .args(HOP_ARGS)
        .env(PLAN, plan.to_string())
        .env(INDEX, index.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

pub fn run_hop() {
    let Ok(plan) = std::env::var(PLAN) else {
        return;
    };
    let plan: Value = serde_json::from_str(&plan).unwrap();
    let index: usize = std::env::var(INDEX).unwrap().parse().unwrap();
    report_self(&plan, index);
    let hops = plan["hops"].as_array().unwrap();
    if hops[index]["orphaned"].as_bool().unwrap() && !wait_for_launchd() {
        finish(&plan, json!({"error": "the hop was never reparented"}));
        return;
    }
    match hops.get(index + 1) {
        Some(next) => {
            let mut child = spawn_hop(&plan, index + 1);
            if !next["orphaned"].as_bool().unwrap() {
                let _ = child.wait();
            }
        }
        None => run_hook(&plan),
    }
}

fn report_self(plan: &Value, index: usize) {
    let identity = match DarwinProvider::new().and_then(|provider| provider.read(std::process::id() as i32)) {
        Ok(ProcessRead::Present(identity)) => serde_json::to_value(identity).unwrap(),
        _ => Value::Null,
    };
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(plan["report"].as_str().unwrap())
        .unwrap();
    let line = format!("{}\n", json!({"index": index, "identity": identity}));
    file.write_all(line.as_bytes()).unwrap();
}

fn wait_for_launchd() -> bool {
    let deadline = Instant::now() + HANG_GUARD;
    while parent_id() != 1 {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(POLL);
    }
    true
}

fn run_hook(plan: &Value) {
    let mut command = Command::new(plan["hook"].as_str().unwrap());
    command
        .args(["hook", "claude"])
        .env_remove(PLAN)
        .env_remove(INDEX)
        .env("AGENTDUST_DATA_DIR", plan["data_dir"].as_str().unwrap())
        .stdin(File::open(plan["input"].as_str().unwrap()).unwrap());
    match plan["env_file"].as_str() {
        Some(path) => command.env("CLAUDE_ENV_FILE", path),
        None => command.env_remove("CLAUDE_ENV_FILE"),
    };
    for pair in plan["envs"].as_array().unwrap() {
        command.env(pair[0].as_str().unwrap(), pair[1].as_str().unwrap());
    }
    if let Some(cwd) = plan["cwd"].as_str() {
        command.current_dir(cwd);
    }
    let output = command.output().unwrap();
    finish(
        plan,
        json!({
            "status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }),
    );
}

fn finish(plan: &Value, result: Value) {
    let done = plan["done"].as_str().unwrap();
    let partial = format!("{done}.partial");
    fs::write(&partial, result.to_string()).unwrap();
    fs::rename(partial, done).unwrap();
}
