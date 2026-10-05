#![cfg(target_os = "macos")]
#![allow(dead_code)]

use std::collections::HashMap;
use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentdust_core::apply::exec::{Outcome, Reason};
use agentdust_core::apply::server::{Call, Challenge, Report, Response};
use agentdust_core::apply::signal::{SignalResult, Signaller};
use agentdust_core::finding::ModelFinding;
use agentdust_testkit::harness::{Error as HarnessError, Harness, ProcHandle, Signal};

pub const WAIT: Duration = Duration::from_secs(30);

pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "agentdust-apply-live-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
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

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub struct Gate {
    pub entered: Mutex<mpsc::Sender<()>>,
    pub release: Mutex<mpsc::Receiver<()>>,
}

pub struct Guarded {
    pub harness: Arc<Mutex<Harness>>,
    pub handles: HashMap<i32, ProcHandle>,
    pub gate: Option<Gate>,
}

impl Signaller for Guarded {
    fn sigterm(&self, pid: i32) -> SignalResult {
        if let Some(gate) = &self.gate {
            gate.entered.lock().unwrap().send(()).unwrap();
            gate.release.lock().unwrap().recv().unwrap();
        }
        let Some(handle) = self.handles.get(&pid) else {
            return SignalResult::Refused;
        };
        match self.harness.lock().unwrap().signal(*handle, Signal::Term) {
            Ok(()) => SignalResult::Delivered,
            Err(HarnessError::NotRunning(_)) => SignalResult::NoSuchProcess,
            Err(_) => SignalResult::Failed(0),
        }
    }
}

pub fn code_of(challenge: &Challenge) -> String {
    let at = challenge.message.find("Type ").unwrap();
    challenge.message[at + 5..at + 9].to_owned()
}

pub fn accept(challenge: &Challenge) -> Response {
    Response::Accept(Some(code_of(challenge)))
}

pub fn item_for(created: &agentdust_core::plan::Created, pid: i32) -> ModelFinding {
    created
        .items
        .iter()
        .find(|model| model.pid == pid)
        .unwrap_or_else(|| panic!("pid {pid} is not in the plan"))
        .clone()
}

pub fn call_for(created: &agentdust_core::plan::Created, pids: &[i32]) -> Call {
    Call {
        plan_id: created.plan_id.clone(),
        item_ids: pids.iter().map(|pid| item_for(created, *pid).item_id).collect(),
    }
}

pub fn results(report: &Report) -> Vec<(Outcome, Option<Reason>)> {
    report
        .items
        .iter()
        .map(|item| (item.result, item.reason))
        .collect()
}
