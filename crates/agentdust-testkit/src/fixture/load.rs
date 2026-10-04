use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use agentdust_core::class::Class;
use agentdust_core::journal::{Agent, Kind};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

use super::{Fixture, FixtureEvent, FixtureProcess, Label, SCHEMA_VERSION};
use crate::spec::ProcSpec;

const MAX_ROLE_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub file: PathBuf,
    pub role: Option<String>,
    pub kind: ProblemKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProblemKind {
    #[error("cannot be read: {0}")]
    Read(String),
    #[error("not valid JSON: {0}")]
    Syntax(String),
    #[error("schema_version {found} is not supported, this build reads {SCHEMA_VERSION}")]
    UnsupportedVersion { found: String },
    #[error("{0}")]
    Shape(String),
    #[error("name {name:?} does not match the file name {stem:?}")]
    NameMismatch { name: String, stem: String },
    #[error("a role is 1 to 64 characters of a to z, 0 to 9 and _")]
    BadRole,
    #[error("role is listed {count} times")]
    DuplicateRole { count: usize },
    #[error("parent_role {parent:?} is not a role of this fixture")]
    MissingParent { parent: String },
    #[error("parent_role forms a cycle: {}", .path.join(" -> "))]
    Cycle { path: Vec<String> },
    #[error("expected class {found} contradicts {label}, which is {expected}")]
    ClassContradictsLabel {
        label: Label,
        found: Class,
        expected: Class,
    },
    #[error("must_never_signal is {found} but {label} requires {expected}")]
    MustNeverSignalContradictsLabel {
        label: Label,
        found: bool,
        expected: bool,
    },
    #[error("flags do not parse: {0}")]
    BadFlags(String),
    #[error("flag {0} is set by the harness and cannot be listed")]
    HarnessOwnedFlag(&'static str),
    #[error("record_only is only for true_managed processes, this one is {label}")]
    RecordOnlyNotAllowed { label: Label },
    #[error("journal[{index}]: a {kind:?} event cannot list roles")]
    EventRolesNotAllowed { index: usize, kind: Kind },
    #[error("journal[{index}]: role {role:?} is not a role of this fixture")]
    EventUnknownRole { index: usize, role: String },
    #[error("journal[{index}]: a {kind:?} event needs a tool_use_id")]
    EventNeedsToolUseId { index: usize, kind: Kind },
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.file.display())?;
        if let Some(role) = &self.role {
            write!(f, "role {role}: ")?;
        }
        write!(f, "{}", self.kind)
    }
}

impl std::error::Error for Problem {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub path: PathBuf,
    pub fixture: Fixture,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    schema_version: u32,
    name: String,
    description: String,
    agent: Agent,
    processes: Vec<Value>,
    journal: Vec<Value>,
    session_ended: bool,
}

struct Collector<'a> {
    file: &'a Path,
    problems: Vec<Problem>,
}

impl Collector<'_> {
    fn add(&mut self, role: Option<&str>, kind: ProblemKind) {
        self.problems.push(Problem {
            file: self.file.to_owned(),
            role: role.map(str::to_owned),
            kind,
        });
    }

    fn stop(mut self, kind: ProblemKind) -> Result<Fixture, Vec<Problem>> {
        self.add(None, kind);
        Err(self.problems)
    }
}

pub fn parse_str(file: &Path, text: &str) -> Result<Fixture, Vec<Problem>> {
    parse_with(file, text, None)
}

pub fn load(path: &Path) -> Result<Fixture, Vec<Problem>> {
    let text = fs::read_to_string(path).map_err(|err| {
        vec![Problem {
            file: path.to_owned(),
            role: None,
            kind: ProblemKind::Read(err.to_string()),
        }]
    })?;
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    parse_with(path, &text, Some(&stem))
}

