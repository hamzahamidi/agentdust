use std::collections::{BTreeSet, HashMap, HashSet};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::Duration;

use serde::{Serialize, Serializer};

use crate::ancestry;
use crate::class::{Actionability, Class, actionable_as};
use crate::identity::KernelIdentity;
use crate::inventory::{Launchd, RawIdentity, RawProcess, Snapshot, Tag};
use crate::journal::{Agent, SessionTagKey};
use crate::session::{Liveness, Scope};

pub const SUSPECT_MIN_AGE: Duration = Duration::from_secs(30 * 60);
pub const SYSTEM_PREFIXES: [&str; 6] = [
    "/System",
    "/usr",
    "/bin",
    "/sbin",
    "/Library/Apple",
    "/Applications",
];
const ANCESTOR_LIMIT: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Evidence {
    ManagedInit,
    ManagedOtherUser,
    ManagedSelf,
    ManagedAncestor,
    ManagedAgent,
    ManagedSystemPath,
    ManagedLaunchd,
    ManagedLaunchdChild,
    OwnedTag,
    OwnedAgentAlive,
    OwnedAgentUnverified,
    OwnedAgentGone,
    TagUnmatched,
    TagUnverifiable,
    TagDegraded,
    SuspectParentLaunchd,
    SuspectSameUser,
    SuspectAge,
    SuspectIdle,
    LaunchdUnavailable,
    PathUnreadable,
}

impl Evidence {
    pub const ALL: [Evidence; 21] = [
        Evidence::ManagedInit,
        Evidence::ManagedOtherUser,
        Evidence::ManagedSelf,
        Evidence::ManagedAncestor,
        Evidence::ManagedAgent,
        Evidence::ManagedSystemPath,
        Evidence::ManagedLaunchd,
        Evidence::ManagedLaunchdChild,
        Evidence::OwnedTag,
        Evidence::OwnedAgentAlive,
        Evidence::OwnedAgentUnverified,
        Evidence::OwnedAgentGone,
        Evidence::TagUnmatched,
        Evidence::TagUnverifiable,
        Evidence::TagDegraded,
        Evidence::SuspectParentLaunchd,
        Evidence::SuspectSameUser,
        Evidence::SuspectAge,
        Evidence::SuspectIdle,
        Evidence::LaunchdUnavailable,
        Evidence::PathUnreadable,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Evidence::ManagedInit => "managed.deny.init",
            Evidence::ManagedOtherUser => "managed.deny.other_user",
            Evidence::ManagedSelf => "managed.deny.self",
            Evidence::ManagedAncestor => "managed.deny.ancestor",
            Evidence::ManagedAgent => "managed.deny.agent",
            Evidence::ManagedSystemPath => "managed.deny.system_path",
            Evidence::ManagedLaunchd => "managed.launchd",
            Evidence::ManagedLaunchdChild => "managed.launchd_child",
            Evidence::OwnedTag => "owned.tag",
            Evidence::OwnedAgentAlive => "owned.agent_alive",
            Evidence::OwnedAgentUnverified => "owned.agent_unverified",
            Evidence::OwnedAgentGone => "owned.agent_gone",
            Evidence::TagUnmatched => "tag.unmatched",
            Evidence::TagUnverifiable => "tag.unverifiable",
            Evidence::TagDegraded => "tag.degraded_session",
            Evidence::SuspectParentLaunchd => "suspect.parent_launchd",
            Evidence::SuspectSameUser => "suspect.same_user",
            Evidence::SuspectAge => "suspect.age",
            Evidence::SuspectIdle => "suspect.idle",
            Evidence::LaunchdUnavailable => "launchd.unavailable",
            Evidence::PathUnreadable => "identity.path_unreadable",
        }
    }
}

impl Serialize for Evidence {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    pub uid: u32,
    pub self_pid: i32,
    pub min_age: Duration,
}

