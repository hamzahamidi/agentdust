use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use agentdust_core::manifest::{self, Entry, MANIFEST_FILE, Manifest, ManifestError, Origin, Resource};
use agentdust_core::safe_open::SafeOpenError;
use agentdust_core::user_file::{self, Loaded, UserFileError};
use thiserror::Error;

use crate::diff::unified_diff;
use crate::hook_config::{self, HookReport, HookState, hook_command, shell_quote};
use crate::native_cli::{
    self, CliRunner, McpLookup, SERVER_ARGS, SERVER_NAME, describe_server, matches_desired, mcp_hash,
};
use crate::transaction::{self, Step};

pub struct SetupEnv<'a> {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub exe: PathBuf,
    pub claude_cli: Option<PathBuf>,
    pub runner: Option<&'a dyn CliRunner>,
}

impl SetupEnv<'_> {
    pub fn settings_path(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }

    fn exe_text(&self) -> Result<String, SetupError> {
        self.exe.to_str().map(str::to_owned).ok_or(SetupError::BinaryPath)
    }

    fn target(&self) -> String {
        self.settings_path().to_string_lossy().into_owned()
    }

    fn config_target(&self) -> String {
        self.config_dir.to_string_lossy().into_owned()
    }
}

#[derive(Debug, Error)]
pub enum SetupError {
    #[error("the path of the agentdust binary is not valid UTF-8")]
    BinaryPath,
    #[error("the manifest cannot be used: {0}")]
    Manifest(#[from] ManifestError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpState {
    Installed,
    PreExisting,
    Absent,
    Missing,
    Modified,
    Stale,
    Conflict(String),
    Unknown(String),
    NoCli,
    Recorded,
}

impl McpState {
    fn is_ok(&self) -> bool {
        matches!(self, Self::Installed | Self::PreExisting | Self::Recorded)
    }

    fn is_drift(&self) -> bool {
        matches!(self, Self::Modified | Self::Stale | Self::Conflict(_))
    }

    fn label(&self) -> String {
        match self {
            Self::Installed => "registered".to_owned(),
            Self::PreExisting => "registered (not created by setup)".to_owned(),
            Self::Absent => "not registered".to_owned(),
            Self::Missing => "missing (setup registered it and it is gone)".to_owned(),
            Self::Modified => "modified since setup".to_owned(),
            Self::Stale => "registered for a different agentdust binary".to_owned(),
            Self::Conflict(found) => format!("a different server with this name exists: {found}"),
            Self::Unknown(reason) => format!("could not be checked: {reason}"),
            Self::NoCli => "cannot be checked: the claude CLI was not found".to_owned(),
            Self::Recorded => "recorded in the manifest (run agentdust setup --check to verify)".to_owned(),
        }
    }
}

fn hook_label(state: HookState) -> &'static str {
    match state {
        HookState::Installed => "installed",
        HookState::PreExisting => "present (not created by setup)",
        HookState::Absent => "not installed",
        HookState::Missing => "missing (setup installed it and it is gone)",
        HookState::Modified => "modified since setup",
        HookState::Stale => "installed for a different agentdust binary",
    }
}

fn hook_ok(state: HookState) -> bool {
    matches!(state, HookState::Installed | HookState::PreExisting)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overall {
    Installed,
    NotInstalled,
    Partial,
    Modified,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    pub settings_path: PathBuf,
    pub hooks: Result<Vec<HookReport>, String>,
    pub mcp: McpState,
    pub manifest_problem: Option<String>,
    pub through_symlink: bool,
}

impl CheckReport {
    pub fn ok(&self) -> bool {
        self.manifest_problem.is_none()
            && self.mcp.is_ok()
            && self
                .hooks
                .as_ref()
                .is_ok_and(|hooks| hooks.iter().all(|hook| hook_ok(hook.state)))
    }

    pub fn overall(&self) -> Overall {
        let Ok(hooks) = &self.hooks else {
            return Overall::Unknown;
        };
        if self.manifest_problem.is_some() {
            return Overall::Unknown;
        }
        if self.mcp.is_drift()
            || hooks
                .iter()
                .any(|hook| matches!(hook.state, HookState::Modified | HookState::Stale))
        {
            return Overall::Modified;
        }
        let good = hooks.iter().filter(|hook| hook_ok(hook.state)).count() + usize::from(self.mcp.is_ok());
        if good == hooks.len() + 1 {
            Overall::Installed
        } else if good == 0 {
            Overall::NotInstalled
        } else {
            Overall::Partial
        }
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("Claude Code settings: {}", self.settings_path.display()));
        if self.through_symlink {
            out.push_str(" (read through a symbolic link)");
        }
        out.push('\n');
        match &self.hooks {
            Ok(hooks) => {
                for hook in hooks {
                    out.push_str(&format!("  {}: {}\n", hook.event, hook_label(hook.state)));
                }
            }
            Err(reason) => out.push_str(&format!("  could not be checked: {reason}\n")),
        }
        out.push_str(&format!("MCP server {SERVER_NAME}: {}\n", self.mcp.label()));
        if let Some(problem) = &self.manifest_problem {
            out.push_str(&format!("Manifest: {problem}\n"));
        }
        out.push_str(match self.overall() {
            Overall::Installed => "Setup is installed.\n",
            Overall::NotInstalled => "Setup is not installed.\n",
            Overall::Partial => "Setup is only partly installed.\n",
            Overall::Modified => "Setup entries were modified or belong to another agentdust binary.\n",
            Overall::Unknown => "The state of setup could not be determined.\n",
        });
        out
    }
}

