use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

use crate::apply::LOCKS_DIR;
use crate::apply::audit::{AUDIT_MAX_BYTES, AuditLog, Entry};
use crate::apply::lock::IdentityLock;
use crate::apply::signal::{SignalResult, Signaller};
use crate::apply::timer::Timer;
use crate::automatic::{self, PolicyGuard};
use crate::classifier::Finding;
use crate::code;
use crate::config::{self, ApplySwitch, Disabled};
use crate::plan::PlanItem;
use crate::provider::ProcessProvider;
use crate::revalidate::{Field, Revalidation, revalidate};
use crate::survey::Surveyor;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Terminated,
    Survivor,
    Gone,
    RevalidationFailed,
    HandledElsewhere,
    SignalFailed,
    AuditUnavailable,
    LockUnavailable,
    Disabled,
    Declined,
    Cancelled,
    TimedOut,
    Empty,
    WrongCode,
    Expired,
    PlanExpired,
}

impl Outcome {
    pub const fn code(self) -> &'static str {
        match self {
            Outcome::Terminated => "terminated",
            Outcome::Survivor => "survivor",
            Outcome::Gone => "gone",
            Outcome::RevalidationFailed => "revalidation_failed",
            Outcome::HandledElsewhere => "handled_elsewhere",
            Outcome::SignalFailed => "signal_failed",
            Outcome::AuditUnavailable => "audit_unavailable",
            Outcome::LockUnavailable => "lock_unavailable",
            Outcome::Disabled => "disabled",
            Outcome::Declined => "declined",
            Outcome::Cancelled => "cancelled",
            Outcome::TimedOut => "timed_out",
            Outcome::Empty => "empty",
            Outcome::WrongCode => "wrong_code",
            Outcome::Expired => "expired",
            Outcome::PlanExpired => "plan_expired",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    ClassChanged,
    OwnershipChanged,
    IdentityChanged,
    PathChanged,
    Unreadable,
    SurveyFailed,
}

impl Reason {
    pub const fn code(self) -> &'static str {
        match self {
            Reason::ClassChanged => "class_changed",
            Reason::OwnershipChanged => "ownership_changed",
            Reason::IdentityChanged => "identity_changed",
            Reason::PathChanged => "path_changed",
            Reason::Unreadable => "unreadable",
            Reason::SurveyFailed => "survey_failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Verdict {
    pub outcome: Outcome,
    pub reason: Option<Reason>,
}

impl Verdict {
    pub const fn of(outcome: Outcome) -> Self {
        Self {
            outcome,
            reason: None,
        }
    }

    pub const fn failed(reason: Reason) -> Self {
        Self {
            outcome: Outcome::RevalidationFailed,
            reason: Some(reason),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub term_wait: Duration,
    pub poll_interval: Duration,
    pub plan_ttl: Duration,
    pub code_ttl: Duration,
    pub audit_max_bytes: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            term_wait: Duration::from_secs(5),
            poll_interval: Duration::from_millis(50),
            plan_ttl: Duration::from_secs(600),
            code_ttl: code::TTL,
            audit_max_bytes: AUDIT_MAX_BYTES,
        }
    }
}

pub struct Deps {
    pub data_dir: PathBuf,
    pub surveyor: Arc<dyn Surveyor>,
    pub provider: Box<dyn ProcessProvider + Send + Sync>,
    pub signaller: Box<dyn Signaller>,
    pub timer: Arc<dyn Timer>,
}

pub struct Executor {
    deps: Deps,
    settings: Settings,
    audit: AuditLog,
}

impl Executor {
    pub fn new(deps: Deps, settings: Settings) -> Self {
        let audit = AuditLog::new(&deps.data_dir, settings.audit_max_bytes);
        Self {
            deps,
            settings,
            audit,
        }
    }

    pub fn surveyor(&self) -> &dyn Surveyor {
        self.deps.surveyor.as_ref()
    }

