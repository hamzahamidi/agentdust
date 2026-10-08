use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use agentdust_core::darwin::DarwinProvider;
use agentdust_core::identity::ProcessIdentity;
use agentdust_core::provider::{ProcessProvider, ProcessRead};
use agentdust_core::revalidate::{Revalidation, revalidate};

use super::HANG_GUARD;

const SECONDS: &str = "AGENTDUST_TEST_SLEEP_SECONDS";
const HOP_ARGS: [&str; 4] = ["sleeper_hop", "--exact", "--ignored", "--nocapture"];
const LIFE: &str = "120";

pub struct Sleeper {
    child: Child,
    pub identity: ProcessIdentity,
}

impl Sleeper {
    pub fn pid(&self) -> i32 {
        self.identity.kernel.pid
    }

    pub fn alive(&self) -> bool {
        let provider = DarwinProvider::new().unwrap();
        revalidate(&self.identity, &provider) == Revalidation::Match
    }

    pub fn end(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Sleeper {
    fn drop(&mut self) {
        self.end();
    }
}

#[allow(clippy::zombie_processes)]
pub fn spawn(dir: &Path, name: &str, cwd: &Path, envs: &[(&str, &str)], markers: &[&str]) -> Sleeper {
    DirBuilder::new().recursive(true).mode(0o700).create(dir).unwrap();
    DirBuilder::new().recursive(true).mode(0o700).create(cwd).unwrap();
    let exe = dir.join(name);
    fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
    let mut child = Command::new(&exe)
        .args(HOP_ARGS)
        .args(markers)
        .env(SECONDS, LIFE)
        .env_remove("AGENTDUST_SESSION")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .envs(envs.iter().copied())
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id() as i32;
    let provider = DarwinProvider::new().unwrap();
    let deadline = Instant::now() + HANG_GUARD;
    loop {
        if let Ok(ProcessRead::Present(identity)) = provider.read(pid) {
            return Sleeper { child, identity };
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the sleeper never became readable");
        }
        thread::sleep(Duration::from_millis(5));
    }
}

pub fn run_sleeper_hop() {
    if let Ok(seconds) = std::env::var(SECONDS) {
        thread::sleep(Duration::from_secs(seconds.parse().unwrap_or(0)));
    }
}