fn describe_file_error(path: &Path, err: &UserFileError) -> String {
    let what = match err {
        UserFileError::Refused(SafeOpenError::Symlink) => {
            "is a symbolic link, so setup does not edit it".to_owned()
        }
        UserFileError::Refused(SafeOpenError::NotRegular) => {
            "is not a regular file, so setup does not edit it".to_owned()
        }
        UserFileError::Refused(SafeOpenError::HardLinked { links }) => {
            format!("has {links} hard links, so setup does not edit it")
        }
        UserFileError::Refused(other) => format!("was refused: {other}"),
        UserFileError::Changed => "changed after it was read".to_owned(),
        UserFileError::Io(err) => format!("cannot be read: {err}"),
    };
    format!("{} {what}", path.display())
}

struct SettingsRead {
    text: Option<String>,
    through_symlink: bool,
}

fn read_settings_for_check(path: &Path) -> Result<SettingsRead, String> {
    let utf8 = |bytes: Vec<u8>| {
        String::from_utf8(bytes).map_err(|_| format!("{} is not valid UTF-8", path.display()))
    };
    match user_file::load(path) {
        Ok(Loaded::Missing) => Ok(SettingsRead {
            text: None,
            through_symlink: false,
        }),
        Ok(Loaded::Present(snapshot)) => Ok(SettingsRead {
            text: Some(utf8(snapshot.bytes)?),
            through_symlink: false,
        }),
        Err(UserFileError::Refused(SafeOpenError::Symlink)) => match fs::read(path) {
            Ok(bytes) => Ok(SettingsRead {
                text: Some(utf8(bytes)?),
                through_symlink: true,
            }),
            Err(err) => Err(format!(
                "{} cannot be read through the symbolic link: {err}",
                path.display()
            )),
        },
        Err(err) => Err(describe_file_error(path, &err)),
    }
}

fn owned_mcp<'a>(entries: &'a [Entry], config_target: &str) -> Option<&'a Entry> {
    entries.iter().find(|entry| {
        entry.resource == Resource::McpServer && entry.target == config_target && entry.key == SERVER_NAME
    })
}

fn classify_mcp(owned: Option<&Entry>, found: McpLookup, exe: &str) -> McpState {
    let created = owned.filter(|entry| entry.origin == Origin::Created);
    match found {
        McpLookup::Unknown(reason) => McpState::Unknown(reason),
        McpLookup::NotFound => {
            if created.is_some() {
                McpState::Missing
            } else {
                McpState::Absent
            }
        }
        McpLookup::Found(server) => {
            if let Some(entry) = created {
                let live = server
                    .command
                    .as_deref()
                    .map(|command| mcp_hash(command, &server.args));
                let intact = live.as_deref() == Some(entry.hash.as_str())
                    && matches_desired(&server, &entry.command, &entry.args.join(" "));
                return match (intact, entry.command == exe) {
                    (true, true) => McpState::Installed,
                    (true, false) => McpState::Stale,
                    (false, _) => McpState::Modified,
                };
            }
            if matches_desired(&server, exe, SERVER_ARGS) {
                McpState::PreExisting
            } else {
                McpState::Conflict(describe_server(&server))
            }
        }
    }
}

