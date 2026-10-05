use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use thiserror::Error;

use crate::apply::timer::Timer;
use crate::class::{Actionability, Class, actionable_as};
use crate::classifier::Finding;
use crate::clock;
use crate::digest::to_hex;
use crate::entropy;
use crate::finding::{HumanDisplay, ModelFinding};
use crate::identity::{IdentityEvidence, ProcessIdentity};
use crate::safe_open::{self, Access, SafeOpenError};
use crate::survey::Surveyor;

const INSPECTION_DIR: &str = "inspection";
const REPORT_SUFFIX: &str = ".txt";
const ID_BYTES: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanItem {
    pub model: ModelFinding,
    pub identity: ProcessIdentity,
}

impl PlanItem {
    pub fn plannable(finding: &Finding) -> bool {
        matches!(
            actionable_as(finding.class),
            Actionability::BatchCode | Actionability::PerItemCode
        ) && finding.identity.kernel.pid > 1
            && finding.identity.exe_path.is_some()
    }

    pub fn from_finding(finding: &Finding, model: ModelFinding) -> Option<PlanItem> {
        if !Self::plannable(finding) || model.class != finding.class {
            return None;
        }
        Some(PlanItem {
            model,
            identity: ProcessIdentity {
                kernel: finding.identity.kernel.clone(),
                evidence: IdentityEvidence {
                    exe_path: finding.identity.exe_path.clone()?,
                },
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    pub plan_id: String,
    pub expires_in: Duration,
    pub items: Vec<ModelFinding>,
    pub report: String,
}

#[derive(Debug, Error)]
pub enum PlanError {
    #[error("the process inventory failed: {0}")]
    Survey(io::Error),
    #[error("the inspection report could not be written: {0}")]
    Report(io::Error),
    #[error("the operating system gave no random bytes: {0}")]
    Entropy(io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ClaimError {
    #[error("the plan is unknown or has expired")]
    UnknownPlan,
    #[error("item {index} is not in the plan")]
    UnknownItem { index: usize },
    #[error("item {index} is already being approved or has been applied")]
    Unavailable { index: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Available,
    Pending,
    Done,
}

struct Slot {
    item: PlanItem,
    state: State,
}

struct Stored {
    slots: Vec<Slot>,
    expires_at: Duration,
    report: PathBuf,
}

pub struct PlanStore {
    data_dir: PathBuf,
    ttl: Duration,
    timer: Arc<dyn Timer>,
    plans: Mutex<HashMap<String, Stored>>,
}

impl PlanStore {
    pub fn new(data_dir: &Path, ttl: Duration, timer: Arc<dyn Timer>) -> Self {
        clear_stale_reports(&data_dir.join(INSPECTION_DIR));
        Self {
            data_dir: data_dir.to_path_buf(),
            ttl,
            timer,
            plans: Mutex::new(HashMap::new()),
        }
    }

    pub fn create(&self, surveyor: &dyn Surveyor) -> Result<Created, PlanError> {
        self.sweep();
        let found = surveyor.survey().map_err(PlanError::Survey)?;
        let mut items: Vec<PlanItem> = found
            .iter()
            .filter(|finding| PlanItem::plannable(finding))
            .filter_map(|finding| PlanItem::from_finding(finding, surveyor.describe(finding)))
            .collect();
        items.sort_by_key(|item| (rank(item.model.class), item.model.pid));
        let plan_id = to_hex(&entropy::bytes::<ID_BYTES>().map_err(PlanError::Entropy)?);
        let models: Vec<ModelFinding> = items.iter().map(|item| item.model.clone()).collect();
        let report = format!("{plan_id}{REPORT_SUFFIX}");
        let path = self.write_report(&report, &models).map_err(PlanError::Report)?;
        let stored = Stored {
            slots: items
                .into_iter()
                .map(|item| Slot {
                    item,
                    state: State::Available,
                })
                .collect(),
            expires_at: self.timer.now() + self.ttl,
            report: path,
        };
        self.lock().insert(plan_id.clone(), stored);
        Ok(Created {
            plan_id,
            expires_in: self.ttl,
            items: models,
            report,
        })
    }

    pub fn sweep(&self) {
        let mut plans = self.lock();
        self.sweep_locked(&mut plans);
    }

    pub fn alive(&self, plan_id: &str) -> bool {
        let mut plans = self.lock();
        self.sweep_locked(&mut plans);
        plans.contains_key(plan_id)
    }

    pub fn plans(&self) -> usize {
        let mut plans = self.lock();
        self.sweep_locked(&mut plans);
        plans.len()
    }

    pub fn claim(&self, plan_id: &str, ids: &[String]) -> Result<Vec<PlanItem>, ClaimError> {
        let mut plans = self.lock();
        self.sweep_locked(&mut plans);
        let plan = plans.get_mut(plan_id).ok_or(ClaimError::UnknownPlan)?;
        let mut taken: Vec<usize> = Vec::with_capacity(ids.len());
        for (index, id) in ids.iter().enumerate() {
            let slot = plan
                .slots
                .iter()
                .position(|slot| slot.item.model.item_id == *id)
                .ok_or(ClaimError::UnknownItem { index })?;
            if plan.slots[slot].state != State::Available || taken.contains(&slot) {
                return Err(ClaimError::Unavailable { index });
            }
            taken.push(slot);
        }
        for slot in &taken {
            plan.slots[*slot].state = State::Pending;
        }
        Ok(taken.iter().map(|slot| plan.slots[*slot].item.clone()).collect())
    }

    pub fn release(&self, plan_id: &str, ids: &[String]) {
        self.move_pending(plan_id, ids, State::Available);
    }

    pub fn finish(&self, plan_id: &str, ids: &[String]) {
        self.move_pending(plan_id, ids, State::Done);
    }

    fn move_pending(&self, plan_id: &str, ids: &[String], to: State) {
        let mut plans = self.lock();
        let Some(plan) = plans.get_mut(plan_id) else {
            return;
        };
        for slot in &mut plan.slots {
            if slot.state == State::Pending && ids.contains(&slot.item.model.item_id) {
                slot.state = to;
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Stored>> {
        self.plans.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn sweep_locked(&self, plans: &mut HashMap<String, Stored>) {
        let now = self.timer.now();
        plans.retain(|_, plan| {
            let expired = now >= plan.expires_at;
            if expired {
                let _ = fs::remove_file(&plan.report);
            }
            !expired
        });
    }

    fn write_report(&self, name: &str, models: &[ModelFinding]) -> io::Result<PathBuf> {
        let dir = self.data_dir.join(INSPECTION_DIR);
        safe_open::ensure_dir(&self.data_dir).map_err(refusal)?;
        safe_open::ensure_dir(&dir).map_err(refusal)?;
        let path = dir.join(name);
        let mut file = safe_open::open_file(&path, Access::Create).map_err(refusal)?;
        file.write_all(report_text(models, self.ttl).as_bytes())?;
        Ok(path)
    }
}

impl Drop for PlanStore {
    fn drop(&mut self) {
        for plan in self.lock().values() {
            let _ = fs::remove_file(&plan.report);
        }
    }
}

fn rank(class: Class) -> u8 {
    match class {
        Class::OwnedEnded => 0,
        _ => 1,
    }
}

fn refusal(err: SafeOpenError) -> io::Error {
    match err {
        SafeOpenError::Io(err) => err,
        refused => io::Error::new(io::ErrorKind::PermissionDenied, refused.to_string()),
    }
}

fn report_text(models: &[ModelFinding], ttl: Duration) -> String {
    let count = |class: Class| models.iter().filter(|model| model.class == class).count();
    let mut text = String::from("agentdust plan report\n");
    text.push_str("For reading only. This file cannot be passed to apply, and it holds no approval state.\n");
    let _ = writeln!(
        text,
        "made at unix ms {}, valid for {} seconds",
        clock::wall_ms(),
        ttl.as_secs()
    );
    let _ = writeln!(
        text,
        "items: {} (owned-ended {}, suspect {})",
        models.len(),
        count(Class::OwnedEnded),
        count(Class::Suspect)
    );
    for model in models {
        text.push('\n');
        text.push_str(&HumanDisplay::prompt(model));
        text.push('\n');
    }
    text
}

fn clear_stale_reports(dir: &Path) {
    if safe_open::check_dir(dir).is_err() {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let ours = entry.file_name().to_str().is_some_and(is_report_name);
        let regular = fs::symlink_metadata(entry.path()).is_ok_and(|meta| meta.file_type().is_file());
        if ours && regular {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn is_report_name(name: &str) -> bool {
    name.strip_suffix(REPORT_SUFFIX).is_some_and(|stem| {
        stem.len() == ID_BYTES * 2 && stem.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    })
}
