use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::Serialize;
use thiserror::Error;

use crate::apply::MAX_ITEMS_PER_CALL;
use crate::apply::exec::{Deps, Executor, Outcome, Reason, Settings, Verdict};
use crate::apply::timer::Timer;
use crate::class::Class;
use crate::code::{self, Check};
use crate::config::Disabled;
use crate::digest::to_hex;
use crate::entropy;
use crate::finding::HumanDisplay;
use crate::plan::{ClaimError, Created, PlanError, PlanItem, PlanStore};

const NONCE_BYTES: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub plan_id: String,
    pub item_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    pub nonce: String,
    pub message: String,
    pub index: usize,
    pub total: usize,
    pub item_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    Accept(Option<String>),
    Decline,
    Cancel,
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ItemReport {
    pub item_id: String,
    pub result: Outcome,
    pub reason: Option<Reason>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub plan_id: String,
    pub items: Vec<ItemReport>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Ask(Challenge),
    Done(Report),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ApplyError {
    #[error("apply is switched off: {}", .0.describe())]
    Disabled(Disabled),
    #[error("a call needs at least one item")]
    NoItems,
    #[error("a call takes at most {max} items and {given} were given")]
    TooManyItems { given: usize, max: usize },
    #[error("item {index} is named twice")]
    DuplicateItem { index: usize },
    #[error("the plan is unknown or has expired")]
    UnknownPlan,
    #[error("item {index} is not in the plan")]
    UnknownItem { index: usize },
    #[error("item {index} is already being approved or has been applied")]
    ItemUnavailable { index: usize },
    #[error("the request state is unknown or has been used")]
    UnknownRequestState,
    #[error("the arguments differ from the call that the request state belongs to")]
    ArgumentsChanged,
    #[error("the operating system gave no random bytes")]
    Entropy,
}

struct Pending {
    call: Call,
    units: Vec<Vec<PlanItem>>,
    index: usize,
    code: String,
    expires_at: Duration,
    done: Vec<ItemReport>,
}

enum Decision {
    Approved,
    Refused(Outcome),
}

pub struct Server {
    executor: Executor,
    store: PlanStore,
    timer: Arc<dyn Timer>,
    settings: Settings,
    pending: Mutex<HashMap<String, Pending>>,
}

impl Server {
    pub fn new(deps: Deps, settings: Settings) -> Self {
        let timer = Arc::clone(&deps.timer);
        let store = PlanStore::new(&deps.data_dir, settings.plan_ttl, Arc::clone(&timer));
        Self {
            executor: Executor::new(deps, settings),
            store,
            timer,
            settings,
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn data_dir(&self) -> &Path {
        self.executor.data_dir()
    }

    pub fn plan(&self) -> Result<Created, PlanError> {
        self.expire_pending();
        self.store.create(self.executor.surveyor())
    }

    pub fn begin(&self, call: &Call) -> Result<Step, ApplyError> {
        self.executor.enabled().map_err(ApplyError::Disabled)?;
        self.expire_pending();
        check_shape(call)?;
        let claimed = self
            .store
            .claim(&call.plan_id, &call.item_ids)
            .map_err(|err| match err {
                ClaimError::UnknownPlan => ApplyError::UnknownPlan,
                ClaimError::UnknownItem { index } => ApplyError::UnknownItem { index },
                ClaimError::Unavailable { index } => ApplyError::ItemUnavailable { index },
            })?;
        self.ask(Pending {
            call: call.clone(),
            units: units_of(claimed),
            index: 0,
            code: String::new(),
            expires_at: Duration::ZERO,
            done: Vec::new(),
        })
    }

    pub fn answer(&self, call: &Call, nonce: &str, response: Response) -> Result<Step, ApplyError> {
        let mut entry = self
            .pending()
            .remove(nonce)
            .ok_or(ApplyError::UnknownRequestState)?;
        if entry.call != *call {
            self.release_rest(&entry);
            return Err(ApplyError::ArgumentsChanged);
        }
        if let Err(reason) = self.executor.enabled() {
            let verdict = Verdict::of(Outcome::Disabled);
            for unit in &entry.units[entry.index..] {
                for item in unit {
                    self.executor.note(&entry.call.plan_id, item, verdict);
                }
            }
            self.release_rest(&entry);
            return Err(ApplyError::Disabled(reason));
        }
        if !self.store.alive(&call.plan_id) {
            return Ok(Step::Done(self.finish_rest(entry, Outcome::PlanExpired, true)));
        }
        let unit = entry.units[entry.index].clone();
        let reports = match self.decide(&entry, response) {
            Decision::Approved => self.execute_unit(&call.plan_id, &unit),
            Decision::Refused(outcome) => self.refuse_unit(&call.plan_id, &unit, outcome),
        };
        entry.done.extend(reports);
        entry.index += 1;
        if entry.index >= entry.units.len() {
            return Ok(Step::Done(report_of(entry)));
        }
        if self.executor.enabled().is_err() {
            return Ok(Step::Done(self.finish_rest(entry, Outcome::Disabled, true)));
        }
        self.ask(entry)
    }

    pub fn run(
        &self,
        call: &Call,
        ask: &mut dyn FnMut(&Challenge) -> Response,
    ) -> Result<Report, ApplyError> {
        let mut step = self.begin(call)?;
        loop {
            match step {
                Step::Done(report) => return Ok(report),
                Step::Ask(challenge) => {
                    let response = ask(&challenge);
                    step = self.answer(call, &challenge.nonce, response)?;
                }
            }
        }
    }

    fn pending(&self) -> MutexGuard<'_, HashMap<String, Pending>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn ask(&self, mut entry: Pending) -> Result<Step, ApplyError> {
        let drawn = (code::generate(), entropy::bytes::<NONCE_BYTES>());
        let (Ok(code), Ok(nonce)) = drawn else {
            self.release_rest(&entry);
            return Err(ApplyError::Entropy);
        };
        let unit = &entry.units[entry.index];
        let challenge = Challenge {
            nonce: to_hex(&nonce),
            message: prompt(unit, &code),
            index: entry.index,
            total: entry.units.len(),
            item_ids: unit.iter().map(|item| item.model.item_id.clone()).collect(),
        };
        entry.code = code;
        entry.expires_at = self.timer.now() + self.settings.code_ttl;
        self.pending().insert(challenge.nonce.clone(), entry);
        Ok(Step::Ask(challenge))
    }

    fn decide(&self, entry: &Pending, response: Response) -> Decision {
        match response {
            Response::Decline => Decision::Refused(Outcome::Declined),
            Response::Cancel => Decision::Refused(Outcome::Cancelled),
            Response::Timeout => Decision::Refused(Outcome::TimedOut),
            Response::Accept(answer) => {
                if self.timer.now() >= entry.expires_at {
                    return Decision::Refused(Outcome::Expired);
                }
                match code::check(answer.as_deref(), &entry.code) {
                    Check::Approved => Decision::Approved,
                    Check::Wrong => Decision::Refused(Outcome::WrongCode),
                    Check::Empty => Decision::Refused(Outcome::Empty),
                }
            }
        }
    }

    fn execute_unit(&self, plan_id: &str, unit: &[PlanItem]) -> Vec<ItemReport> {
        let mut reports = Vec::with_capacity(unit.len());
        let mut finished = Vec::new();
        let mut released = Vec::new();
        for item in unit {
            let verdict = if self.store.alive(plan_id) {
                self.executor.execute(plan_id, item)
            } else {
                let verdict = Verdict::of(Outcome::PlanExpired);
                self.executor.note(plan_id, item, verdict);
                verdict
            };
            let id = item.model.item_id.clone();
            match verdict.outcome {
                Outcome::HandledElsewhere
                | Outcome::Disabled
                | Outcome::LockUnavailable
                | Outcome::AuditUnavailable
                | Outcome::PlanExpired => released.push(id.clone()),
                _ => finished.push(id.clone()),
            }
            reports.push(report_item(id, verdict));
        }
        self.store.finish(plan_id, &finished);
        self.store.release(plan_id, &released);
        reports
    }

    fn refuse_unit(&self, plan_id: &str, unit: &[PlanItem], outcome: Outcome) -> Vec<ItemReport> {
        let verdict = Verdict::of(outcome);
        for item in unit {
            self.executor.note(plan_id, item, verdict);
        }
        let ids: Vec<String> = unit.iter().map(|item| item.model.item_id.clone()).collect();
        self.store.release(plan_id, &ids);
        ids.into_iter().map(|id| report_item(id, verdict)).collect()
    }

    fn finish_rest(&self, mut entry: Pending, outcome: Outcome, note: bool) -> Report {
        let verdict = Verdict::of(outcome);
        let plan_id = entry.call.plan_id.clone();
        for unit in &entry.units[entry.index..] {
            for item in unit {
                if note {
                    self.executor.note(&plan_id, item, verdict);
                }
                entry.done.push(report_item(item.model.item_id.clone(), verdict));
            }
        }
        self.release_rest(&entry);
        report_of(entry)
    }

    fn release_rest(&self, entry: &Pending) {
        for unit in &entry.units[entry.index..] {
            let ids: Vec<String> = unit.iter().map(|item| item.model.item_id.clone()).collect();
            self.store.release(&entry.call.plan_id, &ids);
        }
    }

    fn expire_pending(&self) {
        let now = self.timer.now();
        let expired: Vec<Pending> = {
            let mut pending = self.pending();
            let stale: Vec<String> = pending
                .iter()
                .filter(|(_, entry)| now >= entry.expires_at)
                .map(|(nonce, _)| nonce.clone())
                .collect();
            stale.iter().filter_map(|nonce| pending.remove(nonce)).collect()
        };
        for entry in &expired {
            self.release_rest(entry);
        }
    }
}

fn check_shape(call: &Call) -> Result<(), ApplyError> {
    if call.item_ids.is_empty() {
        return Err(ApplyError::NoItems);
    }
    if call.item_ids.len() > MAX_ITEMS_PER_CALL {
        return Err(ApplyError::TooManyItems {
            given: call.item_ids.len(),
            max: MAX_ITEMS_PER_CALL,
        });
    }
    for (index, id) in call.item_ids.iter().enumerate() {
        if call.item_ids[..index].contains(id) {
            return Err(ApplyError::DuplicateItem { index });
        }
    }
    Ok(())
}

fn units_of(claimed: Vec<PlanItem>) -> Vec<Vec<PlanItem>> {
    let (owned, suspects): (Vec<PlanItem>, Vec<PlanItem>) = claimed
        .into_iter()
        .partition(|item| item.model.class == Class::OwnedEnded);
    let mut units = Vec::new();
    if !owned.is_empty() {
        units.push(owned);
    }
    units.extend(suspects.into_iter().map(|item| vec![item]));
    units
}

fn prompt(unit: &[PlanItem], code: &str) -> String {
    let lines: Vec<String> = unit
        .iter()
        .map(|item| HumanDisplay::prompt(&item.model))
        .collect();
    let body = lines.join("\n");
    match (unit[0].model.class, unit.len()) {
        (Class::OwnedEnded, 1) => format!(
            "Stop 1 process that a Claude Code session left behind? The session has ended. It gets one SIGTERM.\n\n{body}\n\nType {code} to approve."
        ),
        (Class::OwnedEnded, count) => format!(
            "Stop {count} processes that a Claude Code session left behind? The session has ended. Each gets one SIGTERM.\n\n{body}\n\nType {code} to approve all {count}."
        ),
        _ => format!(
            "Stop this process? No session that AgentDust knows started it, and it has run detached and idle for a long time. It gets one SIGTERM.\n\n{body}\n\nType {code} to approve."
        ),
    }
}

fn report_item(item_id: String, verdict: Verdict) -> ItemReport {
    ItemReport {
        item_id,
        result: verdict.outcome,
        reason: verdict.reason,
    }
}

fn report_of(entry: Pending) -> Report {
    let mut by_id: HashMap<String, ItemReport> = entry
        .done
        .into_iter()
        .map(|report| (report.item_id.clone(), report))
        .collect();
    Report {
        plan_id: entry.call.plan_id,
        items: entry
            .call
            .item_ids
            .iter()
            .filter_map(|id| by_id.remove(id))
            .collect(),
    }
}
