#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::io;
use std::rc::Rc;
use std::time::Duration;

use agentdust_core::identity::KernelIdentity;
use agentdust_core::inventory::{Clock, Cpu, LaunchdSource, ProcessSource, RawIdentity, RawProcess, Tag};
use agentdust_core::journal::SessionTagKey;

pub const BOOT: &str = "b1";
pub const START: u64 = 1_800_000_000_000_000;
pub const SECOND: u64 = 1_000_000;
pub const MINUTE: u64 = 60 * SECOND;
pub const HOUR: u64 = 60 * MINUTE;
pub const UID: u32 = 501;
pub const SELF_PID: i32 = 9000;
pub const NOW: u64 = START + 48 * HOUR;

pub fn kernel(pid: i32) -> KernelIdentity {
    KernelIdentity {
        boot_session_uuid: BOOT.to_owned(),
        pid,
        start_time_us: START + pid as u64,
        uid: UID,
    }
}

pub fn key(n: u8) -> SessionTagKey {
    SessionTagKey::try_from(format!("{n:02x}").repeat(32)).unwrap()
}

pub fn raw(pid: i32) -> RawProcess {
    RawProcess {
        identity: RawIdentity {
            kernel: kernel(pid),
            exe_path: Some("/opt/homebrew/bin/node".into()),
            ppid: 100,
            pgid: pid,
        },
        tag: Tag::Absent,
        agent_script: false,
        cpu: Cpu {
            first_ns: Some(1_000),
            later_ns: Some(1_000),
        },
    }
}

pub trait Edit: Sized {
    fn uid(self, uid: u32) -> Self;
    fn ppid(self, ppid: i32) -> Self;
    fn exe(self, path: &str) -> Self;
    fn no_path(self) -> Self;
    fn started(self, start_time_us: u64) -> Self;
    fn tagged(self, n: u8) -> Self;
    fn tag(self, tag: Tag) -> Self;
    fn busy(self) -> Self;
    fn cpu(self, first: Option<u64>, later: Option<u64>) -> Self;
    fn script_agent(self) -> Self;
}

impl Edit for RawProcess {
    fn uid(mut self, uid: u32) -> Self {
        self.identity.kernel.uid = uid;
        self
    }

    fn ppid(mut self, ppid: i32) -> Self {
        self.identity.ppid = ppid;
        self
    }

    fn exe(mut self, path: &str) -> Self {
        self.identity.exe_path = Some(path.into());
        self
    }

    fn no_path(mut self) -> Self {
        self.identity.exe_path = None;
        self
    }

    fn started(mut self, start_time_us: u64) -> Self {
        self.identity.kernel.start_time_us = start_time_us;
        self
    }

    fn tagged(mut self, n: u8) -> Self {
        self.tag = Tag::Keyed(key(n));
        self
    }

    fn tag(mut self, tag: Tag) -> Self {
        self.tag = tag;
        self
    }

    fn busy(mut self) -> Self {
        self.cpu = Cpu {
            first_ns: Some(1_000),
            later_ns: Some(2_000),
        };
        self
    }

    fn cpu(mut self, first: Option<u64>, later: Option<u64>) -> Self {
        self.cpu = Cpu {
            first_ns: first,
            later_ns: later,
        };
        self
    }

    fn script_agent(mut self) -> Self {
        self.agent_script = true;
        self
    }
}

pub type Log = Rc<RefCell<Vec<String>>>;

pub fn log() -> Log {
    Rc::default()
}

#[derive(Clone, Copy)]
pub enum Later {
    Same,
    Value(u64),
    Gone,
    Fails,
}

pub struct ScriptedSource {
    processes: Vec<RawProcess>,
    later: HashMap<i32, Later>,
    fail_scan: bool,
    log: Log,
}

impl ScriptedSource {
    pub fn new(log: &Log, processes: Vec<RawProcess>) -> Self {
        Self {
            processes,
            later: HashMap::new(),
            fail_scan: false,
            log: Rc::clone(log),
        }
    }

    pub fn later(mut self, pid: i32, later: Later) -> Self {
        self.later.insert(pid, later);
        self
    }

    pub fn failing(mut self) -> Self {
        self.fail_scan = true;
        self
    }
}

impl ProcessSource for ScriptedSource {
    fn scan(&self) -> io::Result<Vec<RawProcess>> {
        self.log.borrow_mut().push("scan".to_owned());
        if self.fail_scan {
            return Err(io::Error::other("scan failed"));
        }
        Ok(self
            .processes
            .iter()
            .cloned()
            .map(|mut process| {
                process.cpu.later_ns = None;
                process
            })
            .collect())
    }

    fn cpu_time_ns(&self, identity: &KernelIdentity) -> io::Result<Option<u64>> {
        self.log.borrow_mut().push(format!("cpu {}", identity.pid));
        let first = self
            .processes
            .iter()
            .find(|process| process.identity.kernel == *identity)
            .and_then(|process| process.cpu.first_ns);
        match self.later.get(&identity.pid).copied().unwrap_or(Later::Same) {
            Later::Same => Ok(first),
            Later::Value(value) => Ok(Some(value)),
            Later::Gone => Ok(None),
            Later::Fails => Err(io::Error::other("cpu read failed")),
        }
    }
}

pub struct ScriptedLaunchd {
    pids: Option<BTreeSet<i32>>,
    log: Log,
}

impl ScriptedLaunchd {
    pub fn new(log: &Log, pids: &[i32]) -> Self {
        Self {
            pids: Some(pids.iter().copied().collect()),
            log: Rc::clone(log),
        }
    }

    pub fn failing(log: &Log) -> Self {
        Self {
            pids: None,
            log: Rc::clone(log),
        }
    }
}

impl LaunchdSource for ScriptedLaunchd {
    fn pids(&self) -> io::Result<BTreeSet<i32>> {
        self.log.borrow_mut().push("launchd".to_owned());
        self.pids
            .clone()
            .ok_or_else(|| io::Error::other("launchctl failed"))
    }
}

pub struct ScriptedClock {
    now_us: Cell<u64>,
    log: Log,
}

impl ScriptedClock {
    pub fn new(log: &Log, now_us: u64) -> Self {
        Self {
            now_us: Cell::new(now_us),
            log: Rc::clone(log),
        }
    }
}

impl Clock for ScriptedClock {
    fn now_us(&self) -> u64 {
        self.log.borrow_mut().push("now".to_owned());
        self.now_us.get()
    }

    fn sleep(&self, duration: Duration) {
        self.log
            .borrow_mut()
            .push(format!("sleep {}ms", duration.as_millis()));
        self.now_us.set(self.now_us.get() + duration.as_micros() as u64);
    }
}
