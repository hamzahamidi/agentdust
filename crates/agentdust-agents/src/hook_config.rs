use std::path::{Component, Path, PathBuf};

use agentdust_core::digest::sha256_hex;
use agentdust_core::manifest::{Entry, Origin, Resource};
use serde_json::Value;
use thiserror::Error;

use crate::json_edit::{self, EditError, Json, Seg};

pub const HOOK_TIMEOUT_SECS: u64 = 10;
const SHELL_TOOL: &str = "Bash";
const EMPTY_SETTINGS: &str = "{}\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookSpec {
    pub event: &'static str,
    pub matcher: Option<&'static str>,
}

pub const HOOK_SPECS: [HookSpec; 4] = [
    HookSpec {
        event: "SessionStart",
        matcher: None,
    },
    HookSpec {
        event: "SessionEnd",
        matcher: None,
    },
    HookSpec {
        event: "PreToolUse",
        matcher: Some(SHELL_TOOL),
    },
    HookSpec {
        event: "PostToolUse",
        matcher: Some(SHELL_TOOL),
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookState {
    Installed,
    PreExisting,
    Absent,
    Missing,
    Modified,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookReport {
    pub event: &'static str,
    pub state: HookState,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PatchError {
    #[error("the settings file is not valid JSON: {0}")]
    Syntax(String),
    #[error("the settings file cannot be edited safely: {0}")]
    Shape(String),
    #[error("the settings file repeats the key \"{0}\" and cannot be edited safely")]
    Duplicate(String),
}

impl From<EditError> for PatchError {
    fn from(err: EditError) -> Self {
        match err {
            EditError::Syntax(reason) => Self::Syntax(reason),
            EditError::Duplicate(key) => Self::Duplicate(key),
            other => Self::Shape(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Install {
    pub text: String,
    pub entries: Vec<Entry>,
    pub states: Vec<HookReport>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    pub text: String,
    pub removed: Vec<String>,
    pub gone: Vec<String>,
    pub released: Vec<String>,
    pub drift: Vec<String>,
    pub kept: Vec<Entry>,
}

pub fn hook_command(exe: &Path) -> String {
    format!("{} hook claude", shell_quote(&exe.to_string_lossy()))
}

pub fn shell_quote(text: &str) -> String {
    let safe = !text.is_empty()
        && text.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(ch, '_' | '@' | '%' | '+' | '=' | ':' | ',' | '.' | '/' | '-')
        });
    if safe {
        text.to_owned()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

pub fn stable_exe(path: &Path) -> PathBuf {
    let parts: Vec<Component> = path.components().collect();
    for (index, part) in parts.iter().enumerate() {
        let is_cellar = matches!(part, Component::Normal(name) if *name == "Cellar");
        let shaped =
            parts.len() == index + 5 && matches!(parts[index + 3], Component::Normal(name) if name == "bin");
        if is_cellar && shaped {
            let prefix: PathBuf = parts[..index].iter().collect();
            return prefix.join("bin").join(parts[index + 4]);
        }
    }
    path.to_path_buf()
}

pub fn canonical_json(value: &Value) -> String {
    match value {
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", parts.join(","))
        }
        Value::Object(map) => {
            let mut names: Vec<&String> = map.keys().collect();
            names.sort();
            let parts: Vec<String> = names
                .into_iter()
                .map(|name| format!("{}:{}", Value::String(name.clone()), canonical_json(&map[name])))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        scalar => scalar.to_string(),
    }
}

pub fn group_hash(group: &Value) -> String {
    sha256_hex(canonical_json(group).as_bytes())
}

fn group_json(spec: HookSpec, command: &str) -> Json {
    let hook = Json::Object(vec![
        ("type".to_owned(), Json::Str("command".to_owned())),
        ("command".to_owned(), Json::Str(command.to_owned())),
        ("timeout".to_owned(), Json::Int(HOOK_TIMEOUT_SECS)),
    ]);
    let mut members = Vec::new();
    if let Some(matcher) = spec.matcher {
        members.push(("matcher".to_owned(), Json::Str(matcher.to_owned())));
    }
    members.push(("hooks".to_owned(), Json::Array(vec![hook])));
    Json::Object(members)
}

fn hooks_of(group: &Value) -> impl Iterator<Item = &Value> {
    group.get("hooks").and_then(Value::as_array).into_iter().flatten()
}

fn group_has_command(group: &Value, command: &str) -> bool {
    hooks_of(group).any(|hook| hook.get("command").and_then(Value::as_str) == Some(command))
}

fn equivalent(group: &Value, spec: HookSpec, command: &str) -> bool {
    let matcher = group.get("matcher");
    let matcher_ok = match spec.matcher {
        Some(expected) => matcher.and_then(Value::as_str) == Some(expected),
        None => matcher.is_none() || matcher.and_then(Value::as_str) == Some(""),
    };
    matcher_ok
        && hooks_of(group).any(|hook| {
            hook.get("type").and_then(Value::as_str) == Some("command")
                && hook.get("command").and_then(Value::as_str) == Some(command)
        })
}

fn owned_entry<'a>(owned: &'a [Entry], target: &str, event: &str) -> Option<&'a Entry> {
    owned
        .iter()
        .find(|entry| entry.resource == Resource::Hook && entry.target == target && entry.key == event)
}

fn base_text(text: Option<&str>) -> &str {
    match text {
        Some(text) if !text.trim().is_empty() => text,
        _ => EMPTY_SETTINGS,
    }
}

fn read_settings(text: &str) -> Result<Value, PatchError> {
    json_edit::validate(text)?;
    let value: Value = serde_json::from_str(text).map_err(|err| PatchError::Syntax(err.to_string()))?;
    if !value.is_object() {
        return Err(PatchError::Shape(
            "the top level must be a JSON object".to_owned(),
        ));
    }
    Ok(value)
}

fn groups_of<'a>(settings: &'a Value, event: &str) -> Result<&'a [Value], PatchError> {
    let Some(hooks) = settings.get("hooks") else {
        return Ok(&[]);
    };
    let Some(hooks) = hooks.as_object() else {
        return Err(PatchError::Shape("hooks is not an object".to_owned()));
    };
    match hooks.get(event) {
        None => Ok(&[]),
        Some(Value::Array(groups)) => Ok(groups),
        Some(_) => Err(PatchError::Shape(format!("hooks.{event} is not an array"))),
    }
}

fn classify(groups: &[Value], spec: HookSpec, owned: Option<&Entry>, command: &str) -> HookState {
    if let Some(entry) = owned.filter(|entry| entry.origin == Origin::Created) {
        if groups.iter().any(|group| group_hash(group) == entry.hash) {
            return if entry.command == command {
                HookState::Installed
            } else {
                HookState::Stale
            };
        }
        return if groups
            .iter()
            .any(|group| group_has_command(group, &entry.command))
        {
            HookState::Modified
        } else {
            HookState::Missing
        };
    }
    if groups.iter().any(|group| equivalent(group, spec, command)) {
        HookState::PreExisting
    } else {
        HookState::Absent
    }
}

fn states_of(
    settings: &Value,
    target: &str,
    owned: &[Entry],
    command: &str,
) -> Result<Vec<HookReport>, PatchError> {
    HOOK_SPECS
        .iter()
        .map(|spec| {
            let groups = groups_of(settings, spec.event)?;
            let state = classify(groups, *spec, owned_entry(owned, target, spec.event), command);
            Ok(HookReport {
                event: spec.event,
                state,
            })
        })
        .collect()
}

pub fn inspect(
    text: Option<&str>,
    target: &str,
    owned: &[Entry],
    command: &str,
) -> Result<Vec<HookReport>, PatchError> {
    let settings = read_settings(base_text(text))?;
    states_of(&settings, target, owned, command)
}

fn key(name: &str) -> Seg {
    Seg::Key(name.to_owned())
}

fn add_group(working: &str, spec: HookSpec, command: &str) -> Result<(String, bool, bool), PatchError> {
    let group = group_json(spec, command);
    let settings = read_settings(working)?;
    if settings.get("hooks").is_none() {
        let hooks = Json::Object(vec![(spec.event.to_owned(), Json::Array(vec![group]))]);
        let edited = json_edit::insert_member(working, &[], "hooks", &hooks)?;
        return Ok((edited, true, true));
    }
    let event_exists = settings["hooks"].get(spec.event).is_some();
    if event_exists {
        let edited = json_edit::append_item(working, &[key("hooks"), key(spec.event)], &group)?;
        Ok((edited, false, false))
    } else {
        let edited =
            json_edit::insert_member(working, &[key("hooks")], spec.event, &Json::Array(vec![group]))?;
        Ok((edited, false, true))
    }
}

pub fn install(
    text: Option<&str>,
    target: &str,
    owned: &[Entry],
    command: &str,
) -> Result<Install, PatchError> {
    let mut working = base_text(text).to_owned();
    let settings = read_settings(&working)?;
    let states = states_of(&settings, target, owned, command)?;
    let mut entries = Vec::new();
    for (spec, report) in HOOK_SPECS.iter().zip(&states) {
        let current = owned_entry(owned, target, spec.event);
        match report.state {
            HookState::Installed | HookState::Stale | HookState::Modified => {
                entries.extend(current.cloned());
            }
            HookState::PreExisting => {
                let group = groups_of(&settings, spec.event)?
                    .iter()
                    .find(|group| equivalent(group, *spec, command))
                    .map(group_hash)
                    .unwrap_or_default();
                entries.push(entry(
                    target,
                    *spec,
                    command,
                    Origin::PreExisting,
                    group,
                    (false, false),
                ));
            }
            HookState::Absent | HookState::Missing => {
                let (edited, created_hooks, created_event) = add_group(&working, *spec, command)?;
                working = edited;
                let hash = group_hash(&group_json(*spec, command).to_value());
                entries.push(entry(
                    target,
                    *spec,
                    command,
                    Origin::Created,
                    hash,
                    (created_hooks, created_event),
                ));
            }
        }
    }
    Ok(Install {
        text: working,
        entries,
        states,
    })
}

fn entry(
    target: &str,
    spec: HookSpec,
    command: &str,
    origin: Origin,
    hash: String,
    created: (bool, bool),
) -> Entry {
    Entry {
        resource: Resource::Hook,
        target: target.to_owned(),
        key: spec.event.to_owned(),
        origin,
        command: command.to_owned(),
        args: Vec::new(),
        hash,
        created_hooks_key: created.0,
        created_event_key: created.1,
    }
}

pub fn remove(text: Option<&str>, target: &str, owned: &[Entry]) -> Result<Removal, PatchError> {
    let mine: Vec<&Entry> = owned
        .iter()
        .filter(|entry| entry.resource == Resource::Hook && entry.target == target)
        .collect();
    let mut result = Removal {
        text: text.unwrap_or_default().to_owned(),
        removed: Vec::new(),
        gone: Vec::new(),
        released: Vec::new(),
        drift: Vec::new(),
        kept: Vec::new(),
    };
    let mut working = text.map(str::to_owned);
    let mut created_hooks = false;
    for entry in mine {
        if entry.origin == Origin::PreExisting {
            result.released.push(entry.key.clone());
            continue;
        }
        if !HOOK_SPECS.iter().any(|spec| spec.event == entry.key) {
            result.kept.push(entry.clone());
            continue;
        }
        let Some(current) = working.as_deref() else {
            result.gone.push(entry.key.clone());
            continue;
        };
        let settings = read_settings(current)?;
        let groups = groups_of(&settings, &entry.key)?;
        if let Some(index) = groups.iter().position(|group| group_hash(group) == entry.hash) {
            let at = [key("hooks"), key(&entry.key)];
            let mut edited = json_edit::remove_item(current, &at, index)?;
            if entry.created_event_key && groups.len() == 1 {
                edited = json_edit::remove_member(&edited, &[key("hooks")], &entry.key)?;
            }
            created_hooks |= entry.created_hooks_key;
            working = Some(edited);
            result.removed.push(entry.key.clone());
        } else if groups
            .iter()
            .any(|group| group_has_command(group, &entry.command))
        {
            result.drift.push(entry.key.clone());
            result.kept.push(entry.clone());
        } else {
            result.gone.push(entry.key.clone());
        }
    }
    if let Some(current) = working.as_deref() {
        let settings = read_settings(current)?;
        let hooks_empty = settings
            .get("hooks")
            .and_then(Value::as_object)
            .is_some_and(|hooks| hooks.is_empty());
        let edited = if created_hooks && hooks_empty {
            json_edit::remove_member(current, &[], "hooks")?
        } else {
            current.to_owned()
        };
        result.text = edited;
    }
    Ok(result)
}
