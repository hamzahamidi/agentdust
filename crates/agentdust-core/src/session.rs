use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

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
    pub subagents: BTreeMap<String, SubagentActivity>,
    pub additional_owners: Vec<OwnerState>,
    pub attribution_ambiguous: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerState {
    pub identity: KernelIdentity,
    pub liveness: Liveness,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SubagentActivity {
    pub start_events: u32,
    pub stop_events: u32,
}

impl Scope {
    pub fn agent_gone(&self) -> bool {
        self.liveness == Some(Liveness::Gone)
            && self
                .additional_owners
                .iter()
                .all(|owner| owner.liveness == Liveness::Gone)
    }

    pub fn degraded(&self) -> bool {
        self.identity.is_none() || self.attribution_ambiguous
    }

    pub fn owner_states(&self) -> Vec<OwnerState> {
        let mut owners = self
            .identity
            .as_ref()
            .zip(self.liveness)
            .map(|(identity, liveness)| OwnerState {
                identity: identity.clone(),
                liveness,
            })
            .into_iter()
            .collect::<Vec<_>>();
        owners.extend(self.additional_owners.iter().cloned());
        owners
    }
}

struct Draft {
    identity: Option<KernelIdentity>,
    exe_base: Option<ExeBase>,
    started: bool,
    ended: bool,
    tag_keys: BTreeSet<SessionTagKey>,
    subagent_ids: BTreeSet<String>,
    subagents: BTreeMap<String, SubagentActivity>,
    additional_owners: HashSet<KernelIdentity>,
    attribution_ambiguous: bool,
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
            subagents: BTreeMap::new(),
            additional_owners: HashSet::new(),
            attribution_ambiguous: false,
        }
    }

    fn matches_identity(&self, identity: &KernelIdentity) -> bool {
        self.identity.as_ref() == Some(identity) || self.additional_owners.contains(identity)
    }

    fn apply(&mut self, record: &Record, recorded: Option<&KernelIdentity>) {
        if record.subagent_id.is_some()
            && let Some(identity) = recorded
            && self.identity.as_ref() != Some(identity)
        {
            self.additional_owners.insert(identity.clone());
        }
        if self.exe_base.is_none() && recorded.is_some() && recorded == self.identity.as_ref() {
            self.exe_base = record
                .agent_identity
                .as_ref()
                .and_then(|identity| identity.exe_base().cloned());
        }
        if let Some(subagent) = &record.subagent_id {
            self.subagent_ids.insert(subagent.clone());
            let activity = self.subagents.entry(subagent.clone()).or_default();
            match record.kind {
                Kind::SessionStart | Kind::SubagentStart => {
                    activity.start_events = activity.start_events.saturating_add(1);
                }
                Kind::SessionEnd | Kind::SubagentStop => {
                    activity.stop_events = activity.stop_events.saturating_add(1);
                }
                Kind::SubagentAttributionUnknown
                | Kind::ShellStart
                | Kind::ShellEnd
                | Kind::Sample
                | Kind::ServerStart => {}
            }
        }
        if record.agent == Agent::Codex
            && let Some(key) = &record.session_tag_key
        {
            self.tag_keys.insert(key.clone());
        }
        match record.kind {
            Kind::SessionStart => {
                self.started = true;
                if let Some(key) = &record.session_tag_key {
                    self.tag_keys.insert(key.clone());
                }
            }
            Kind::SessionEnd if record.subagent_id.is_none() => self.ended = true,
            Kind::SessionEnd
            | Kind::SubagentStart
            | Kind::SubagentStop
            | Kind::SubagentAttributionUnknown
            | Kind::ShellStart
            | Kind::ShellEnd
            | Kind::Sample
            | Kind::ServerStart => {}
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
) -> Option<usize> {
    let matching = recorded.and_then(|identity| {
        group
            .iter()
            .copied()
            .find(|&index| entries[index].draft.matches_identity(identity))
    });
    if record.subagent_id.is_some() {
        if matching.is_some() {
            return matching;
        }
        if group.len() == 1 {
            let index = group[0];
            if recorded.is_none() && record.kind != Kind::SubagentStop {
                entries[index].draft.attribution_ambiguous = true;
            }
            return Some(index);
        }
        if group.len() > 1 {
            for &index in group.iter() {
                entries[index].draft.attribution_ambiguous = true;
            }
            return None;
        }
        if recorded.is_some() || record.kind == Kind::SessionStart {
            return Some(open(entries, group, record, recorded));
        }
        if matches!(record.kind, Kind::SubagentStart | Kind::SubagentStop) {
            return None;
        }
        return Some(open(entries, group, record, recorded));
    }
    let Some(&latest) = group.last() else {
        return Some(open(entries, group, record, recorded));
    };
    let opens = match recorded {
        Some(_) => matching.is_none(),
        None => record.kind == Kind::SessionStart && entries[latest].draft.identity.is_some(),
    };
    if opens {
        return Some(open(entries, group, record, recorded));
    }
    if recorded.is_none() && group.len() > 1 && record.kind != Kind::SessionStart {
        return Some(open(entries, group, record, recorded));
    }
    Some(matching.unwrap_or(latest))
}

pub fn scopes(records: &[Record], probe: &dyn LivenessProbe) -> Vec<Scope> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut groups: HashMap<(Agent, &str), Vec<usize>> = HashMap::new();
    let mut ambiguous_sessions: HashSet<(Agent, String)> = HashSet::new();
    for record in records {
        if record.agent == Agent::Codex && record.agent_identity.is_none() {
            ambiguous_sessions.insert((record.agent, record.session_id.clone()));
        }
        if record.kind == Kind::SubagentAttributionUnknown
            || (matches!(record.kind, Kind::SubagentStart | Kind::SubagentStop)
                && record.subagent_id.is_none())
        {
            ambiguous_sessions.insert((record.agent, record.session_id.clone()));
            continue;
        }
        let group = groups
            .entry((record.agent, record.session_id.as_str()))
            .or_default();
        let recorded = record
            .agent_identity
            .as_ref()
            .map(|identity| identity.kernel(&record.boot));
        if record.subagent_id.is_some() {
            let resolved = recorded.as_ref().is_some_and(|identity| {
                group
                    .iter()
                    .any(|&index| entries[index].draft.matches_identity(identity))
                    || group.len() == 1
            });
            if !resolved {
                ambiguous_sessions.insert((record.agent, record.session_id.clone()));
            }
        }
        if let Some(index) = target(&mut entries, group, record, recorded.as_ref()) {
            entries[index].draft.apply(record, recorded.as_ref());
        } else {
            ambiguous_sessions.insert((record.agent, record.session_id.clone()));
        }
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
                let session_ambiguous = ambiguous_sessions.contains(&(agent, session_id.clone()));
                let liveness = draft.identity.as_ref().map(|identity| {
                    *probed
                        .entry(identity.clone())
                        .or_insert_with(|| probe.probe(identity))
                });
                let mut additional_owners: Vec<OwnerState> = draft
                    .additional_owners
                    .iter()
                    .map(|identity| OwnerState {
                        identity: identity.clone(),
                        liveness: *probed
                            .entry(identity.clone())
                            .or_insert_with(|| probe.probe(identity)),
                    })
                    .collect();
                additional_owners.sort_by(|left, right| {
                    (
                        left.identity.boot_session_uuid.as_str(),
                        left.identity.pid,
                        left.identity.start_time_us,
                        left.identity.uid,
                    )
                        .cmp(&(
                            right.identity.boot_session_uuid.as_str(),
                            right.identity.pid,
                            right.identity.start_time_us,
                            right.identity.uid,
                        ))
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
                    subagents: draft.subagents,
                    additional_owners,
                    attribution_ambiguous: draft.attribution_ambiguous
                        || session_ambiguous
                        || (agent == Agent::Codex && !draft.started),
                }
            },
        )
        .collect()
}
