use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::apply::lock::IdentityLock;
use crate::atomic::write_atomic;
use crate::class::Class;
use crate::config::{self, ApplySwitch};
use crate::digest::sha256_hex;
use crate::identity::KernelIdentity;
use crate::journal::{CwdKey, Journal, Kind};
use crate::plan::PlanItem;
use crate::safe_open::{self, Access};
use crate::secret;

pub const DIRECTORY: &str = "automatic";
const MAX_BYTES: u64 = 64 * 1024;
const POLICY_FILE: &str = "policy.json";
const WORKER_FILE: &str = "worker.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub version: u32,
    pub enabled: bool,
    pub projects: Vec<CwdKey>,
    pub keep: Vec<KernelIdentity>,
}

pub struct PolicyGuard {
    dir: PathBuf,
    pub policy: Policy,
    _lock: IdentityLock,
}

#[derive(Debug, Deserialize, Serialize)]
struct WorkerRecord {
    #[serde(flatten)]
    identity: KernelIdentity,
    worker_version: String,
}

struct WorkerInfo {
    version: Option<String>,
}

fn error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

pub fn read(dir: &Path) -> io::Result<Policy> {
    let root = dir.join(DIRECTORY);
    match fs::symlink_metadata(&root) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Policy::default()),
        Err(err) => return Err(err),
        Ok(_) => safe_open::check_dir(&root).map_err(error)?,
    }
    let file = match safe_open::open_file(&root.join(POLICY_FILE), Access::Read) {
        Ok(file) => file,
        Err(safe_open::SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(Policy::default());
        }
        Err(err) => return Err(error(err)),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(error("automatic policy is too large"));
    }
    let policy: Policy = serde_json::from_slice(&bytes).map_err(error)?;
    if policy.version != 1 || policy.projects.len() > 128 || policy.keep.len() > 256 {
        return Err(error("unsupported automatic policy"));
    }
    Ok(policy)
}