    pub fn data_dir(&self) -> &std::path::Path {
        &self.deps.data_dir
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn enabled(&self) -> Result<(), Disabled> {
        match config::apply_switch(&self.deps.data_dir) {
            ApplySwitch::Enabled => Ok(()),
            ApplySwitch::Disabled(reason) => Err(reason),
        }
    }

    pub fn note(&self, plan_id: &str, item: &PlanItem, verdict: Verdict) {
        if verdict.outcome != Outcome::Disabled {
            self.record(plan_id, item, verdict);
        }
    }

    pub fn execute(&self, plan_id: &str, item: &PlanItem) -> Verdict {
        let guard = match automatic::manual_guard(&self.deps.data_dir) {
            Ok(guard) => guard,
            Err(_) => return self.record(plan_id, item, Verdict::of(Outcome::Disabled)),
        };
        if guard
            .as_ref()
            .is_some_and(|guard| guard.policy.keep.contains(&item.identity.kernel))
        {
            return self.record(plan_id, item, Verdict::of(Outcome::Disabled));
        }
        self.execute_with_policy(plan_id, item, None)
    }

    pub fn execute_automatic(&self, plan_id: &str, item: &PlanItem) -> Result<Verdict, String> {
        let guard = PolicyGuard::acquire(&self.deps.data_dir).map_err(|_| "policy_unavailable".to_owned())?;
        if let Err(error) = guard.authorize(item) {
            let message = error.to_string();
            let reason = match message.as_str() {
                "kept"
                | "approval_required"
                | "journal_unavailable"
                | "ownership_unavailable"
                | "shared_ownership"
                | "project_not_enabled" => message,
                _ => "policy_unavailable".to_owned(),
            };
            let _ = self.audit.append(&Entry::result(
                plan_id,
                &item.model,
                &item.identity.kernel,
                "skipped",
                Some(&reason),
            ));
            return Err(reason);
        }
        Ok(self.execute_with_policy(plan_id, item, Some(&guard)))
    }

    fn execute_with_policy(
        &self,
        plan_id: &str,
        item: &PlanItem,
        automatic: Option<&PolicyGuard>,
    ) -> Verdict {
        if matches!(
            config::apply_switch(&self.deps.data_dir),
            ApplySwitch::Disabled(_)
        ) {
            return Verdict::of(Outcome::Disabled);
        }
        let lock = match IdentityLock::try_acquire(&self.deps.data_dir.join(LOCKS_DIR), &item.model.item_id) {
            Ok(Some(lock)) => lock,
            Ok(None) => return self.record(plan_id, item, Verdict::of(Outcome::HandledElsewhere)),
            Err(_) => return self.record(plan_id, item, Verdict::of(Outcome::LockUnavailable)),
        };
        let verdict = self.locked(plan_id, item, automatic);
        let verdict = self.record(plan_id, item, verdict);
        drop(lock);
        verdict
    }

    fn record(&self, plan_id: &str, item: &PlanItem, verdict: Verdict) -> Verdict {
        let entry = Entry::result(
            plan_id,
            &item.model,
            &item.identity.kernel,
            verdict.outcome.code(),
            verdict.reason.map(Reason::code),
        );
        let _ = self.audit.append(&entry);
        verdict
    }

    fn locked(&self, plan_id: &str, item: &PlanItem, automatic: Option<&PolicyGuard>) -> Verdict {
        let fresh = match self.deps.surveyor.survey() {
            Ok(fresh) => fresh,
            Err(_) => return Verdict::failed(Reason::SurveyFailed),
        };
        if let Some(verdict) = reclassify(item, &fresh) {
            return verdict;
        }
        if matches!(
            config::apply_switch(&self.deps.data_dir),
            ApplySwitch::Disabled(_)
        ) {
            return Verdict::of(Outcome::Disabled);
        }
        if let Some(guard) = automatic {
            if guard.authorize(item).is_err() {
                return Verdict::of(Outcome::Disabled);
            }
            match guard.claim_attempt(item) {
                Ok(true) => {}
                Ok(false) => return Verdict::of(Outcome::HandledElsewhere),
                Err(_) => return Verdict::of(Outcome::AuditUnavailable),
            }
        }
        let attempt = Entry::attempt(plan_id, &item.model, &item.identity.kernel);
        if self.audit.append(&attempt).is_err() {
            return Verdict::of(Outcome::AuditUnavailable);
        }
        self.signal_and_wait(item)
    }

    fn signal_and_wait(&self, item: &PlanItem) -> Verdict {
        let sent = match revalidate(&item.identity, self.deps.provider.as_ref()) {
            Revalidation::Match => self.deps.signaller.sigterm(item.identity.kernel.pid),
            Revalidation::Gone => return Verdict::of(Outcome::Gone),
            Revalidation::Changed(Field::ExePath) => return Verdict::failed(Reason::PathChanged),
            Revalidation::Changed(_) => return Verdict::failed(Reason::IdentityChanged),
            Revalidation::Unreadable => return Verdict::failed(Reason::Unreadable),
        };
        match sent {
            SignalResult::Delivered => self.wait_for_exit(item),
            SignalResult::NoSuchProcess => Verdict::of(Outcome::Gone),
            SignalResult::Refused | SignalResult::Failed(_) => Verdict::of(Outcome::SignalFailed),
        }
    }

    fn wait_for_exit(&self, item: &PlanItem) -> Verdict {
        let started = self.deps.timer.now();
        loop {
            let ended = matches!(
                revalidate(&item.identity, self.deps.provider.as_ref()),
                Revalidation::Gone
                    | Revalidation::Changed(Field::StartTime | Field::BootSession | Field::Pid)
            );
            if ended {
                return Verdict::of(Outcome::Terminated);
            }
            if self.deps.timer.now().saturating_sub(started) >= self.settings.term_wait {
                return Verdict::of(Outcome::Survivor);
            }
            self.deps.timer.sleep(self.settings.poll_interval);
        }
    }
}

fn reclassify(item: &PlanItem, fresh: &[Finding]) -> Option<Verdict> {
    let kernel = &item.identity.kernel;
    let Some(found) = fresh.iter().find(|found| found.identity.kernel == *kernel) else {
        let reused = fresh.iter().any(|found| found.identity.kernel.pid == kernel.pid);
        return Some(if reused {
            Verdict::failed(Reason::IdentityChanged)
        } else {
            Verdict::of(Outcome::Gone)
        });
    };
    if found.class != item.model.class {
        return Some(Verdict::failed(Reason::ClassChanged));
    }
    if found.attribution_owners != item.attribution_owners {
        return Some(Verdict::failed(Reason::OwnershipChanged));
    }
    match found.identity.exe_path.as_deref() {
        None => Some(Verdict::failed(Reason::Unreadable)),
        Some(path) if path.as_os_str() != item.identity.evidence.exe_path.as_os_str() => {
            Some(Verdict::failed(Reason::PathChanged))
        }
        Some(_) => None,
    }
}
