#![allow(dead_code)]

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

use agentdust_core::identity::{IdentityEvidence, KernelIdentity, ProcessIdentity};
use agentdust_core::provider::{ProcessProvider, ProcessRead};

pub const BOOT: &str = "11111111-2222-3333-4444-555555555555";

pub fn kernel(pid: i32) -> KernelIdentity {
    KernelIdentity {
        boot_session_uuid: BOOT.to_owned(),
        pid,
        start_time_us: 1_800_000_000_000_000,
        uid: 501,
    }
}

pub fn identity(pid: i32) -> ProcessIdentity {
    ProcessIdentity {
        kernel: kernel(pid),
        evidence: IdentityEvidence {
            exe_path: PathBuf::from("/usr/local/bin/node"),
        },
    }
}

pub fn changed(base: &ProcessIdentity, edit: impl FnOnce(&mut ProcessIdentity)) -> ProcessIdentity {
    let mut copy = base.clone();
    edit(&mut copy);
    copy
}

enum Step {
    Read(ProcessRead),
    Fail(io::ErrorKind),
}

#[derive(Default)]
struct Script {
    steps: Vec<Step>,
    next: usize,
    reads: usize,
}

#[derive(Default)]
pub struct ScriptedProvider {
    scripts: Mutex<HashMap<i32, Script>>,
}

impl ScriptedProvider {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn then(self, pid: i32, read: ProcessRead) -> Self {
        self.push(pid, Step::Read(read))
    }

    pub fn then_fail(self, pid: i32, kind: io::ErrorKind) -> Self {
        self.push(pid, Step::Fail(kind))
    }

    pub fn reads(&self, pid: i32) -> usize {
        self.scripts
            .lock()
            .unwrap()
            .get(&pid)
            .map_or(0, |script| script.reads)
    }

    fn push(self, pid: i32, step: Step) -> Self {
        self.scripts
            .lock()
            .unwrap()
            .entry(pid)
            .or_default()
            .steps
            .push(step);
        self
    }
}

impl ProcessProvider for ScriptedProvider {
    fn read(&self, pid: i32) -> io::Result<ProcessRead> {
        let mut scripts = self.scripts.lock().unwrap();
        let script = scripts.entry(pid).or_default();
        script.reads += 1;
        let Some(last) = script.steps.len().checked_sub(1) else {
            return Ok(ProcessRead::Gone);
        };
        let step = &script.steps[script.next.min(last)];
        script.next += 1;
        match step {
            Step::Read(read) => Ok(read.clone()),
            Step::Fail(kind) => Err(io::Error::from(*kind)),
        }
    }
}
