use std::path::Path;

use serde::Serialize;

use crate::class::Class;
use crate::classifier::{Evidence, Finding};
use crate::cwd::CwdRelation;
use crate::identity::KernelIdentity;
use crate::journal::MAX_EXE_BASE_LEN;
use crate::sanitize::{escape, terminal_command, terminal_path};

const UNLISTED: &str = "(unlisted)";
const UNREADABLE: &str = "(unreadable)";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelFinding {
    pub item_id: String,
    pub class: Class,
    pub exe_base: Option<String>,
    pub pid: i32,
    pub age_secs: u64,
    pub evidence: Vec<Evidence>,
    pub cwd_relation: CwdRelation,
}

impl ModelFinding {
    pub fn new(finding: &Finding, cwd_relation: CwdRelation) -> Self {
        let kernel = &finding.identity.kernel;
        Self {
            item_id: item_id(kernel),
            class: finding.class,
            exe_base: exe_base_for_model(finding.identity.exe_path.as_deref()),
            pid: kernel.pid,
            age_secs: finding.age_us / 1_000_000,
            evidence: finding.evidence.clone(),
            cwd_relation,
        }
    }
}

pub fn item_id(kernel: &KernelIdentity) -> String {
    format!("p{}-{:x}", kernel.pid, kernel.start_time_us)
}

pub fn exe_base_for_model(path: Option<&Path>) -> Option<String> {
    let name = path?.file_name()?.to_str()?;
    let plain = name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'@' | b'-'));
    (!name.is_empty() && name.len() <= MAX_EXE_BASE_LEN && plain).then(|| name.to_owned())
}

pub fn format_age(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h {:02}m", secs / 3600, secs % 3600 / 60),
        _ => format!("{}d {}h", secs / 86_400, secs % 86_400 / 3600),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveText {
    pub command: Option<String>,
    pub directory: Option<String>,
}

impl LiveText {
    pub fn new(args: Option<&[Vec<u8>]>, cwd: Option<&Path>) -> Self {
        Self {
            command: args.map(terminal_command),
            directory: cwd.map(terminal_path),
        }
    }
}

pub struct HumanDisplay;

impl HumanDisplay {
    pub fn prompt(finding: &ModelFinding) -> String {
        format!("{}  evidence {}", header(finding), evidence(finding))
    }

    pub fn terminal(finding: &ModelFinding, live: &LiveText) -> String {
        format!(
            "{}\n  evidence: {}\n  command: {}\n  directory: {}",
            header(finding),
            evidence(finding),
            live.command.as_deref().unwrap_or(UNREADABLE),
            live.directory.as_deref().unwrap_or(UNREADABLE),
        )
    }
}

fn header(finding: &ModelFinding) -> String {
    format!(
        "{}  {}  pid {}  {}  age {}  cwd {}",
        escape(&finding.item_id),
        finding.class,
        finding.pid,
        finding
            .exe_base
            .as_deref()
            .map_or_else(|| UNLISTED.to_owned(), escape),
        format_age(finding.age_secs),
        finding.cwd_relation.as_str(),
    )
}

fn evidence(finding: &ModelFinding) -> String {
    if finding.evidence.is_empty() {
        return "none".to_owned();
    }
    finding
        .evidence
        .iter()
        .map(|kind| kind.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}
