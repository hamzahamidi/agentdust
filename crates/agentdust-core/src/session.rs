use std::collections::{BTreeSet, HashMap};

use crate::identity::KernelIdentity;
use crate::journal::{Agent, ExeBase, Kind, Record, SessionTagKey};
use crate::provider::{ProcessProvider, ProcessRead};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    Unknown,
    Active,
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Liveness {
    Alive,
    Gone,
    Unknown,
}

pub trait LivenessProbe {
    fn probe(&self, identity: &KernelIdentity) -> Liveness;
}

pub struct ProviderLiveness<'a, P: ?Sized>(pub &'a P);

impl<P: ProcessProvider + ?Sized> LivenessProbe for ProviderLiveness<'_, P> {
    fn probe(&self, expected: &KernelIdentity) -> Liveness {
        if expected.pid <= 0 {
            return Liveness::Unknown;
        }
        match self.0.read(expected.pid) {
            Err(_) => Liveness::Unknown,
            Ok(ProcessRead::Gone) => Liveness::Gone,
            Ok(ProcessRead::PathUnreadable(fresh)) => compare(expected, &fresh),
            Ok(ProcessRead::Present(fresh)) => compare(expected, &fresh.kernel),
        }
    }
}

fn compare(expected: &KernelIdentity, fresh: &KernelIdentity) -> Liveness {
    if expected == fresh {
        Liveness::Alive
    } else {
        Liveness::Gone
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub agent: Agent,
    pub session_id: String,
    pub identity: Option<KernelIdentity>,
    pub exe_base: Option<ExeBase>,
    pub state: State,
    pub session_ended: bool,
    pub liveness: Option<Liveness>,
    pub tag_keys: BTreeSet<SessionTagKey>,
    pub subagent_ids: BTreeSet<String>,
}

impl Scope {
    pub fn agent_gone(&self) -> bool {
        self.liveness == Some(Liveness::Gone)
    }

    pub fn degraded(&self) -> bool {
        self.identity.is_none()
    }
}

struct Draft {
    identity: Option<KernelIdentity>,
    exe_base: Option<ExeBase>,
    started: bool,
    ended: bool,
    tag_keys: BTreeSet<SessionTagKey>,
    subagent_ids: BTreeSet<String>,
}

impl Draft {
    fn new(identity: Option<KernelIdentity>) -> Self {
        Self {
            identity,
            exe_base: None,
            started: false,
            ended: false,
            tag_keys: BTreeSet::new(),
            subagent_ids: BTreeSet::new(),
        }
    }

    fn apply(&mut self, record: &Record, recorded: Option<&KernelIdentity>) {
        if self.exe_base.is_none() && recorded.is_some() && recorded == self.identity.as_ref() {
            self.exe_base = record
                .agent_identity
                .as_ref()
                .and_then(|identity| identity.exe_base().cloned());
        }
        if let Some(subagent) = &record.subagent_id {
            self.subagent_ids.insert(subagent.clone());
            return;
        }
        match record.kind {
            Kind::SessionStart => {
                self.started = true;
                if let Some(key) = &record.session_tag_key {
                    self.tag_keys.insert(key.clone());
                }
            }
            Kind::SessionEnd => self.ended = true,
            Kind::ShellStart | Kind::ShellEnd | Kind::Sample | Kind::ServerStart => {}
        }
    }
}

struct Entry {
    agent: Agent,
    session_id: String,
    draft: Draft,
}

fn open(
    entries: &mut Vec<Entry>,
    group: &mut Vec<usize>,
    record: &Record,
    identity: Option<&KernelIdentity>,
) -> usize {
    entries.push(Entry {
        agent: record.agent,
        session_id: record.session_id.clone(),
        draft: Draft::new(identity.cloned()),
    });
    group.push(entries.len() - 1);
    entries.len() - 1
}

fn target(
    entries: &mut Vec<Entry>,
    group: &mut Vec<usize>,
    record: &Record,
    recorded: Option<&KernelIdentity>,
) -> usize {
    let Some(&latest) = group.last() else {
        return open(entries, group, record, recorded);
    };
    let matching = recorded.and_then(|identity| {
        group
            .iter()
            .copied()
            .find(|&index| entries[index].draft.identity.as_ref() == Some(identity))
    });
    if record.subagent_id.is_some() {
        return matching.unwrap_or(latest);
    }
    let opens = match recorded {
        Some(_) => matching.is_none(),
        None => record.kind == Kind::SessionStart && entries[latest].draft.identity.is_some(),
    };
    if opens {
        return open(entries, group, record, recorded);
    }
    matching.unwrap_or(latest)
}

pub fn scopes(records: &[Record], probe: &dyn LivenessProbe) -> Vec<Scope> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut groups: HashMap<(Agent, &str), Vec<usize>> = HashMap::new();
    for record in records {
        let group = groups
            .entry((record.agent, record.session_id.as_str()))
            .or_default();
        let recorded = record
            .agent_identity
            .as_ref()
            .map(|identity| identity.kernel(&record.boot));
        let index = target(&mut entries, group, record, recorded.as_ref());
        entries[index].draft.apply(record, recorded.as_ref());
    }
    let mut probed: HashMap<KernelIdentity, Liveness> = HashMap::new();
    entries
        .into_iter()
        .map(
            |Entry {
                 agent,
                 session_id,
                 draft,
             }| {
                let liveness = draft.identity.as_ref().map(|identity| {
                    *probed
                        .entry(identity.clone())
                        .or_insert_with(|| probe.probe(identity))
                });
                let state = if liveness == Some(Liveness::Gone) || draft.ended {
                    State::Ended
                } else if draft.started || liveness == Some(Liveness::Alive) {
                    State::Active
                } else {
                    State::Unknown
                };
                Scope {
                    agent,
                    session_id,
                    identity: draft.identity,
                    exe_base: draft.exe_base,
                    state,
                    session_ended: draft.ended,
                    liveness,
                    tag_keys: draft.tag_keys,
                    subagent_ids: draft.subagent_ids,
                }
            },
        )
        .collect()
}
