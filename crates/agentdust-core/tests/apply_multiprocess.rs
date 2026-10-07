mod apply_support;
mod scratch;

use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use agentdust_core::apply::exec::{Deps, Executor, Outcome, Reason, Settings, Verdict};
use agentdust_core::apply::signal::{SignalResult, Signaller};
use agentdust_core::apply::timer::{SystemTimer, Timer};
use agentdust_core::class::Class;
use agentdust_core::classifier::Finding;
use agentdust_core::identity::ProcessIdentity;
use agentdust_core::plan::PlanItem;
use agentdust_core::provider::{ProcessProvider, ProcessRead};
use agentdust_core::survey::Surveyor;
use apply_support::{PLAN, describe, finding, item};
use scratch::TempDir;

const READY_ENV: &str = "AGENTDUST_M6_APPLY_READY";
const RELEASE_ENV: &str = "AGENTDUST_M6_APPLY_RELEASE";
const DATA_ENV: &str = "AGENTDUST_M6_APPLY_DATA";
const SIGNALS_ENV: &str = "AGENTDUST_M6_APPLY_SIGNALS";
const SCENARIO_ENV: &str = "AGENTDUST_M6_APPLY_SCENARIO";

struct Survey {
    finding: Finding,
    gate: Option<(PathBuf, PathBuf)>,
}

impl Surveyor for Survey {
    fn survey(&self) -> io::Result<Vec<Finding>> {
        if let Some((ready, release)) = &self.gate {
            fs::write(ready, b"ready")?;
            let deadline = Instant::now() + Duration::from_secs(10);
            while !release.exists() {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "apply race timed out"));
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
        Ok(vec![self.finding.clone()])
    }

    fn describe(&self, finding: &Finding) -> agentdust_core::finding::ModelFinding {
        describe(finding)
    }
}

struct ProcessReads {
    identity: ProcessIdentity,
    calls: AtomicUsize,
}

impl ProcessProvider for ProcessReads {
    fn read(&self, _pid: i32) -> io::Result<ProcessRead> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(ProcessRead::Present(self.identity.clone()))
        } else {
            Ok(ProcessRead::Gone)
        }
    }
}

struct FileSignaller(PathBuf);

impl Signaller for FileSignaller {
    fn sigterm(&self, pid: i32) -> SignalResult {
        use std::io::Write;

        writeln!(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.0)
                .unwrap(),
            "{pid}"
        )
        .unwrap();
        SignalResult::Delivered
    }
}

fn executor(
    data_dir: &Path,
    planned: &PlanItem,
    finding: Finding,
    gate: Option<(PathBuf, PathBuf)>,
    signals: &Path,
) -> Executor {
    Executor::new(
        Deps {
            data_dir: data_dir.to_path_buf(),
            surveyor: Arc::new(Survey { finding, gate }),
            provider: Box::new(ProcessReads {
                identity: planned.identity.clone(),
                calls: AtomicUsize::new(0),
            }),
            signaller: Box::new(FileSignaller(signals.to_path_buf())),
            timer: Arc::new(SystemTimer::new()) as Arc<dyn Timer>,
        },
        Settings::default(),
    )
}

fn fresh_finding(scenario: &str) -> Finding {
    let mut fresh = finding(4242, Class::OwnedEnded);
    match scenario {
        "valid" => {}
        "changed_owner" => {
            fresh.attribution_owners.as_mut().unwrap()[0]
                .identity
                .start_time_us += 1;
        }
        other => panic!("unknown apply race scenario: {other}"),
    }
    fresh
}

struct ChildProcess {
    child: Option<Child>,
    release: PathBuf,
}

impl ChildProcess {
    fn wait_with_output(&mut self) -> Output {
        self.child.take().unwrap().wait_with_output().unwrap()
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        if self
            .child
            .as_mut()
            .is_some_and(|child| child.try_wait().ok().flatten().is_none())
        {
            let _ = fs::write(&self.release, b"release");
            if let Some(child) = self.child.as_mut() {
                let _ = child.wait();
            }
        }
    }
}

fn start_child(dir: &TempDir, scenario: &str) -> (ChildProcess, PathBuf, PathBuf) {
    let ready = dir.join("ready");
    let release = dir.join("release");
    let signals = dir.join("signals");
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "child_process_executes_approved_apply_until_released",
            "--nocapture",
        ])
        .env(DATA_ENV, dir.path())
        .env(READY_ENV, &ready)
        .env(RELEASE_ENV, &release)
        .env(SIGNALS_ENV, &signals)
        .env(SCENARIO_ENV, scenario)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    (
        ChildProcess {
            child: Some(child),
            release,
        },
        ready,
        signals,
    )
}

fn wait_for_ready(child: &mut ChildProcess, ready: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready.exists() {
        if let Some(status) = child.child.as_mut().unwrap().try_wait().unwrap() {
            let output = child.wait_with_output();
            panic!(
                "the apply process exited with {status}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert!(
            Instant::now() < deadline,
            "the apply process did not reach survey"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn child_process_executes_approved_apply_until_released() {
    let (Some(data_dir), Some(ready), Some(release), Some(signals), Some(scenario)) = (
        std::env::var_os(DATA_ENV),
        std::env::var_os(READY_ENV),
        std::env::var_os(RELEASE_ENV),
        std::env::var_os(SIGNALS_ENV),
        std::env::var_os(SCENARIO_ENV),
    ) else {
        return;
    };
    let data_dir = PathBuf::from(data_dir);
    let ready = PathBuf::from(ready);
    let release = PathBuf::from(release);
    let signals = PathBuf::from(signals);
    let scenario = scenario.to_string_lossy();
    let planned = item(4242, Class::OwnedEnded);
    let fresh = fresh_finding(&scenario);
    let executor = executor(&data_dir, &planned, fresh, Some((ready, release)), &signals);
    let result = executor.execute(PLAN, &planned);
    assert_eq!(
        result,
        if scenario == "changed_owner" {
            Verdict::failed(Reason::OwnershipChanged)
        } else {
            Verdict::of(Outcome::Terminated)
        }
    );
}

fn competing_apply_is_serialized(scenario: &str) {
    let dir = TempDir::private("apply-process-race");
    let planned = item(4242, Class::OwnedEnded);
    let (mut child, ready, signals) = start_child(&dir, scenario);
    wait_for_ready(&mut child, &ready);

    let fresh = fresh_finding(scenario);
    let parent = executor(&dir, &planned, fresh, None, &signals);
    assert_eq!(
        parent.execute(PLAN, &planned),
        Verdict::of(Outcome::HandledElsewhere)
    );

    fs::write(&child.release, b"release").unwrap();
    let output = child.wait_with_output();
    assert!(
        output.status.success(),
        "the apply process failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    if scenario == "changed_owner" {
        assert_eq!(
            parent.execute(PLAN, &planned),
            Verdict::failed(Reason::OwnershipChanged)
        );
        assert!(!signals.exists());
    } else {
        assert_eq!(fs::read_to_string(signals).unwrap(), "4242\n");
    }
}

#[test]
fn separate_apply_processes_signal_a_valid_identity_once() {
    competing_apply_is_serialized("valid");
}

#[test]
fn separate_apply_processes_refuse_a_changed_owner_after_approval() {
    competing_apply_is_serialized("changed_owner");
}
