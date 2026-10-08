use std::io;
use std::sync::Arc;

use serde::Serialize;

use crate::apply::exec::Executor;
use crate::classifier::Finding;
use crate::finding::ModelFinding;
use crate::identity::ProcessIdentity;
use crate::plan::PlanItem;
use crate::survey::Surveyor;

pub const MAX_LISTENERS: usize = 16;

pub trait ListenerSource: Send + Sync {
    fn listeners(&self, port: u16) -> io::Result<Vec<ProcessIdentity>>;
}

pub struct PortSurveyor {
    pub port: u16,
    pub listeners: Arc<dyn ListenerSource>,
    pub processes: Arc<dyn Surveyor>,
}

impl Surveyor for PortSurveyor {
    fn survey(&self) -> io::Result<Vec<Finding>> {
        let before = self.listeners.listeners(self.port)?;
        if before.len() > MAX_LISTENERS {
            return Err(io::Error::other("listener limit reached"));
        }
        let findings = self.processes.survey()?;
        let after = self.listeners.listeners(self.port)?;
        if after.len() > MAX_LISTENERS {
            return Err(io::Error::other("listener limit reached"));
        }
        Ok(findings
            .into_iter()
            .filter(|finding| {
                before
                    .iter()
                    .any(|identity| matches(finding, identity) && after.contains(identity))
            })
            .collect())
    }

    fn describe(&self, finding: &Finding) -> ModelFinding {
        self.processes.describe(finding)
    }
}

fn matches(finding: &Finding, identity: &ProcessIdentity) -> bool {
    finding.identity.kernel == identity.kernel
        && finding.identity.exe_path.as_ref() == Some(&identity.evidence.exe_path)
}

#[derive(Serialize)]
pub struct Listener {
    pub pid: i32,
    pub finding: Option<ModelFinding>,
    pub result: String,
    pub reason: Option<String>,
}

#[derive(Serialize)]
pub struct Report {
    pub version: u32,
    pub port: u16,
    pub protocol: &'static str,
    pub visibility: &'static str,
    pub state: &'static str,
    pub listeners: Vec<Listener>,
    pub remaining_pids: Vec<i32>,
    pub error: Option<&'static str>,
}

pub fn run(port: u16, resolve: bool, surveyor: &PortSurveyor, executor: &Executor) -> io::Result<Report> {
    if port == 0 || port != surveyor.port {
        return Err(io::Error::other("port must be between 1 and 65535"));
    }
    let mut report = Report {
        version: 1,
        port,
        protocol: "tcp",
        visibility: "current_user_visible",
        state: "unavailable",
        listeners: Vec::new(),
        remaining_pids: Vec::new(),
        error: None,
    };
    let initial = match surveyor.listeners.listeners(port) {
        Ok(listeners) if listeners.len() <= MAX_LISTENERS => listeners,
        _ => {
            report.error = Some("listener_inventory_unavailable");
            return Ok(report);
        }
    };
    if initial.is_empty() {
        report.state = "no_visible_listener";
        return Ok(report);
    }
    let findings = match surveyor.survey() {
        Ok(findings) => findings,
        Err(_) => {
            report.error = Some("classification_unavailable");
            return Ok(report);
        }
    };
    for identity in initial {
        let finding = findings.iter().find(|finding| matches(finding, &identity));
        let model = finding.map(|finding| surveyor.describe(finding));
        let mut listener = Listener {
            pid: identity.kernel.pid,
            finding: model.clone(),
            result: "report_only".into(),
            reason: None,
        };
        if resolve {
            match finding
                .zip(model)
                .and_then(|(finding, model)| PlanItem::from_finding(finding, model))
            {
                Some(item) => match executor.execute_automatic(&format!("port-{port}"), &item) {
                    Ok(verdict) => {
                        listener.result = verdict.outcome.code().into();
                        listener.reason = verdict.reason.map(|reason| reason.code().into());
                    }
                    Err(reason) => {
                        listener.result = "skipped".into();
                        listener.reason = Some(reason);
                    }
                },
                None => {
                    listener.reason = Some("protected_or_unavailable".into());
                }
            }
        }
        report.listeners.push(listener);
    }
    match surveyor.listeners.listeners(port) {
        Ok(remaining) => {
            report.remaining_pids = remaining.iter().map(|identity| identity.kernel.pid).collect();
            report.state = if remaining.is_empty() {
                "no_visible_listener"
            } else {
                "listening"
            };
        }
        Err(_) => {
            report.error = Some("verification_unavailable");
        }
    }
    Ok(report)
}

pub fn parse_lsof(bytes: &[u8], port: u16) -> io::Result<Vec<i32>> {
    let invalid = || io::Error::other("invalid listener inventory");
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let mut pid = None;
    let mut descriptor = false;
    let mut named = true;
    let mut pids = Vec::new();
    for field in bytes.split(|byte| *byte == 0) {
        let field = field.strip_prefix(b"\n").unwrap_or(field);
        if field.is_empty() {
            continue;
        }
        let text = std::str::from_utf8(&field[1..]).map_err(|_| invalid())?;
        match field[0] {
            b'p' => {
                if !named {
                    return Err(invalid());
                }
                descriptor = false;
                named = false;
                pid = Some(
                    text.parse::<i32>()
                        .ok()
                        .filter(|pid| *pid > 0)
                        .ok_or_else(invalid)?,
                );
            }
            b'f' => {
                descriptor = true;
                if pid.is_none() || text.parse::<u32>().is_err() {
                    return Err(invalid());
                }
            }
            b'n' => {
                if !descriptor {
                    return Err(invalid());
                }
                named = true;
                if text
                    .rsplit_once(':')
                    .is_none_or(|(_, value)| value.parse::<u16>().ok() != Some(port))
                {
                    return Err(invalid());
                }
                let pid = pid.ok_or_else(invalid)?;
                if !pids.contains(&pid) {
                    pids.push(pid);
                }
                if pids.len() > MAX_LISTENERS {
                    return Err(invalid());
                }
            }
            _ => return Err(invalid()),
        }
    }
    if !named || pids.is_empty() || !bytes.ends_with(b"\0\n") {
        return Err(invalid());
    }
    Ok(pids)
}