pub fn check(env: &SetupEnv, verify_mcp: bool) -> CheckReport {
    let settings_path = env.settings_path();
    let (entries, manifest_problem) = match manifest::load(&env.data_dir) {
        Ok(found) => (found.map(|manifest| manifest.entries).unwrap_or_default(), None),
        Err(err) => (Vec::new(), Some(err.to_string())),
    };
    let command = hook_command(&env.exe);
    let (hooks, through_symlink) = match read_settings_for_check(&settings_path) {
        Ok(read) => (
            hook_config::inspect(read.text.as_deref(), &env.target(), &entries, &command)
                .map_err(|err| err.to_string()),
            read.through_symlink,
        ),
        Err(reason) => (Err(reason), false),
    };
    let owned = owned_mcp(&entries, &env.config_target());
    let mcp = if manifest_problem.is_some() {
        McpState::Unknown("the manifest could not be read".to_owned())
    } else if !verify_mcp {
        if owned.is_some() {
            McpState::Recorded
        } else {
            McpState::Absent
        }
    } else if let (Some(runner), Some(exe)) = (env.runner, env.exe.to_str()) {
        classify_mcp(owned, native_cli::lookup(runner), exe)
    } else {
        McpState::NoCli
    };
    CheckReport {
        settings_path,
        hooks,
        mcp,
        manifest_problem,
        through_symlink,
    }
}

fn manual_mcp_command(exe: &str) -> String {
    format!(
        "claude mcp add --scope user {SERVER_NAME} -- {} {SERVER_ARGS}",
        shell_quote(exe)
    )
}

enum HooksPlan {
    Edit {
        before: Loaded,
        after: String,
        install: hook_config::Install,
    },
    Unchanged {
        install: hook_config::Install,
    },
    Blocked {
        reason: String,
        snippet: String,
    },
}

enum McpAction {
    Nothing,
    Register,
    Manual(String),
}

struct McpPlan {
    state: McpState,
    action: McpAction,
    entry: Option<Entry>,
}

struct Plan {
    exe: String,
    settings: PathBuf,
    config_dir: PathBuf,
    data_dir: PathBuf,
    hooks: HooksPlan,
    mcp: McpPlan,
    manifest_old: Option<Manifest>,
    manifest_new: Manifest,
    problems: Vec<String>,
    todo: Vec<String>,
}

fn plan_hooks(env: &SetupEnv, entries: &[Entry], command: &str) -> HooksPlan {
    let target = env.target();
    let blocked = |reason: String| HooksPlan::Blocked {
        reason,
        snippet: hook_config::install(None, &target, &[], command)
            .map(|fresh| fresh.text)
            .unwrap_or_default(),
    };
    let settings = env.settings_path();
    let before = match user_file::load(&settings) {
        Ok(loaded) => loaded,
        Err(err) => return blocked(describe_file_error(&settings, &err)),
    };
    let text = match before.bytes().map(std::str::from_utf8) {
        None => None,
        Some(Ok(text)) => Some(text),
        Some(Err(_)) => return blocked(format!("{} is not valid UTF-8", settings.display())),
    };
    match hook_config::install(text, &target, entries, command) {
        Ok(install) if text == Some(install.text.as_str()) => HooksPlan::Unchanged { install },
        Ok(install) => HooksPlan::Edit {
            before: before.clone(),
            after: install.text.clone(),
            install,
        },
        Err(err) => blocked(format!("{}: {err}", settings.display())),
    }
}

fn plan_mcp(env: &SetupEnv, exe: &str, entries: &[Entry]) -> McpPlan {
    let owned = owned_mcp(entries, &env.config_target());
    let Some(runner) = env.runner else {
        return McpPlan {
            state: McpState::NoCli,
            action: McpAction::Manual("the claude CLI was not found on PATH".to_owned()),
            entry: owned.cloned(),
        };
    };
    let state = classify_mcp(owned, native_cli::lookup(runner), exe);
    let created = |origin| Entry {
        resource: Resource::McpServer,
        target: env.config_target(),
        key: SERVER_NAME.to_owned(),
        origin,
        command: exe.to_owned(),
        args: vec![SERVER_ARGS.to_owned()],
        hash: mcp_hash(exe, SERVER_ARGS),
        created_hooks_key: false,
        created_event_key: false,
    };
    let (action, entry) = match &state {
        McpState::Absent | McpState::Missing => (McpAction::Register, Some(created(Origin::Created))),
        McpState::PreExisting => (McpAction::Nothing, Some(created(Origin::PreExisting))),
        McpState::Unknown(reason) => (McpAction::Manual(reason.clone()), owned.cloned()),
        _ => (McpAction::Nothing, owned.cloned()),
    };
    McpPlan { state, action, entry }
}