impl Policy {
    pub fn new(uid: u32, self_pid: i32) -> Self {
        Self {
            uid,
            self_pid,
            min_age: SUSPECT_MIN_AGE,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Provenance<'a> {
    Available(&'a [Scope]),
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub identity: RawIdentity,
    pub class: Class,
    pub evidence: Vec<Evidence>,
    pub age_us: u64,
    pub attribution_owners: Option<Vec<AttributionOwner>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributionOwner {
    pub agent: Agent,
    pub session_id: String,
    pub identity: KernelIdentity,
}

impl Finding {
    pub fn actionability(&self) -> Actionability {
        actionable_as(self.class)
    }
}

pub fn classify(snapshot: &Snapshot, provenance: &Provenance, policy: &Policy) -> Vec<Finding> {
    let context = Context::new(snapshot, provenance, policy);
    snapshot
        .processes
        .iter()
        .map(|process| {
            let (class, evidence) = context.decide(process);
            Finding {
                identity: process.identity.clone(),
                class,
                evidence,
                age_us: context.age_us(process),
                attribution_owners: context.attribution_owners(process),
            }
        })
        .collect()
}

struct Context<'a> {
    taken_at_us: u64,
    policy: &'a Policy,
    launchd: Option<&'a BTreeSet<i32>>,
    scopes: Option<&'a [Scope]>,
    parents: HashMap<i32, i32>,
    ancestors: HashSet<i32>,
    agents: HashSet<&'a KernelIdentity>,
    by_key: HashMap<&'a SessionTagKey, Vec<&'a Scope>>,
}

impl<'a> Context<'a> {
    fn new(snapshot: &'a Snapshot, provenance: &Provenance<'a>, policy: &'a Policy) -> Self {
        let scopes = match provenance {
            Provenance::Available(scopes) => Some(*scopes),
            Provenance::Unavailable => None,
        };
        let mut agents = HashSet::new();
        let mut by_key: HashMap<&SessionTagKey, Vec<&Scope>> = HashMap::new();
        for scope in scopes.unwrap_or_default() {
            agents.extend(scope.identity.as_ref());
            agents.extend(scope.additional_owners.iter().map(|owner| &owner.identity));
            for key in &scope.tag_keys {
                by_key.entry(key).or_default().push(scope);
            }
        }
        let parents: HashMap<i32, i32> = snapshot
            .processes
            .iter()
            .map(|process| (process.identity.kernel.pid, process.identity.ppid))
            .collect();
        Self {
            taken_at_us: snapshot.taken_at_us,
            policy,
            launchd: match &snapshot.launchd {
                Launchd::Known(pids) => Some(pids),
                Launchd::Unavailable => None,
            },
            scopes,
            ancestors: ancestors(&parents, policy.self_pid),
            parents,
            agents,
            by_key,
        }
    }

    fn age_us(&self, process: &RawProcess) -> u64 {
        self.taken_at_us
            .saturating_sub(process.identity.kernel.start_time_us)
    }

    fn attribution_owners(&self, process: &RawProcess) -> Option<Vec<AttributionOwner>> {
        let Tag::Keyed(key) = &process.tag else {
            return None;
        };
        self.scopes?;
        let matching = self.by_key.get(key)?;
        if matching.is_empty() || matching.iter().any(|scope| scope.degraded()) {
            return None;
        }
        let mut owners: Vec<AttributionOwner> = matching
            .iter()
            .flat_map(|scope| {
                scope.owner_states().into_iter().map(|owner| AttributionOwner {
                    agent: scope.agent,
                    session_id: scope.session_id.clone(),
                    identity: owner.identity,
                })
            })
            .collect();
        if owners.is_empty() {
            return None;
        }
        owners.sort_by(|left, right| {
            (
                left.agent,
                left.session_id.as_str(),
                left.identity.boot_session_uuid.as_str(),
                left.identity.pid,
                left.identity.start_time_us,
                left.identity.uid,
            )
                .cmp(&(
                    right.agent,
                    right.session_id.as_str(),
                    right.identity.boot_session_uuid.as_str(),
                    right.identity.pid,
                    right.identity.start_time_us,
                    right.identity.uid,
                ))
        });
        owners.dedup();
        Some(owners)
    }

    fn decide(&self, process: &RawProcess) -> (Class, Vec<Evidence>) {
        let managed = self.managed(process);
        if !managed.is_empty() {
            return (Class::Managed, managed);
        }
        match &process.tag {
            Tag::Absent => self.untagged(process),
            Tag::Unkeyed | Tag::Unreadable => (Class::Unknown, vec![Evidence::TagUnverifiable]),
            Tag::Keyed(key) => self.tagged(process, key),
        }
    }

    fn managed(&self, process: &RawProcess) -> Vec<Evidence> {
        let kernel = &process.identity.kernel;
        let mut evidence = Vec::new();
        if kernel.pid <= 1 {
            evidence.push(Evidence::ManagedInit);
        }
        if kernel.uid != self.policy.uid {
            evidence.push(Evidence::ManagedOtherUser);
        }
        if kernel.pid == self.policy.self_pid {
            evidence.push(Evidence::ManagedSelf);
        }
        if self.ancestors.contains(&kernel.pid) {
            evidence.push(Evidence::ManagedAncestor);
        }
        if self.is_agent(process) {
            evidence.push(Evidence::ManagedAgent);
        }
        if process.identity.exe_path.as_deref().is_some_and(system_path) {
            evidence.push(Evidence::ManagedSystemPath);
        }
        if self.launchd.is_some_and(|pids| pids.contains(&kernel.pid)) {
            evidence.push(Evidence::ManagedLaunchd);
        } else if self.child_of_launchd_job(kernel.pid) {
            evidence.push(Evidence::ManagedLaunchdChild);
        }
        evidence
    }

    fn child_of_launchd_job(&self, pid: i32) -> bool {
        match (self.launchd, self.parents.get(&pid)) {
            (Some(jobs), Some(parent)) => jobs.contains(parent),
            _ => false,
        }
    }

    fn is_agent(&self, process: &RawProcess) -> bool {
        process.agent_script
            || process
                .identity
                .exe_path
                .as_deref()
                .is_some_and(ancestry::agent_exe)
            || self.agents.contains(&process.identity.kernel)
    }

    fn tagged(&self, process: &RawProcess, key: &SessionTagKey) -> (Class, Vec<Evidence>) {
        if self.scopes.is_none() {
            return (Class::Unknown, vec![Evidence::TagUnverifiable]);
        }
        let Some(matching) = self.by_key.get(key) else {
            return (Class::Unknown, vec![Evidence::TagUnmatched]);
        };
        let degraded = matching.iter().any(|scope| scope.degraded());
        let anchored: Vec<&&Scope> = matching.iter().filter(|scope| !scope.degraded()).collect();
        if anchored.is_empty() {
            return (Class::Unknown, vec![Evidence::TagDegraded]);
        }
        let owner_states: Vec<_> = anchored.iter().flat_map(|scope| scope.owner_states()).collect();
        if owner_states.is_empty() {
            return (Class::Unknown, vec![Evidence::TagDegraded]);
        }
        if owner_states.iter().any(|owner| owner.liveness != Liveness::Gone) {
            let alive = owner_states.iter().any(|owner| owner.liveness == Liveness::Alive);
            let mut evidence = vec![
                Evidence::OwnedTag,
                if alive {
                    Evidence::OwnedAgentAlive
                } else {
                    Evidence::OwnedAgentUnverified
                },
            ];
            if degraded {
                evidence.push(Evidence::TagDegraded);
            }
            return (Class::OwnedLive, evidence);
        }
        let mut evidence = vec![Evidence::OwnedTag, Evidence::OwnedAgentGone];
        if degraded {
            evidence.push(Evidence::TagDegraded);
            return (Class::Unknown, evidence);
        }
        self.settle(process, Class::OwnedEnded, evidence)
    }

    fn untagged(&self, process: &RawProcess) -> (Class, Vec<Evidence>) {
        let old = u128::from(self.age_us(process)) >= self.policy.min_age.as_micros();
        if process.identity.ppid != 1 || !old || !process.cpu.idle() {
            return (Class::Unknown, Vec::new());
        }
        let evidence = vec![
            Evidence::SuspectParentLaunchd,
            Evidence::SuspectSameUser,
            Evidence::SuspectAge,
            Evidence::SuspectIdle,
        ];
        self.settle(process, Class::Suspect, evidence)
    }

    fn settle(
        &self,
        process: &RawProcess,
        class: Class,
        mut evidence: Vec<Evidence>,
    ) -> (Class, Vec<Evidence>) {
        let mut held_back = false;
        if process.identity.exe_path.is_none() {
            evidence.push(Evidence::PathUnreadable);
            held_back = true;
        }
        if self.launchd.is_none() {
            evidence.push(Evidence::LaunchdUnavailable);
            held_back = true;
        }
        if held_back {
            (Class::Unknown, evidence)
        } else {
            (class, evidence)
        }
    }
}

fn ancestors(parents: &HashMap<i32, i32>, self_pid: i32) -> HashSet<i32> {
    let mut found = HashSet::new();
    let mut pid = self_pid;
    for _ in 0..ANCESTOR_LIMIT {
        let Some(&parent) = parents.get(&pid) else {
            break;
        };
        if parent <= 1 || !found.insert(parent) {
            break;
        }
        pid = parent;
    }
    found
}

fn system_path(path: &Path) -> bool {
    let bytes = path.as_os_str().as_bytes();
    SYSTEM_PREFIXES.iter().any(|prefix| {
        bytes
            .strip_prefix(prefix.as_bytes())
            .is_some_and(|rest| rest.first().is_none_or(|byte| *byte == b'/'))
    })
}