pub fn load_dir(dir: &Path) -> Result<Vec<Loaded>, Vec<Problem>> {
    let read_error = |err: std::io::Error| {
        vec![Problem {
            file: dir.to_owned(),
            role: None,
            kind: ProblemKind::Read(err.to_string()),
        }]
    };
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir).map_err(read_error)? {
        let path = entry.map_err(read_error)?.path();
        if path.extension() == Some(OsStr::new("json")) && path.is_file() {
            paths.push(path);
        }
    }
    paths.sort();
    let mut loaded = Vec::new();
    let mut problems = Vec::new();
    for path in paths {
        match load(&path) {
            Ok(fixture) => loaded.push(Loaded { path, fixture }),
            Err(mut found) => problems.append(&mut found),
        }
    }
    if problems.is_empty() {
        Ok(loaded)
    } else {
        Err(problems)
    }
}

fn parse_with(file: &Path, text: &str, stem: Option<&str>) -> Result<Fixture, Vec<Problem>> {
    let mut found = Collector {
        file,
        problems: Vec::new(),
    };
    let value: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(err) => return found.stop(ProblemKind::Syntax(err.to_string())),
    };
    let Some(top) = value.as_object() else {
        return found.stop(ProblemKind::Shape("a fixture is a JSON object".to_owned()));
    };
    match top.get("schema_version") {
        None => {
            return found.stop(ProblemKind::Shape("missing field `schema_version`".to_owned()));
        }
        Some(version) if version.as_u64() == Some(u64::from(SCHEMA_VERSION)) => {}
        Some(version) => {
            return found.stop(ProblemKind::UnsupportedVersion {
                found: version.to_string(),
            });
        }
    }
    let raw: Raw = match serde_json::from_value(value) {
        Ok(raw) => raw,
        Err(err) => return found.stop(ProblemKind::Shape(err.to_string())),
    };
    if let Some(stem) = stem
        && raw.name != stem
    {
        found.add(
            None,
            ProblemKind::NameMismatch {
                name: raw.name.clone(),
                stem: stem.to_owned(),
            },
        );
    }

    let mut processes = Vec::new();
    let mut nodes: Vec<(String, Option<String>)> = Vec::new();
    for (index, value) in raw.processes.into_iter().enumerate() {
        let role = value.get("role").and_then(Value::as_str).map(str::to_owned);
        if let Some(role) = &role {
            let parent = value
                .get("parent_role")
                .and_then(Value::as_str)
                .map(str::to_owned);
            nodes.push((role.clone(), parent));
        }
        match serde_json::from_value::<FixtureProcess>(value) {
            Ok(process) => processes.push(process),
            Err(err) => found.add(
                role.as_deref(),
                ProblemKind::Shape(format!("processes[{index}]: {err}")),
            ),
        }
    }
    let mut events = Vec::new();
    for (index, value) in raw.journal.into_iter().enumerate() {
        match serde_json::from_value::<FixtureEvent>(value) {
            Ok(event) => events.push((index, event)),
            Err(err) => found.add(None, ProblemKind::Shape(format!("journal[{index}]: {err}"))),
        }
    }

    check_roles(&mut found, &nodes);
    for process in &processes {
        check_process(&mut found, process);
    }
    let known: HashSet<&str> = nodes.iter().map(|(role, _)| role.as_str()).collect();
    for (index, event) in &events {
        check_event(&mut found, *index, event, &known);
    }

    if !found.problems.is_empty() {
        return Err(found.problems);
    }
    Ok(Fixture {
        schema_version: raw.schema_version,
        name: raw.name,
        description: raw.description,
        agent: raw.agent,
        processes,
        journal: events.into_iter().map(|(_, event)| event).collect(),
        session_ended: raw.session_ended,
    })
}

fn well_formed(role: &str) -> bool {
    !role.is_empty()
        && role.len() <= MAX_ROLE_LEN
        && role
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_'))
}

fn check_roles(found: &mut Collector<'_>, nodes: &[(String, Option<String>)]) {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut first_seen: Vec<&str> = Vec::new();
    for (role, _) in nodes {
        let count = counts.entry(role.as_str()).or_insert(0);
        if *count == 0 {
            first_seen.push(role.as_str());
        }
        *count += 1;
    }
    for role in &first_seen {
        if !well_formed(role) {
            found.add(Some(role), ProblemKind::BadRole);
        }
    }
    for role in &first_seen {
        if counts[role] > 1 {
            found.add(Some(role), ProblemKind::DuplicateRole { count: counts[role] });
        }
    }
    for (role, parent) in nodes {
        if let Some(parent) = parent
            && !counts.contains_key(parent.as_str())
        {
            found.add(
                Some(role),
                ProblemKind::MissingParent {
                    parent: parent.clone(),
                },
            );
        }
    }
    for path in cycles(nodes) {
        let role = path[0].clone();
        found.add(Some(&role), ProblemKind::Cycle { path });
    }
}

