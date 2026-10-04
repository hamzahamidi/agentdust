#![allow(dead_code)]

use std::collections::HashMap;

use agentdust_core::identity::KernelIdentity;
use agentdust_core::journal::{Agent, AgentIdentity, ExeBase, Kind, Record, SCHEMA_VERSION, SessionTagKey};
use agentdust_core::session::{Liveness, LivenessProbe, Scope, scopes};

pub const BOOT: &str = "b1";
pub const START: u64 = 1_800_000_000_000_000;

pub fn kernel(pid: i32) -> KernelIdentity {
    kernel_on(BOOT, pid)
}

pub fn kernel_on(boot: &str, pid: i32) -> KernelIdentity {
    KernelIdentity {
        boot_session_uuid: boot.to_owned(),
        pid,
        start_time_us: START + pid as u64,
        uid: 501,
    }
}

pub fn identity(pid: i32) -> AgentIdentity {
    AgentIdentity::new(
        pid,
        START + pid as u64,
        501,
        Some(ExeBase::try_from("claude").unwrap()),
    )
    .unwrap()
}

pub fn key(n: u8) -> SessionTagKey {
    SessionTagKey::try_from(format!("{n:02x}").repeat(32)).unwrap()
}

pub fn rec(kind: Kind) -> Record {
    Record {
        v: SCHEMA_VERSION,
        kind,
        agent: Agent::Claude,
        session_id: "s1".to_owned(),
        subagent_id: None,
        agent_identity: None,
        tool_use_id: None,
        wall_ts: 1,
        mono_ts: 1,
        boot: BOOT.to_owned(),
        session_tag_key: None,
        cwd_key: None,
        exe_base: None,
    }
}

pub trait Edit: Sized {
    fn session(self, id: &str) -> Self;
    fn agent(self, agent: Agent) -> Self;
    fn by(self, pid: i32) -> Self;
    fn on_boot(self, boot: &str) -> Self;
    fn sub(self, id: &str) -> Self;
    fn tagged(self, n: u8) -> Self;
}

impl Edit for Record {
    fn session(mut self, id: &str) -> Self {
        self.session_id = id.to_owned();
        self
    }

    fn agent(mut self, agent: Agent) -> Self {
        self.agent = agent;
        self
    }

    fn by(mut self, pid: i32) -> Self {
        self.agent_identity = Some(identity(pid));
        self
    }

    fn on_boot(mut self, boot: &str) -> Self {
        self.boot = boot.to_owned();
        self
    }

    fn sub(mut self, id: &str) -> Self {
        self.subagent_id = Some(id.to_owned());
        self
    }

    fn tagged(mut self, n: u8) -> Self {
        self.session_tag_key = Some(key(n));
        self
    }
}

#[derive(Default)]
pub struct Table(HashMap<i32, Liveness>);

impl Table {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn alive(mut self, pid: i32) -> Self {
        self.0.insert(pid, Liveness::Alive);
        self
    }

    pub fn gone(mut self, pid: i32) -> Self {
        self.0.insert(pid, Liveness::Gone);
        self
    }
}

impl LivenessProbe for Table {
    fn probe(&self, identity: &KernelIdentity) -> Liveness {
        self.0.get(&identity.pid).copied().unwrap_or(Liveness::Unknown)
    }
}

pub fn run(records: &[Record], probe: &impl LivenessProbe) -> Vec<Scope> {
    scopes(records, probe)
}