impl Plan {
    fn build(env: &SetupEnv) -> Result<Self, SetupError> {
        let exe = env.exe_text()?;
        let manifest_old = manifest::load(&env.data_dir)?;
        let base = manifest_old.clone().unwrap_or_default();
        let command = hook_command(&env.exe);
        let hooks = plan_hooks(env, &base.entries, &command);
        let mcp = plan_mcp(env, &exe, &base.entries);
        let target = env.target();
        let config_target = env.config_target();
        let mut entries: Vec<Entry> = base
            .entries
            .iter()
            .filter(|entry| match entry.resource {
                Resource::Hook => entry.target != target,
                Resource::McpServer => entry.target != config_target,
            })
            .cloned()
            .collect();
        match &hooks {
            HooksPlan::Edit { install, .. } | HooksPlan::Unchanged { install } => {
                entries.extend(install.entries.iter().cloned());
            }
            HooksPlan::Blocked { .. } => entries.extend(
                base.entries
                    .iter()
                    .filter(|entry| entry.resource == Resource::Hook && entry.target == target)
                    .cloned(),
            ),
        }
        entries.extend(mcp.entry.iter().cloned());
        let mut manifest_new = Manifest {
            version: base.version,
            entries,
            agent_executables: base.agent_executables.clone(),
        };
        if let Some(cli) = &env.claude_cli {
            let resolved = fs::canonicalize(cli).unwrap_or_else(|_| cli.clone());
            manifest_new
                .agent_executables
                .insert("claude".to_owned(), resolved.to_string_lossy().into_owned());
        }
        let mut plan = Self {
            exe,
            settings: env.settings_path(),
            config_dir: env.config_dir.clone(),
            data_dir: env.data_dir.clone(),
            hooks,
            mcp,
            manifest_old,
            manifest_new,
            problems: Vec::new(),
            todo: Vec::new(),
        };
        plan.collect_notes();
        Ok(plan)
    }

    fn collect_notes(&mut self) {
        match &self.hooks {
            HooksPlan::Blocked { reason, snippet } => {
                self.problems.push(format!("Hooks were not installed: {reason}."));
                self.todo.push(format!(
                    "Add these hooks to {} by hand:\n{}",
                    self.settings.display(),
                    indent(snippet)
                ));
            }
            HooksPlan::Edit { install, .. } | HooksPlan::Unchanged { install } => {
                for report in &install.states {
                    if matches!(report.state, HookState::Modified | HookState::Stale) {
                        self.problems.push(format!(
                            "Hook {} is {} and was left alone. Fix it by hand or run agentdust setup --remove.",
                            report.event,
                            hook_label(report.state)
                        ));
                    }
                }
            }
        }
        match (&self.mcp.state, &self.mcp.action) {
            (_, McpAction::Manual(reason)) if self.mcp.state == McpState::NoCli => {
                self.todo.push(format!(
                    "The claude CLI was not found ({reason}). Register the MCP server yourself:\n  {}",
                    manual_mcp_command(&self.exe)
                ));
            }
            (_, McpAction::Manual(reason)) => {
                self.problems
                    .push(format!("The MCP server was not registered: {reason}."));
                self.todo.push(format!(
                    "Register the MCP server yourself:\n  {}",
                    manual_mcp_command(&self.exe)
                ));
            }
            (state @ (McpState::Conflict(_) | McpState::Modified | McpState::Stale), _) => {
                self.problems.push(format!(
                    "MCP server {SERVER_NAME} was left alone: {}. Remove it with claude mcp remove {SERVER_NAME} --scope user and run setup again.",
                    state.label()
                ));
            }
            _ => {}
        }
    }

    fn manifest_changed(&self) -> bool {
        self.manifest_old.as_ref() != Some(&self.manifest_new)
            && !(self.manifest_old.is_none() && self.manifest_new.entries.is_empty())
    }

    fn is_noop(&self) -> bool {
        matches!(
            self.hooks,
            HooksPlan::Unchanged { .. } | HooksPlan::Blocked { .. }
        ) && matches!(self.mcp.action, McpAction::Nothing | McpAction::Manual(_))
            && !self.manifest_changed()
    }