impl PolicyGuard {
    pub fn acquire(dir: &Path) -> io::Result<Self> {
        safe_open::check_dir(dir).map_err(error)?;
        let root = dir.join(DIRECTORY);
        safe_open::ensure_dir(&root).map_err(error)?;
        for _ in 0..400 {
            if let Some(lock) = IdentityLock::try_acquire(&root.join("locks"), "policy").map_err(error)? {
                return Ok(Self {
                    dir: dir.to_path_buf(),
                    policy: read(dir)?,
                    _lock: lock,
                });
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err(error("automatic policy is busy"))
    }

    pub fn write(&mut self) -> io::Result<()> {
        self.policy.version = 1;
        let bytes = serde_json::to_vec(&self.policy).map_err(error)?;
        if bytes.len() as u64 > MAX_BYTES || self.policy.projects.len() > 128 || self.policy.keep.len() > 256
        {
            return Err(error("automatic policy limit reached"));
        }
        write_atomic(&self.dir.join(DIRECTORY).join(POLICY_FILE), &bytes, 0o600).map_err(error)
    }

    pub fn authorize(&self, item: &PlanItem) -> io::Result<&'static str> {
        if !self.policy.enabled || !matches!(config::apply_switch(&self.dir), ApplySwitch::Enabled) {
            return Err(error("automatic cleanup is disabled"));
        }
        if self.policy.keep.contains(&item.identity.kernel) {
            return Err(error("kept"));
        }
        if item.model.class != Class::OwnedEnded {
            return Err(error("approval_required"));
        }
        secret::load_existing(&self.dir).map_err(error)?;
        let journal = Journal::new(&self.dir).read().map_err(error)?;
        if journal.unsupported_version
            || journal.skipped_lines() != 0
            || journal.filesystem.as_ref().is_none_or(|facts| !facts.supported)
        {
            return Err(error("journal_unavailable"));
        }
        let owners = item
            .attribution_owners
            .as_ref()
            .filter(|owners| !owners.is_empty())
            .ok_or_else(|| error("ownership_unavailable"))?;
        if owners.iter().any(|owner| {
            !matches!(
                owner.agent,
                crate::journal::Agent::Claude | crate::journal::Agent::Codex
            ) || owner.agent != owners[0].agent
                || owner.session_id != owners[0].session_id
                || owner.identity.boot_session_uuid != owners[0].identity.boot_session_uuid
        }) {
            return Err(error("shared_ownership"));
        }
        for owner in owners {
            let matching = |record: &&crate::journal::Record| {
                record.agent == owner.agent
                    && record.session_id == owner.session_id
                    && record.boot == owner.identity.boot_session_uuid
            };
            let records: Vec<_> = journal.records.iter().filter(matching).collect();
            if !records.iter().any(|record| {
                record
                    .agent_identity
                    .as_ref()
                    .is_some_and(|identity| identity.kernel(&record.boot) == owner.identity)
            }) {
                return Err(error("ownership_unavailable"));
            }
            let starts: Vec<_> = records
                .iter()
                .filter(|record| record.kind == Kind::SessionStart && record.subagent_id.is_none())
                .collect();
            if starts.is_empty()
                || starts.iter().any(|record| {
                    record
                        .cwd_key
                        .as_ref()
                        .is_none_or(|key| !self.policy.projects.contains(key))
                })
            {
                return Err(error("project_not_enabled"));
            }
        }
        Ok("automatic_project_policy")
    }

    pub fn claim_attempt(&self, item: &PlanItem) -> io::Result<bool> {
        let receipts = self.dir.join(DIRECTORY).join("attempts");
        safe_open::ensure_dir(&receipts).map_err(error)?;
        let kernel = serde_json::to_vec(&item.identity.kernel).map_err(error)?;
        let path = receipts.join(sha256_hex(&kernel));
        match safe_open::open_file(&path, Access::Read) {
            Ok(_) => return Ok(false),
            Err(safe_open::SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(error(err)),
        }
        if fs::read_dir(&receipts)?.take(16_385).count() >= 16_384 {
            return Err(error("automatic attempt storage is full"));
        }
        let mut file = safe_open::open_file(&path, Access::Create).map_err(error)?;
        file.write_all(&kernel)?;
        file.sync_all()?;
        fs::File::open(&receipts)?.sync_all()?;
        Ok(true)
    }
}

pub fn manual_guard(dir: &Path) -> io::Result<PolicyGuard> {
    PolicyGuard::acquire(dir)
}

pub fn write_report(dir: &Path, report: &serde_json::Value) -> io::Result<()> {
    safe_open::check_dir(&dir.join(DIRECTORY)).map_err(error)?;
    let bytes = serde_json::to_vec(report).map_err(error)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(error("automatic report is too large"));
    }
    write_atomic(&dir.join(DIRECTORY).join("results.json"), &bytes, 0o600).map_err(error)
}

pub fn prune_attempts(dir: &Path, probe: &dyn crate::session::LivenessProbe) -> io::Result<()> {
    let receipts = dir.join(DIRECTORY).join("attempts");
    match safe_open::check_dir(&receipts) {
        Ok(()) => {}
        Err(safe_open::SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(error(err)),
    }
    for entry in fs::read_dir(&receipts)?.take(16_384) {
        let entry = entry?;
        let file = safe_open::open_file(&entry.path(), Access::Read).map_err(error)?;
        let mut bytes = Vec::new();
        file.take(1025).read_to_end(&mut bytes)?;
        let Ok(identity) = serde_json::from_slice::<KernelIdentity>(&bytes) else {
            continue;
        };
        if entry.file_name().to_str() == Some(&sha256_hex(&bytes))
            && probe.probe(&identity) == crate::session::Liveness::Gone
        {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

pub fn status(dir: &Path) -> io::Result<serde_json::Value> {
    let policy = read(dir)?;
    let worker = worker_info(dir);
    let worker_running = worker.is_some();
    let worker_version = worker.as_ref().and_then(|worker| worker.version.as_deref());
    let mut report = serde_json::Value::Null;
    if fs::symlink_metadata(dir.join(DIRECTORY)).is_ok() {
        match safe_open::open_file(&dir.join(DIRECTORY).join("results.json"), Access::Read) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
                if bytes.len() as u64 > MAX_BYTES {
                    return Err(error("automatic report is too large"));
                }
                report = serde_json::from_slice(&bytes).map_err(error)?;
            }
            Err(safe_open::SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(error(err)),
        }
    }
    Ok(serde_json::json!({
        "version": 1,
        "enabled": policy.enabled,
        "apply_enabled": matches!(config::apply_switch(dir), ApplySwitch::Enabled),
        "project_count": policy.projects.len(),
        "keep_count": policy.keep.len(),
        "worker_running": worker_running,
        "worker_version": worker_version,
        "worker_version_status": worker_version_status(worker_running, worker_version),
        "last_report": report,
    }))
}

pub fn write_worker_identity(dir: &Path, identity: &KernelIdentity) -> io::Result<()> {
    safe_open::check_dir(&dir.join(DIRECTORY)).map_err(error)?;
    write_atomic(
        &dir.join(DIRECTORY).join(WORKER_FILE),
        &serde_json::to_vec(&WorkerRecord {
            identity: identity.clone(),
            worker_version: env!("CARGO_PKG_VERSION").to_owned(),
        })
        .map_err(error)?,
        0o600,
    )
    .map_err(error)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn worker_info(dir: &Path) -> Option<WorkerInfo> {
    use crate::session::{Liveness, LivenessProbe, ProviderLiveness};
    let file = safe_open::open_file(&dir.join(DIRECTORY).join(WORKER_FILE), Access::Read).ok()?;
    let mut bytes = Vec::new();
    file.take(1025).read_to_end(&mut bytes).ok()?;
    let (identity, version) = match serde_json::from_slice::<WorkerRecord>(&bytes) {
        Ok(record) => (record.identity, Some(record.worker_version)),
        Err(_) => (serde_json::from_slice::<KernelIdentity>(&bytes).ok()?, None),
    };
    let provider = crate::platform::Provider::new().ok()?;
    (ProviderLiveness(&provider).probe(&identity) == Liveness::Alive).then_some(WorkerInfo { version })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn worker_info(_dir: &Path) -> Option<WorkerInfo> {
    None
}

fn worker_version_status(running: bool, version: Option<&str>) -> &'static str {
    if !running {
        "stopped"
    } else if version.is_none() {
        "unknown"
    } else if version == Some(env!("CARGO_PKG_VERSION")) {
        "same"
    } else {
        "different"
    }
}
