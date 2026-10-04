#![allow(dead_code)]

use std::collections::BTreeSet;

use agentdust_core::class::Class;
use agentdust_core::classifier::{Finding, Policy, Provenance, SYSTEM_PREFIXES, classify};
use agentdust_core::inventory::{Cpu, Launchd, RawProcess, Tag};
use agentdust_core::session::Scope;
use proptest::prelude::*;
use proptest::sample::select;

use super::{alive, degraded, gone, snapshot_with, unverified};
use crate::inventory_support::{Edit, MINUTE, NOW, SELF_PID, UID, key, raw};

pub const FIRST_FREE: i32 = 10_000;
pub const NATIVE_AGENT: &str = "/Users/dev/.local/share/claude/versions/2.1.289";
pub const FIXED_PROTECTED: [i32; 4] = [1, SELF_PID, 8000, 7000];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Free,
    OtherUser,
    System,
    InLaunchd,
    AgentExe,
    AgentScript,
}

#[derive(Debug, Clone)]
pub struct Spec {
    pub role: Role,
    pub tag: u8,
    pub ppid_one: bool,
    pub age_minutes: u64,
    pub cpu: u8,
    pub has_path: bool,
    pub prefix: usize,
}

pub fn arb_spec() -> impl Strategy<Value = Spec> {
    (
        select(vec![
            Role::Free,
            Role::Free,
            Role::Free,
            Role::Free,
            Role::OtherUser,
            Role::System,
            Role::InLaunchd,
            Role::AgentExe,
            Role::AgentScript,
        ]),
        0..9u8,
        any::<bool>(),
        select(vec![0u64, 29, 30, 31, 600]),
        0..3u8,
        proptest::bool::weighted(0.9),
        0..SYSTEM_PREFIXES.len(),
    )
        .prop_map(|(role, tag, ppid_one, age_minutes, cpu, has_path, prefix)| Spec {
            role,
            tag,
            ppid_one,
            age_minutes,
            cpu,
            has_path,
            prefix,
        })
}

pub fn tag_of(n: u8) -> Tag {
    match n {
        0 => Tag::Absent,
        5 => Tag::Unkeyed,
        6 => Tag::Unreadable,
        n => Tag::Keyed(key(n)),
    }
}

pub fn cpu_of(n: u8) -> Cpu {
    match n {
        0 => Cpu {
            first_ns: Some(5),
            later_ns: Some(5),
        },
        1 => Cpu {
            first_ns: Some(5),
            later_ns: Some(9),
        },
        _ => Cpu {
            first_ns: None,
            later_ns: None,
        },
    }
}

pub fn scopes() -> Vec<Scope> {
    vec![
        gone(7001, &[1]),
        alive(7002, &[2]),
        degraded(&[3]),
        unverified(7003, &[4]),
        gone(7006, &[7]),
        alive(7007, &[7]),
    ]
}

fn bait(pid: i32, ppid: i32) -> RawProcess {
    raw(pid).ppid(ppid).started(NOW - 600 * MINUTE).tagged(1)
}

pub fn pid_of(index: usize) -> i32 {
    FIRST_FREE + index as i32
}

pub fn build(specs: &[Spec]) -> (Vec<RawProcess>, BTreeSet<i32>) {
    let mut processes = vec![
        bait(1, 0).exe("/opt/x/init"),
        bait(SELF_PID, 8000),
        bait(8000, 7000),
        bait(7000, 1),
    ];
    let mut in_launchd = BTreeSet::new();
    for (index, spec) in specs.iter().enumerate() {
        let pid = pid_of(index);
        let mut process = raw(pid)
            .started(NOW - spec.age_minutes * MINUTE)
            .ppid(if spec.ppid_one { 1 } else { 100 })
            .tag(tag_of(spec.tag));
        process.cpu = cpu_of(spec.cpu);
        process.identity.exe_path = spec.has_path.then(|| "/opt/homebrew/bin/node".into());
        match spec.role {
            Role::OtherUser => process = process.uid(UID + 1),
            Role::System => {
                process = process.exe(&format!("{}/lib/tool", SYSTEM_PREFIXES[spec.prefix]));
            }
            Role::InLaunchd => {
                in_launchd.insert(pid);
            }
            Role::AgentExe => process = process.exe(NATIVE_AGENT),
            Role::AgentScript => process = process.script_agent(),
            Role::Free => {}
        }
        processes.push(process);
    }
    (processes, in_launchd)
}

pub fn managed(spec: &Spec, launchd_known: bool) -> bool {
    match spec.role {
        Role::Free => false,
        Role::InLaunchd => launchd_known,
        _ => true,
    }
}

pub fn expected(spec: &Spec, provenance: bool, launchd_known: bool) -> Class {
    if managed(spec, launchd_known) {
        return Class::Managed;
    }
    let tagged = match spec.tag {
        0 => None,
        1 if provenance => Some(Class::OwnedEnded),
        2 | 4 | 7 if provenance => Some(Class::OwnedLive),
        _ => Some(Class::Unknown),
    };
    match tagged {
        Some(Class::OwnedEnded) if launchd_known && spec.has_path => Class::OwnedEnded,
        Some(Class::OwnedEnded) => Class::Unknown,
        Some(class) => class,
        None if spec.ppid_one
            && spec.age_minutes >= 30
            && spec.cpu == 0
            && launchd_known
            && spec.has_path =>
        {
            Class::Suspect
        }
        None => Class::Unknown,
    }
}

pub fn run(specs: &[Spec], provenance: &Provenance, launchd_known: bool) -> Vec<Finding> {
    let (processes, in_launchd) = build(specs);
    let launchd = if launchd_known {
        Launchd::Known(in_launchd)
    } else {
        Launchd::Unavailable
    };
    classify(
        &snapshot_with(processes, launchd),
        provenance,
        &Policy::new(UID, SELF_PID),
    )
}

pub fn class_at(findings: &[Finding], pid: i32) -> Class {
    findings
        .iter()
        .find(|finding| finding.identity.kernel.pid == pid)
        .map(|finding| finding.class)
        .unwrap_or_else(|| panic!("no finding for pid {pid}"))
}
