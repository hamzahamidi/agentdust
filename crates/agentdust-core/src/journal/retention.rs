use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use super::{Agent, Kind, Record};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub ended_max_age: Duration,
    pub max_bytes: u64,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            ended_max_age: Duration::from_secs(14 * 24 * 60 * 60),
            max_bytes: 20_000_000,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Entry<'a> {
    pub record: &'a Record,
    pub bytes: u64,
    pub droppable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Degraded {
    pub agent: Agent,
    pub session_id: String,
    pub dropped_records: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub keep: Vec<bool>,
    pub dropped_earlier_boot: usize,
    pub dropped_aged: usize,
    pub dropped_over_cap: usize,
    pub dropped_pinned: usize,
    pub degraded: Vec<Degraded>,
}

type Scope<'a> = (Agent, &'a str, &'a str);

type Stamp = (u64, u64, usize);

struct ScopeInfo {
    ended: bool,
    newest: u64,
    last_lifecycle: Option<(Stamp, bool)>,
}

struct Run<'a> {
    entries: &'a [Entry<'a>],
    scopes: HashMap<Scope<'a>, ScopeInfo>,
    plan: Plan,
    total: u64,
    cap: u64,
}

pub fn plan(entries: &[Entry<'_>], policy: &Policy, now_ms: u64, current_boot: &str) -> Plan {
    let mut run = Run {
        entries,
        scopes: scopes_of(entries),
        plan: Plan {
            keep: vec![true; entries.len()],
            ..Plan::default()
        },
        total: entries
            .iter()
            .fold(0, |total, entry| total.saturating_add(entry.bytes)),
        cap: policy.max_bytes,
    };
    let max_age = u64::try_from(policy.ended_max_age.as_millis()).unwrap_or(u64::MAX);
    run.drop_earlier_boots_and_aged_sessions(now_ms, max_age, current_boot);
    if run.total > run.cap {
        run.drop_ended_sessions(current_boot);
    }
    if run.total > run.cap {
        run.drop_pinned_evidence(current_boot);
    }
    run.plan
}

fn scope_of(record: &Record) -> Scope<'_> {
    (record.agent, record.session_id.as_str(), record.boot.as_str())
}

fn scopes_of<'a>(entries: &[Entry<'a>]) -> HashMap<Scope<'a>, ScopeInfo> {
    let mut scopes: HashMap<Scope<'a>, ScopeInfo> = HashMap::new();
    for (index, entry) in entries.iter().enumerate() {
        let record = entry.record;
        let info = scopes.entry(scope_of(record)).or_insert(ScopeInfo {
            ended: false,
            newest: 0,
            last_lifecycle: None,
        });
        info.newest = info.newest.max(record.wall_ts);
        if matches!(record.kind, Kind::SessionStart | Kind::SessionEnd) {
            let stamp = (record.mono_ts, record.wall_ts, index);
            if info.last_lifecycle.is_none_or(|(last, _)| stamp > last) {
                info.last_lifecycle = Some((stamp, record.kind == Kind::SessionEnd));
            }
        }
    }
    for info in scopes.values_mut() {
        info.ended = info.last_lifecycle.is_some_and(|(_, ends)| ends);
    }
    scopes
}

impl<'a> Run<'a> {
    fn drop_entry(&mut self, index: usize) {
        self.plan.keep[index] = false;
        self.total = self.total.saturating_sub(self.entries[index].bytes);
    }

    fn info(&self, record: &'a Record) -> &ScopeInfo {
        &self.scopes[&scope_of(record)]
    }

    fn drop_earlier_boots_and_aged_sessions(&mut self, now_ms: u64, max_age: u64, current_boot: &str) {
        for (index, entry) in self.entries.iter().enumerate() {
            if !entry.droppable {
                continue;
            }
            if entry.record.boot != current_boot {
                self.plan.dropped_earlier_boot += 1;
                self.drop_entry(index);
                continue;
            }
            let info = self.info(entry.record);
            if info.ended && now_ms.saturating_sub(info.newest) > max_age {
                self.plan.dropped_aged += 1;
                self.drop_entry(index);
            }
        }
    }

    fn drop_ended_sessions(&mut self, current_boot: &str) {
        let mut sessions: BTreeMap<(u64, u8, &str), Vec<usize>> = BTreeMap::new();
        for (index, entry) in self.entries.iter().enumerate() {
            let info = self.info(entry.record);
            if self.plan.keep[index] && entry.droppable && entry.record.boot == current_boot && info.ended {
                sessions
                    .entry((
                        info.newest,
                        entry.record.agent as u8,
                        entry.record.session_id.as_str(),
                    ))
                    .or_default()
                    .push(index);
            }
        }
        for indexes in sessions.into_values() {
            if self.total <= self.cap {
                return;
            }
            for index in indexes {
                self.plan.dropped_over_cap += 1;
                self.drop_entry(index);
            }
        }
    }

    fn drop_pinned_evidence(&mut self, current_boot: &str) {
        let mut candidates: Vec<(PinnedKey<'a>, usize)> = Vec::new();
        for (index, entry) in self.entries.iter().enumerate() {
            let info = self.info(entry.record);
            if self.plan.keep[index] && entry.droppable && entry.record.boot == current_boot && !info.ended {
                let record = entry.record;
                candidates.push((
                    (
                        info.newest,
                        record.agent as u8,
                        record.session_id.as_str(),
                        record.kind == Kind::SessionStart,
                        record.wall_ts,
                        record.mono_ts,
                    ),
                    index,
                ));
            }
        }
        candidates.sort_unstable();
        let mut lost: BTreeMap<(u8, &str), (Agent, usize)> = BTreeMap::new();
        for (_, index) in candidates {
            if self.total <= self.cap {
                break;
            }
            let record = self.entries[index].record;
            lost.entry((record.agent as u8, record.session_id.as_str()))
                .or_insert((record.agent, 0))
                .1 += 1;
            self.plan.dropped_pinned += 1;
            self.drop_entry(index);
        }
        self.plan.degraded = lost
            .into_iter()
            .map(|((_, session), (agent, dropped_records))| Degraded {
                agent,
                session_id: session.to_owned(),
                dropped_records,
            })
            .collect();
    }
}

type PinnedKey<'a> = (u64, u8, &'a str, bool, u64, u64);
