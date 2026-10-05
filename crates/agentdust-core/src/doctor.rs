use std::fmt::Write;
use std::io;
use std::path::Path;

use serde::Serialize;

use crate::class::Class;
use crate::classifier::{Evidence, Finding, Policy, Provenance, classify};
use crate::cwd::{CwdRelation, RelationContext, relate};
use crate::finding::{HumanDisplay, LiveText, ModelFinding};
use crate::identity::KernelIdentity;
use crate::inventory::{
    Clock, IDLE_SAMPLE_GAP, Launchd, LaunchdSource, LiveDetails, ProcessSource, Snapshot, take,
};
use crate::journal::volume::VolumeProbe;
use crate::journal::{Journal, JournalError};
use crate::sanitize::escape;
use crate::session::{self, LivenessProbe, Scope, State};

pub const REPORT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    UnsupportedVersion,
    UnsupportedFilesystem(String),
    SecretUnavailable,
    JournalRefused,
    JournalUnreadable,
}

impl Unavailable {
    pub fn code(&self) -> &'static str {
        match self {
            Unavailable::UnsupportedVersion => "unsupported_version",
            Unavailable::UnsupportedFilesystem(_) => "unsupported_filesystem",
            Unavailable::SecretUnavailable => "secret_unavailable",
            Unavailable::JournalRefused => "journal_refused",
            Unavailable::JournalUnreadable => "journal_unreadable",
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Unavailable::UnsupportedVersion => {
                "the journal has a schema version this build does not support".to_owned()
            }
            Unavailable::UnsupportedFilesystem(name) => {
                format!(
                    "the data directory is on a volume that is not local APFS ({})",
                    escape(name)
                )
            }
            Unavailable::SecretUnavailable => "the install secret is missing or unusable".to_owned(),
            Unavailable::JournalRefused => {
                "the data directory or the journal failed a safety check".to_owned()
            }
            Unavailable::JournalUnreadable => "the journal could not be read".to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sessions {
    pub unavailable: Option<Unavailable>,
    pub scopes: Vec<Scope>,
    pub records: usize,
    pub skipped_lines: usize,
}

impl Sessions {
    fn without(reason: Unavailable, records: usize, skipped_lines: usize) -> Self {
        Self {
            unavailable: Some(reason),
            scopes: Vec::new(),
            records,
            skipped_lines,
        }
    }

    pub fn provenance(&self) -> Provenance<'_> {
        match self.unavailable {
            None => Provenance::Available(&self.scopes),
            Some(_) => Provenance::Unavailable,
        }
    }
}

pub fn load_sessions(
    dir: &Path,
    volume: &dyn VolumeProbe,
    secret_available: bool,
    probe: &dyn LivenessProbe,
) -> Sessions {
    let report = match Journal::with_volume(dir, volume).read() {
        Ok(report) => report,
        Err(JournalError::UnsupportedFilesystem(facts)) => {
            return Sessions::without(Unavailable::UnsupportedFilesystem(facts.name), 0, 0);
        }
        Err(JournalError::Refused(_)) => return Sessions::without(Unavailable::JournalRefused, 0, 0),
        Err(_) => return Sessions::without(Unavailable::JournalUnreadable, 0, 0),
    };
    let (records, skipped) = (report.records.len(), report.skipped_lines());
    let unavailable = if report.unsupported_version {
        Some(Unavailable::UnsupportedVersion)
    } else if let Some(facts) = report.filesystem.as_ref().filter(|facts| !facts.supported) {
        Some(Unavailable::UnsupportedFilesystem(facts.name.clone()))
    } else if !secret_available {
        Some(Unavailable::SecretUnavailable)
    } else {
        None
    };
    match unavailable {
        Some(reason) => Sessions::without(reason, records, skipped),
        None => Sessions {
            unavailable: None,
            scopes: session::scopes(&report.records, probe),
            records,
            skipped_lines: skipped,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Counts {
    pub managed: usize,
    #[serde(rename = "owned-live")]
    pub owned_live: usize,
    #[serde(rename = "owned-ended")]
    pub owned_ended: usize,
    #[serde(rename = "likely-owned")]
    pub likely_owned: usize,
    pub suspect: usize,
    pub unknown: usize,
}

impl Counts {
    fn add(&mut self, class: Class) {
        let slot = match class {
            Class::Managed => &mut self.managed,
            Class::OwnedLive => &mut self.owned_live,
            Class::OwnedEnded => &mut self.owned_ended,
            Class::LikelyOwned => &mut self.likely_owned,
            Class::Suspect => &mut self.suspect,
            Class::Unknown => &mut self.unknown,
        };
        *slot += 1;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct SessionSummary {
    pub records: usize,
    pub skipped_lines: usize,
    pub active: usize,
    pub ended: usize,
    pub unknown: usize,
    pub degraded: usize,
}

impl SessionSummary {
    fn of(sessions: &Sessions) -> Self {
        let mut summary = Self {
            records: sessions.records,
            skipped_lines: sessions.skipped_lines,
            ..Self::default()
        };
        for scope in &sessions.scopes {
            match scope.state {
                State::Active => summary.active += 1,
                State::Ended => summary.ended += 1,
                State::Unknown => summary.unknown += 1,
            }
            if scope.degraded() {
                summary.degraded += 1;
            }
        }
        summary
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnosis {
    pub processes_seen: usize,
    pub counts: Counts,
    pub owned: Option<Unavailable>,
    pub launchd_available: bool,
    pub sessions: SessionSummary,
    pub unexplained_tags: usize,
    pub findings: Vec<ModelFinding>,
    kernels: Vec<KernelIdentity>,
}

#[derive(Serialize)]
struct OwnedView {
    available: bool,
    reason: Option<&'static str>,
}

#[derive(Serialize)]
struct LaunchdView {
    available: bool,
}

#[derive(Serialize)]
struct JsonView<'a> {
    version: u32,
    owned_classes: OwnedView,
    launchd: LaunchdView,
    sessions: &'a SessionSummary,
    unexplained_tags: usize,
    counts: &'a Counts,
    findings: &'a [ModelFinding],
}

fn list_rank(class: Class) -> Option<u8> {
    match class {
        Class::OwnedEnded => Some(0),
        Class::Suspect => Some(1),
        Class::LikelyOwned => Some(2),
        Class::OwnedLive => Some(3),
        Class::Managed | Class::Unknown => None,
    }
}

fn is_unexplained(finding: &Finding) -> bool {
    finding.class == Class::Unknown
        && finding.evidence.iter().any(|evidence| {
            matches!(
                evidence,
                Evidence::TagUnmatched | Evidence::TagUnverifiable | Evidence::TagDegraded
            )
        })
}

pub fn diagnose(
    snapshot: &Snapshot,
    sessions: &Sessions,
    policy: &Policy,
    live: &dyn LiveDetails,
    relation: &RelationContext,
) -> Diagnosis {
    let classified = classify(snapshot, &sessions.provenance(), policy);
    let mut counts = Counts::default();
    let mut unexplained_tags = 0;
    let mut listed: Vec<(u8, &Finding)> = Vec::new();
    for finding in &classified {
        counts.add(finding.class);
        if is_unexplained(finding) {
            unexplained_tags += 1;
        }
        if let Some(rank) = list_rank(finding.class) {
            listed.push((rank, finding));
        }
    }
    listed.sort_by_key(|(rank, finding)| (*rank, finding.identity.kernel.pid));
    let findings = listed
        .iter()
        .map(|(_, finding)| {
            let cwd_relation = live
                .cwd(&finding.identity.kernel)
                .map_or(CwdRelation::Other, |cwd| relate(&cwd, relation));
            ModelFinding::new(finding, cwd_relation)
        })
        .collect();
    Diagnosis {
        processes_seen: classified.len(),
        counts,
        owned: sessions.unavailable.clone(),
        launchd_available: matches!(snapshot.launchd, Launchd::Known(_)),
        sessions: SessionSummary::of(sessions),
        unexplained_tags,
        findings,
        kernels: listed
            .iter()
            .map(|(_, finding)| finding.identity.kernel.clone())
            .collect(),
    }
}

impl Diagnosis {
    pub fn to_json(&self) -> String {
        let view = JsonView {
            version: REPORT_VERSION,
            owned_classes: OwnedView {
                available: self.owned.is_none(),
                reason: self.owned.as_ref().map(Unavailable::code),
            },
            launchd: LaunchdView {
                available: self.launchd_available,
            },
            sessions: &self.sessions,
            unexplained_tags: self.unexplained_tags,
            counts: &self.counts,
            findings: &self.findings,
        };
        serde_json::to_string(&view).expect("a report of plain fields always serialises")
    }

    pub fn render(&self, live: &dyn LiveDetails) -> String {
        let mut out = String::from("agentdust doctor\n");
        let _ = writeln!(out, "processes: {} seen", self.processes_seen);
        let _ = writeln!(out, "journal: {}", self.journal_line());
        let _ = writeln!(
            out,
            "launchd: {}",
            if self.launchd_available {
                "PID list read"
            } else {
                "PID list unavailable, so no process can be offered for cleanup"
            }
        );
        let counts = &self.counts;
        let _ = writeln!(
            out,
            "classes: managed {}, owned-live {}, owned-ended {}, likely-owned {}, suspect {}, unknown {}",
            counts.managed,
            counts.owned_live,
            counts.owned_ended,
            counts.likely_owned,
            counts.suspect,
            counts.unknown
        );
        if self.unexplained_tags > 0 {
            let _ = writeln!(out, "tags without a usable session: {}", self.unexplained_tags);
        }
        if self.findings.is_empty() {
            out.push_str("findings: none\n");
            return out;
        }
        let _ = writeln!(out, "findings: {}", self.findings.len());
        for (finding, kernel) in self.findings.iter().zip(&self.kernels) {
            let command = live.command(kernel);
            let cwd = live.cwd(kernel);
            let text = LiveText::new(command.as_deref(), cwd.as_deref());
            out.push('\n');
            out.push_str(&HumanDisplay::terminal(finding, &text));
            out.push('\n');
        }
        out
    }

    fn journal_line(&self) -> String {
        if let Some(reason) = &self.owned {
            return format!("owned classes are unavailable: {}", reason.describe());
        }
        let sessions = &self.sessions;
        let total = sessions.active + sessions.ended + sessions.unknown;
        let mut line = format!(
            "{} records, {} sessions (active {}, ended {}, unknown {}), {} without a known agent",
            sessions.records, total, sessions.active, sessions.ended, sessions.unknown, sessions.degraded
        );
        if sessions.skipped_lines > 0 {
            let _ = write!(line, ", {} lines skipped", sessions.skipped_lines);
        }
        line
    }
}

pub struct Components<'a> {
    pub data_dir: &'a Path,
    pub volume: &'a dyn VolumeProbe,
    pub secret_available: bool,
    pub processes: &'a dyn ProcessSource,
    pub launchd: &'a dyn LaunchdSource,
    pub clock: &'a dyn Clock,
    pub liveness: &'a dyn LivenessProbe,
    pub live: &'a dyn LiveDetails,
    pub relation: &'a RelationContext,
    pub policy: Policy,
}

pub fn run(components: &Components) -> io::Result<Diagnosis> {
    let snapshot = take(
        components.processes,
        components.launchd,
        components.clock,
        IDLE_SAMPLE_GAP,
    )?;
    let sessions = load_sessions(
        components.data_dir,
        components.volume,
        components.secret_available,
        components.liveness,
    );
    Ok(diagnose(
        &snapshot,
        &sessions,
        &components.policy,
        components.live,
        components.relation,
    ))
}