    fn render(&self) -> String {
        let mut out = String::from(
            "agentdust setup will make these changes. Nothing is written until you approve.\n\n",
        );
        out.push_str(&format!(
            "  Claude Code config directory: {}\n",
            self.config_dir.display()
        ));
        out.push_str(&format!("  agentdust binary: {}\n", self.exe));
        out.push_str(&format!("  Data directory: {}\n\n", self.data_dir.display()));
        out.push_str("Hooks\n");
        match &self.hooks {
            HooksPlan::Edit { before, after, .. } => {
                let old = before
                    .bytes()
                    .map_or("", |bytes| std::str::from_utf8(bytes).unwrap_or(""));
                let label = if before.bytes().is_some() {
                    self.settings.display().to_string()
                } else {
                    "/dev/null".to_owned()
                };
                out.push_str(&format!("  Edit {}:\n", self.settings.display()));
                out.push_str(&unified_diff(
                    old,
                    after,
                    &label,
                    &self.settings.display().to_string(),
                ));
            }
            HooksPlan::Unchanged { .. } => {
                out.push_str(&format!("  No change to {}.\n", self.settings.display()));
            }
            HooksPlan::Blocked { reason, .. } => {
                out.push_str(&format!("  {reason}. Setup does not edit it.\n"));
            }
        }
        out.push_str("\nMCP server\n");
        match &self.mcp.action {
            McpAction::Register => {
                out.push_str(&format!("  Run: {}\n", manual_mcp_command(&self.exe)));
            }
            McpAction::Nothing => out.push_str(&format!("  No change ({}).\n", self.mcp.state.label())),
            McpAction::Manual(reason) => {
                out.push_str(&format!("  Setup cannot register it ({reason}).\n"));
            }
        }
        if self.manifest_changed() {
            out.push_str(&format!(
                "\nManifest\n  Record what setup adds in {}.\n",
                self.data_dir.join(MANIFEST_FILE).display()
            ));
        }
        for item in &self.todo {
            out.push_str(&format!("\n{item}\n"));
        }
        for problem in &self.problems {
            out.push_str(&format!("\nNot done: {problem}\n"));
        }
        out
    }

    fn apply<'a>(self, env: &SetupEnv<'a>, plan_text: String) -> InstallReport {
        let mut steps: Vec<Box<dyn Step + 'a>> = Vec::new();
        if self.manifest_changed() {
            steps.push(Box::new(ManifestStep {
                dir: self.data_dir.clone(),
                old: self.manifest_old.clone(),
                new: self.manifest_new.clone(),
            }));
        }
        if let HooksPlan::Edit { before, after, .. } = &self.hooks {
            steps.push(Box::new(SettingsStep {
                path: self.settings.clone(),
                before: before.clone(),
                after: after.clone(),
                written: false,
            }));
        }
        if let (McpAction::Register, Some(runner)) = (&self.mcp.action, env.runner) {
            steps.push(Box::new(McpStep {
                runner,
                exe: self.exe.clone(),
            }));
        }
        let transaction = transaction::run(&mut steps);
        InstallReport {
            plan_text,
            transaction,
            problems: self.problems,
            todo: self.todo,
        }
    }
}

fn indent(text: &str) -> String {
    text.lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

struct ManifestStep {
    dir: PathBuf,
    old: Option<Manifest>,
    new: Manifest,
}

impl Step for ManifestStep {
    fn name(&self) -> String {
        format!("record entries in {}", self.dir.join(MANIFEST_FILE).display())
    }

    fn apply(&mut self) -> Result<(), String> {
        manifest::store(&self.dir, &self.new).map_err(|err| err.to_string())
    }

    fn undo(&mut self) -> Result<(), String> {
        match &self.old {
            Some(old) => manifest::store(&self.dir, old),
            None => manifest::delete(&self.dir),
        }
        .map_err(|err| err.to_string())
    }
}

struct SettingsStep {
    path: PathBuf,
    before: Loaded,
    after: String,
    written: bool,
}

impl Step for SettingsStep {
    fn name(&self) -> String {
        format!("edit {}", self.path.display())
    }

    fn apply(&mut self) -> Result<(), String> {
        user_file::replace(&self.path, &self.before, self.after.as_bytes())
            .map_err(|err| format!("{}: {err}", self.path.display()))?;
        self.written = true;
        Ok(())
    }

    fn undo(&mut self) -> Result<(), String> {
        if !self.written {
            return Ok(());
        }
        let ours = Loaded::Present(user_file::Snapshot {
            bytes: self.after.clone().into_bytes(),
            mode: 0,
        });
        match &self.before {
            Loaded::Missing => user_file::delete_if_unchanged(&self.path, &ours),
            Loaded::Present(original) => user_file::replace(&self.path, &ours, &original.bytes),
        }
        .map_err(|err| format!("{}: {err}", self.path.display()))
    }
}

struct McpStep<'a> {
    runner: &'a dyn CliRunner,
    exe: String,
}

