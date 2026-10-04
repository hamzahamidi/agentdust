#![allow(dead_code)]

pub mod specs;

use std::collections::BTreeSet;

use agentdust_core::class::Class;
use agentdust_core::classifier::{Evidence, Finding, Policy, Provenance, classify};
use agentdust_core::inventory::{Launchd, RawProcess, Snapshot};
use agentdust_core::journal::Agent;
use agentdust_core::session::{Liveness, Scope, State};

use crate::inventory_support::{Edit, MINUTE, NOW, SELF_PID, UID, kernel, key, raw};

pub fn snapshot(processes: Vec<RawProcess>) -> Snapshot {
    snapshot_with(processes, Launchd::Known(BTreeSet::new()))
}

pub fn snapshot_with(processes: Vec<RawProcess>, launchd: Launchd) -> Snapshot {
    Snapshot {
        taken_at_us: NOW,
        processes,
        launchd,
    }
}

pub fn launchd(pids: &[i32]) -> Launchd {
    Launchd::Known(pids.iter().copied().collect())
}

pub fn policy() -> Policy {
    Policy::new(UID, SELF_PID)
}

pub fn old(pid: i32) -> RawProcess {
    raw(pid).ppid(1).started(NOW - 31 * MINUTE)
}

pub fn the_tool() -> Vec<RawProcess> {
    vec![raw(SELF_PID).ppid(8000), raw(8000).ppid(7000), raw(7000).ppid(1)]
}

pub fn scope(agent_pid: Option<i32>, liveness: Option<Liveness>, session_ended: bool, keys: &[u8]) -> Scope {
    let state = if liveness == Some(Liveness::Gone) || session_ended {
        State::Ended
    } else {
        State::Active
    };
    Scope {
        agent: Agent::Claude,
        session_id: "s1".to_owned(),
        identity: agent_pid.map(kernel),
        exe_base: None,
        state,
        session_ended,
        liveness,
        tag_keys: keys.iter().map(|n| key(*n)).collect(),
        subagent_ids: BTreeSet::new(),
    }
}

pub fn gone(agent_pid: i32, keys: &[u8]) -> Scope {
    scope(Some(agent_pid), Some(Liveness::Gone), false, keys)
}

pub fn alive(agent_pid: i32, keys: &[u8]) -> Scope {
    scope(Some(agent_pid), Some(Liveness::Alive), false, keys)
}

pub fn unverified(agent_pid: i32, keys: &[u8]) -> Scope {
    scope(Some(agent_pid), Some(Liveness::Unknown), false, keys)
}

pub fn degraded(keys: &[u8]) -> Scope {
    scope(None, None, false, keys)
}

pub fn run(processes: Vec<RawProcess>, scopes: &[Scope]) -> Vec<Finding> {
    classify(&snapshot(processes), &Provenance::Available(scopes), &policy())
}

pub fn find(findings: &[Finding], pid: i32) -> &Finding {
    findings
        .iter()
        .find(|finding| finding.identity.kernel.pid == pid)
        .unwrap_or_else(|| panic!("no finding for pid {pid}"))
}

pub fn class_of(findings: &[Finding], pid: i32) -> Class {
    find(findings, pid).class
}

pub fn evidence_of(findings: &[Finding], pid: i32) -> Vec<Evidence> {
    find(findings, pid).evidence.clone()
}