fn cycles(nodes: &[(String, Option<String>)]) -> Vec<Vec<String>> {
    let mut parent: HashMap<&str, Option<&str>> = HashMap::new();
    for (role, up) in nodes {
        parent.entry(role.as_str()).or_insert(up.as_deref());
    }
    let mut state: HashMap<&str, bool> = HashMap::new();
    let mut found: Vec<Vec<String>> = Vec::new();
    for (start, _) in nodes {
        let mut path: Vec<&str> = Vec::new();
        let mut current = start.as_str();
        loop {
            match state.get(current) {
                Some(false) => break,
                Some(true) => {
                    let at = path.iter().position(|role| *role == current).unwrap_or(0);
                    found.push(rotated(&path[at..]));
                    break;
                }
                None => {}
            }
            state.insert(current, true);
            path.push(current);
            match parent.get(current).copied().flatten() {
                Some(next) if parent.contains_key(next) => current = next,
                _ => break,
            }
        }
        for role in path {
            state.insert(role, false);
        }
    }
    found.sort();
    found
}

fn rotated(cycle: &[&str]) -> Vec<String> {
    let smallest = cycle
        .iter()
        .enumerate()
        .min_by_key(|(_, role)| **role)
        .map_or(0, |(index, _)| index);
    let mut path: Vec<String> = cycle[smallest..]
        .iter()
        .chain(&cycle[..smallest])
        .map(|role| (*role).to_owned())
        .collect();
    path.push(path[0].clone());
    path
}

fn check_process(found: &mut Collector<'_>, process: &FixtureProcess) {
    let role = Some(process.role.as_str());
    let label = process.expected.label;
    if process.expected.class != label.class() {
        found.add(
            role,
            ProblemKind::ClassContradictsLabel {
                label,
                found: process.expected.class,
                expected: label.class(),
            },
        );
    }
    if process.expected.must_never_signal != label.must_never_signal() {
        found.add(
            role,
            ProblemKind::MustNeverSignalContradictsLabel {
                label,
                found: process.expected.must_never_signal,
                expected: label.must_never_signal(),
            },
        );
    }
    match ProcSpec::parse(process.flags.iter().map(String::as_str)) {
        Err(err) => found.add(role, ProblemKind::BadFlags(err.to_string())),
        Ok(spec) => {
            if spec.spawn > 0 {
                found.add(role, ProblemKind::HarnessOwnedFlag("--spawn"));
            }
            if spec.report_file.is_some() {
                found.add(role, ProblemKind::HarnessOwnedFlag("--report-file"));
            }
            if spec.echo_env.is_some() {
                found.add(role, ProblemKind::HarnessOwnedFlag("--echo-env"));
            }
        }
    }
    if process.record_only && label != Label::TrueManaged {
        found.add(role, ProblemKind::RecordOnlyNotAllowed { label });
    }
}

fn check_event(found: &mut Collector<'_>, index: usize, event: &FixtureEvent, known: &HashSet<&str>) {
    if !event.roles.is_empty() && event.kind != Kind::Sample {
        found.add(
            None,
            ProblemKind::EventRolesNotAllowed {
                index,
                kind: event.kind,
            },
        );
    }
    for role in &event.roles {
        if !known.contains(role.as_str()) {
            found.add(
                None,
                ProblemKind::EventUnknownRole {
                    index,
                    role: role.clone(),
                },
            );
        }
    }
    if matches!(event.kind, Kind::ShellStart | Kind::ShellEnd) && event.tool_use_id.is_none() {
        found.add(
            None,
            ProblemKind::EventNeedsToolUseId {
                index,
                kind: event.kind,
            },
        );
    }
}