impl Step for McpStep<'_> {
    fn name(&self) -> String {
        "register the MCP server with the claude CLI".to_owned()
    }

    fn apply(&mut self) -> Result<(), String> {
        native_cli::register(self.runner, &self.exe).map_err(|err| err.to_string())
    }

    fn undo(&mut self) -> Result<(), String> {
        native_cli::unregister_if_ours(self.runner, &self.exe).map_err(|err| err.to_string())
    }
}

#[derive(Debug)]
pub struct InstallReport {
    pub plan_text: String,
    pub transaction: transaction::Report,
    pub problems: Vec<String>,
    pub todo: Vec<String>,
}

impl InstallReport {
    pub fn succeeded(&self) -> bool {
        self.transaction.succeeded() && self.problems.is_empty()
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        if let Some(failure) = &self.transaction.failure {
            out.push_str(&format!(
                "Setup failed at \"{}\": {}\n",
                failure.step, failure.reason
            ));
            if self.transaction.rollback.is_empty() {
                out.push_str("No earlier step had to be undone.\n");
            } else {
                out.push_str("Rolled back:\n");
                for undone in &self.transaction.rollback {
                    match &undone.result {
                        Ok(()) => out.push_str(&format!("  {}: undone\n", undone.step)),
                        Err(reason) => {
                            out.push_str(&format!("  {}: could not be undone ({reason})\n", undone.step));
                        }
                    }
                }
                if !self.transaction.rolled_back_cleanly() {
                    out.push_str("Some changes could not be undone. Check the steps above by hand.\n");
                }
            }
        } else if self.problems.is_empty() {
            out.push_str("Setup finished.\n");
        } else {
            out.push_str("Setup is incomplete.\n");
        }
        for problem in &self.problems {
            out.push_str(&format!("Not done: {problem}\n"));
        }
        for item in &self.todo {
            out.push_str(&format!("{item}\n"));
        }
        out
    }
}

#[derive(Debug)]
pub enum InstallOutcome {
    Nothing { text: String, complete: bool },
    Declined,
    Applied(InstallReport),
}

pub fn install(env: &SetupEnv, ask: &mut dyn FnMut(&str) -> bool) -> Result<InstallOutcome, SetupError> {
    let plan = Plan::build(env)?;
    if plan.is_noop() {
        let mut text = String::from("Nothing to change.\n");
        for problem in &plan.problems {
            text.push_str(&format!("Not done: {problem}\n"));
        }
        for item in &plan.todo {
            text.push_str(&format!("{item}\n"));
        }
        return Ok(InstallOutcome::Nothing {
            text,
            complete: plan.problems.is_empty(),
        });
    }
    let shown = plan.render();
    if !ask(&shown) {
        return Ok(InstallOutcome::Declined);
    }
    Ok(InstallOutcome::Applied(plan.apply(env, shown)))
}

enum HooksRemoval {
    None,
    Blocked(String),
    Edit {
        before: Loaded,
        removal: hook_config::Removal,
    },
    NoEdit {
        removal: hook_config::Removal,
    },
}

enum McpRemoval {
    None,
    Release,
    Gone,
    Unregister { command: String },
    Left(String),
}

struct RemovePlan {
    settings: PathBuf,
    data_dir: PathBuf,
    hooks: HooksRemoval,
    mcp: McpRemoval,
    manifest_old: Manifest,
    manifest_new: Manifest,
    problems: Vec<String>,
}

fn plan_hook_removal(env: &SetupEnv, manifest: &Manifest) -> HooksRemoval {
    let target = env.target();
    if !manifest
        .entries
        .iter()
        .any(|entry| entry.resource == Resource::Hook && entry.target == target)
    {
        return HooksRemoval::None;
    }
    let settings = env.settings_path();
    let before = match user_file::load(&settings) {
        Ok(loaded) => loaded,
        Err(err) => return HooksRemoval::Blocked(describe_file_error(&settings, &err)),
    };
    let text = match before.bytes().map(std::str::from_utf8) {
        None => None,
        Some(Ok(text)) => Some(text),
        Some(Err(_)) => return HooksRemoval::Blocked(format!("{} is not valid UTF-8", settings.display())),
    };
    match hook_config::remove(text, &target, &manifest.entries) {
        Ok(removal) if text.is_some_and(|text| text != removal.text) => {
            HooksRemoval::Edit { before, removal }
        }
        Ok(removal) => HooksRemoval::NoEdit { removal },
        Err(err) => HooksRemoval::Blocked(format!("{}: {err}", settings.display())),
    }
}

