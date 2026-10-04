#![cfg(target_os = "macos")]

use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use agentdust_core::ancestry::{AncestryProvider, Anchor, find_agent};
use agentdust_core::darwin::DarwinProvider;
use agentdust_core::provider::{ProcessProvider, ProcessRead};
use agentdust_testkit::{Fixture, wait_until};
use std::time::Duration;

const SLEEPER: &str = env!("CARGO_BIN_EXE_fixture-sleeper");
const READY: Duration = Duration::from_secs(60);

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "agentdust-ancestry-{name}-{}-{}",
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

    fn copy_as(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::copy(SLEEPER, &path).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn identity_of(provider: &DarwinProvider, pid: i32) -> agentdust_core::identity::ProcessIdentity {
    match provider.read(pid).unwrap() {
        ProcessRead::Present(identity) => identity,
        other => panic!("{other:?}"),
    }
}

fn ready(path: &Path) {
    assert!(wait_until(|| path.exists(), READY), "the fixture did not report");
}

#[test]
fn the_parent_of_a_live_fixture_is_this_test_process() {
    let provider = DarwinProvider::new().unwrap();
    let fixture = Fixture(Command::new(SLEEPER).arg("30").spawn().unwrap());
    let identity = identity_of(&provider, fixture.0.id() as i32);
    assert_eq!(
        provider.parent(&identity.kernel).unwrap(),
        Some(std::process::id() as i32)
    );
}

#[test]
fn a_parent_link_needs_every_field_of_the_identity_to_match() {
    let provider = DarwinProvider::new().unwrap();
    let fixture = Fixture(Command::new(SLEEPER).arg("30").spawn().unwrap());
    let kernel = identity_of(&provider, fixture.0.id() as i32).kernel;
    assert!(provider.parent(&kernel).unwrap().is_some());
    let mut later = kernel.clone();
    later.start_time_us += 1;
    let mut stranger = kernel.clone();
    stranger.uid += 1;
    let mut rebooted = kernel.clone();
    rebooted.boot_session_uuid = "00000000-0000-0000-0000-000000000000".to_owned();
    for changed in [later, stranger, rebooted] {
        assert_eq!(provider.parent(&changed).unwrap(), None, "{changed:?}");
    }
}

#[test]
fn a_process_that_exited_has_no_parent_link() {
    let provider = DarwinProvider::new().unwrap();
    let mut fixture = Fixture(Command::new(SLEEPER).arg("30").spawn().unwrap());
    let kernel = identity_of(&provider, fixture.0.id() as i32).kernel;
    fixture.0.kill().unwrap();
    fixture.0.wait().unwrap();
    assert_eq!(provider.parent(&kernel).unwrap(), None);
}

#[test]
fn the_script_argument_of_a_live_process_is_its_first_argument() {
    let scratch = Scratch::new("script");
    let node = scratch.copy_as("node");
    let provider = DarwinProvider::new().unwrap();
    let fixture = Fixture(Command::new(&node).arg("30").spawn().unwrap());
    let pid = fixture.0.id() as i32;
    assert_eq!(provider.script_argument(pid).unwrap(), Some(b"30".to_vec()));
}

#[test]
fn the_script_argument_of_a_process_that_is_gone_is_none() {
    let provider = DarwinProvider::new().unwrap();
    let mut fixture = Fixture(Command::new(SLEEPER).arg("30").spawn().unwrap());
    let pid = fixture.0.id() as i32;
    fixture.0.kill().unwrap();
    fixture.0.wait().unwrap();
    assert_eq!(provider.script_argument(pid).unwrap(), None);
}

#[test]
fn a_live_process_named_claude_is_found_from_its_own_pid() {
    let scratch = Scratch::new("named");
    let claude = scratch.copy_as("claude");
    let provider = DarwinProvider::new().unwrap();
    let fixture = Fixture(Command::new(&claude).arg("30").spawn().unwrap());
    let pid = fixture.0.id() as i32;
    let found = find_agent(&provider, pid).unwrap();
    assert_eq!(found.via, Anchor::Executable);
    assert_eq!(found.identity, identity_of(&provider, pid));
    assert!(found.identity.evidence.exe_path.ends_with("claude"));
}

#[test]
fn a_live_node_whose_argument_path_mentions_claude_is_found() {
    let scratch = Scratch::new("node-claude");
    let node = scratch.copy_as("node");
    let report = scratch.0.join("claude-code").join("ready");
    fs::create_dir(report.parent().unwrap()).unwrap();
    let provider = DarwinProvider::new().unwrap();
    let fixture = Fixture(
        Command::new(&node)
            .arg("--report-file")
            .arg(&report)
            .spawn()
            .unwrap(),
    );
    ready(&report);
    let pid = fixture.0.id() as i32;
    let found = find_agent(&provider, pid).unwrap();
    assert_eq!(found.via, Anchor::NodeScript);
    assert_eq!(found.identity, identity_of(&provider, pid));
}

#[test]
fn a_live_node_whose_argument_path_does_not_mention_claude_is_not_the_agent_itself() {
    let scratch = Scratch::new("node-other");
    let node = scratch.copy_as("node");
    let report = scratch.0.join("other-tool").join("ready");
    fs::create_dir(report.parent().unwrap()).unwrap();
    let provider = DarwinProvider::new().unwrap();
    let fixture = Fixture(
        Command::new(&node)
            .arg("--report-file")
            .arg(&report)
            .spawn()
            .unwrap(),
    );
    ready(&report);
    let pid = fixture.0.id() as i32;
    let found = find_agent(&provider, pid);
    assert!(found.is_none_or(|found| found.identity.kernel.pid != pid));
}