fn plan_mcp_removal(env: &SetupEnv, manifest: &Manifest) -> McpRemoval {
    let Some(entry) = owned_mcp(&manifest.entries, &env.config_target()) else {
        return McpRemoval::None;
    };
    if entry.origin == Origin::PreExisting {
        return McpRemoval::Release;
    }
    let Some(runner) = env.runner else {
        return McpRemoval::Left("the claude CLI was not found".to_owned());
    };
    match native_cli::lookup(runner) {
        McpLookup::NotFound => McpRemoval::Gone,
        McpLookup::Unknown(reason) => McpRemoval::Left(reason),
        McpLookup::Found(server) => {
            let live = server
                .command
                .as_deref()
                .map(|command| mcp_hash(command, &server.args));
            if live.as_deref() == Some(entry.hash.as_str())
                && matches_desired(&server, &entry.command, &entry.args.join(" "))
            {
                McpRemoval::Unregister {
                    command: entry.command.clone(),
                }
            } else {
                McpRemoval::Left(format!("modified since setup ({})", describe_server(&server)))
            }
        }
    }
}

impl RemovePlan {
    fn build(env: &SetupEnv, manifest_old: Manifest) -> Self {
        let hooks = plan_hook_removal(env, &manifest_old);
        let mcp = plan_mcp_removal(env, &manifest_old);
        let target = env.target();
        let config_target = env.config_target();
        let mut problems = Vec::new();
        let mut entries: Vec<Entry> = manifest_old
            .entries
            .iter()
            .filter(|entry| match entry.resource {
                Resource::Hook => entry.target != target,
                Resource::McpServer => entry.target != config_target,
            })
            .cloned()
            .collect();
        let hook_entries = |keep: &dyn Fn(&Entry) -> bool| -> Vec<Entry> {
            manifest_old
                .entries
                .iter()
                .filter(|entry| entry.resource == Resource::Hook && entry.target == target && keep(entry))
                .cloned()
                .collect()
        };
        match &hooks {
            HooksRemoval::None => {}
            HooksRemoval::Blocked(reason) => {
                problems.push(format!("Hooks were not removed: {reason}."));
                entries.extend(hook_entries(&|_| true));
            }
            HooksRemoval::Edit { removal, .. } | HooksRemoval::NoEdit { removal } => {
                entries.extend(removal.kept.iter().cloned());
                for key in &removal.drift {
                    problems.push(format!(
                        "Hook {key} is modified since setup and was left alone. Delete it by hand."
                    ));
                }
            }
        }
        match &mcp {
            McpRemoval::Left(reason) => {
                problems.push(format!("MCP server {SERVER_NAME} was left alone: {reason}."));
                entries.extend(owned_mcp(&manifest_old.entries, &config_target).cloned());
            }
            McpRemoval::None | McpRemoval::Release | McpRemoval::Gone | McpRemoval::Unregister { .. } => {}
        }
        let manifest_new = Manifest {
            version: manifest_old.version,
            entries,
            agent_executables: manifest_old.agent_executables.clone(),
        };
        Self {
            settings: env.settings_path(),
            data_dir: env.data_dir.clone(),
            hooks,
            mcp,
            manifest_old,
            manifest_new,
            problems,
        }
    }

    fn is_noop(&self) -> bool {
        matches!(
            self.hooks,
            HooksRemoval::None | HooksRemoval::Blocked(_) | HooksRemoval::NoEdit { .. }
        ) && matches!(self.mcp, McpRemoval::None | McpRemoval::Left(_))
            && self.manifest_old == self.manifest_new
    }

    fn render(&self) -> String {
        let mut out = String::from(
            "agentdust setup --remove will make these changes. Nothing is written until you approve.\n\n",
        );
        if let HooksRemoval::Edit { before, removal } = &self.hooks {
            let old = before
                .bytes()
                .map_or("", |bytes| std::str::from_utf8(bytes).unwrap_or(""));
            out.push_str(&format!("Hooks\n  Edit {}:\n", self.settings.display()));
            out.push_str(&unified_diff(
                old,
                &removal.text,
                &self.settings.display().to_string(),
                &self.settings.display().to_string(),
            ));
            out.push('\n');
        }
        match &self.mcp {
            McpRemoval::Unregister { .. } => {
                out.push_str(&format!(
                    "MCP server\n  Run: claude mcp remove {SERVER_NAME} --scope user\n\n"
                ));
            }
            McpRemoval::Release | McpRemoval::Gone => {
                out.push_str("MCP server\n  Forget the registration. The server itself is not touched.\n\n");
            }
            McpRemoval::None | McpRemoval::Left(_) => {}
        }
        let manifest_path = self.data_dir.join(MANIFEST_FILE);
        if self.manifest_new.entries.is_empty() {
            out.push_str(&format!("Manifest\n  Delete {}.\n", manifest_path.display()));
        } else if self.manifest_old != self.manifest_new {
            out.push_str(&format!("Manifest\n  Update {}.\n", manifest_path.display()));
        }
        for problem in &self.problems {
            out.push_str(&format!("\nNot done: {problem}\n"));
        }
        out
    }

    fn apply(self, env: &SetupEnv) -> RemoveReport {
        let mut lines: Vec<String> = Vec::new();
        let mut failed = false;
        if let HooksRemoval::Edit { before, removal } = &self.hooks {
            match user_file::replace(&self.settings, before, removal.text.as_bytes()) {
                Ok(()) => lines.push(format!("Edited {}.", self.settings.display())),
                Err(err) => {
                    failed = true;
                    lines.push(format!("Could not edit {}: {err}.", self.settings.display()));
                }
            }
        }
        let mut manifest_new = self.manifest_new.clone();
        if let (McpRemoval::Unregister { command }, Some(runner)) = (&self.mcp, env.runner) {
            match native_cli::unregister_if_ours(runner, command) {
                Ok(()) => lines.push(format!("Unregistered the MCP server {SERVER_NAME}.")),
                Err(err) => {
                    failed = true;
                    lines.push(format!("Could not unregister the MCP server: {err}."));
                    manifest_new
                        .entries
                        .extend(owned_mcp(&self.manifest_old.entries, &env.config_target()).cloned());
                }
            }
        }
        let stored = if manifest_new.entries.is_empty() {
            manifest::delete(&self.data_dir)
        } else if self.manifest_old != manifest_new {
            manifest::store(&self.data_dir, &manifest_new)
        } else {
            Ok(())
        };
        if let Err(err) = stored {
            failed = true;
            lines.push(format!("Could not update the manifest: {err}."));
        }
        for problem in &self.problems {
            lines.push(format!("Not done: {problem}"));
        }
        let complete = !failed && self.problems.is_empty();
        let mut text = lines.join("\n");
        text.push('\n');
        if complete {
            text.push_str("Setup entries removed.\n");
        }
        RemoveReport { text, complete }
    }
}

#[derive(Debug)]
pub struct RemoveReport {
    pub text: String,
    pub complete: bool,
}

#[derive(Debug)]
pub enum RemoveOutcome {
    Nothing { text: String },
    Declined,
    Applied(RemoveReport),
}

pub fn remove(env: &SetupEnv, ask: &mut dyn FnMut(&str) -> bool) -> Result<RemoveOutcome, SetupError> {
    let Some(manifest) = manifest::load(&env.data_dir)? else {
        return Ok(RemoveOutcome::Nothing {
            text: "Nothing to remove: setup has not recorded anything.\n".to_owned(),
        });
    };
    let plan = RemovePlan::build(env, manifest);
    if plan.is_noop() {
        let mut text = String::from("Nothing to remove.\n");
        for problem in &plan.problems {
            text.push_str(&format!("Not done: {problem}\n"));
        }
        return Ok(RemoveOutcome::Nothing { text });
    }
    if !ask(&plan.render()) {
        return Ok(RemoveOutcome::Declined);
    }
    Ok(RemoveOutcome::Applied(plan.apply(env)))
}

pub fn ask_yes_no(input: &mut dyn BufRead, output: &mut dyn Write, prompt: &str) -> io::Result<bool> {
    output.write_all(prompt.as_bytes())?;
    output.flush()?;
    let mut line = String::new();
    input.read_line(&mut line)?;
    Ok(matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}
