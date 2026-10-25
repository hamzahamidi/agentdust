use std::cell::RefCell;
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, IsTerminal, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
#[cfg(target_os = "macos")]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agentdust_core::ancestry::find_agent;
use agentdust_core::apply::exec::{Deps, Executor, Settings};
use agentdust_core::apply::lock::IdentityLock;
#[cfg(target_os = "macos")]
use agentdust_core::apply::signal::KillSignaller;
use agentdust_core::apply::signal::Signaller;
use agentdust_core::apply::timer::SystemTimer;
use agentdust_core::automatic::{self, PolicyGuard};
use agentdust_core::class::Class;
use agentdust_core::identity::KernelIdentity;
use agentdust_core::journal::{self, Journal};
#[cfg(target_os = "linux")]
use agentdust_core::linux::LinuxPidfdSignaller;
use agentdust_core::live::LiveSurveyor;
use agentdust_core::plan::PlanItem;
use agentdust_core::platform::Provider;
use agentdust_core::provider::{ProcessProvider, ProcessRead};
use agentdust_core::session::{self, Liveness, LivenessProbe, ProviderLiveness};
use agentdust_core::{clock, cwd, paths, safe_open, secret};
use serde_json::{Value, json};

#[cfg(target_os = "macos")]
const LABEL: &str = "com.agentdust.automatic";
const USAGE: &str = "usage: agentdust auto enable [--yes] PROJECT... | enable [--yes] --projects ROOT | disable [PROJECT] | pause | resume | keep PID | unkeep PID | status";

pub fn run(args: &[&str]) -> ExitCode {
    match execute(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("agentdust auto: {err}");
            ExitCode::FAILURE
        }
    }
}

fn error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

struct EnableRequest {
    projects: Vec<PathBuf>,
    unattended: bool,
}

fn enable_request(args: &[&str]) -> io::Result<Option<EnableRequest>> {
    if args.first() != Some(&"enable") {
        return Ok(None);
    }
    let mut paths = &args[1..];
    let unattended = paths.first() == Some(&"--yes");
    if unattended {
        paths = &paths[1..];
    }
    let mut projects = Vec::new();
    match paths {
        ["--projects", root] => {
            let root = project_directory(root)?;
            projects.push(root.clone());
            for entry in fs::read_dir(&root)? {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let git = entry.path().join(".git");
                if fs::symlink_metadata(git).is_ok_and(|meta| meta.is_dir() || meta.is_file()) {
                    let project = project_directory(entry.path())?;
                    if project.parent() != Some(root.as_path()) {
                        return Err(error("discovered project escaped its parent directory"));
                    }
                    projects.push(project);
                    if projects.len() > 128 {
                        return Err(error("automatic policy limit reached: at most 128 directories"));
                    }
                }
            }
        }
        [] => return Err(error(USAGE)),
        paths => {
            for path in paths {
                if path.starts_with("--") {
                    return Err(error(USAGE));
                }
                projects.push(project_directory(path)?);
            }
        }
    }
    projects.sort();
    projects.dedup();
    if projects.len() > 128 {
        return Err(error("automatic policy limit reached: at most 128 directories"));
    }
    Ok(Some(EnableRequest { projects, unattended }))
}

fn project_directory(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let project = fs::canonicalize(path)?;
    if !project.is_dir() || project.parent().is_none() || project.to_str().is_none() {
        return Err(error("select a UTF-8 project directory, not the filesystem root"));
    }
    Ok(project)
}

fn merged_projects(
    existing: &[journal::CwdKey],
    projects: &[PathBuf],
    install_secret: &secret::Secret,
) -> io::Result<Vec<journal::CwdKey>> {
    let mut keys = existing.to_vec();
    for project in projects {
        let key = cwd::cwd_key(
            install_secret,
            project.to_str().ok_or_else(|| error("project is not UTF-8"))?,
        )
        .ok_or_else(|| error("project cannot be recorded"))?;
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    if keys.len() > 128 {
        return Err(error("automatic policy limit reached: at most 128 directories"));
    }
    Ok(keys)
}

fn add_projects(
    guard: &mut PolicyGuard,
    projects: &[PathBuf],
    install_secret: &secret::Secret,
) -> io::Result<()> {
    let keys = merged_projects(&guard.policy.projects, projects, install_secret)?;
    guard.policy.projects = keys;
    guard.policy.enabled = true;
    guard.write()
}

fn execute(args: &[&str]) -> io::Result<()> {
    let dir = paths::data_dir()?;
    if args == ["status"] {
        println!("{}", serde_json::to_string_pretty(&automatic::status(&dir)?)?);
        return Ok(());
    }
    if args == ["worker"] {
        return worker(&dir);
    }
    let enable = enable_request(args)?;
    if enable.is_none()
        && !matches!(
            args,
            ["disable", _] | ["pause"] | ["resume"] | ["disable"] | ["keep", _] | ["unkeep", _]
        )
    {
        return Err(error(USAGE));
    }
    validate_request(&dir, args, enable.as_ref())?;
    let provider = Provider::new()?;
    if let Some(request) = &enable {
        show_selection(request);
    }
    if !enable.as_ref().is_some_and(|request| request.unattended) {
        human_consent(&provider, args)?;
    }
    safe_open::ensure_dir(&dir).map_err(error)?;
    let guard = PolicyGuard::acquire(&dir)?;
    match enable {
        Some(request) => enable_cleanup(&dir, guard, &request),
        None => execute_policy(args, &dir, &provider, guard),
    }
}

fn validate_request(dir: &Path, _args: &[&str], enable: Option<&EnableRequest>) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    if enable.is_some() || _args == ["resume"] {
        require_supported_data_volume(dir)?;
    }
    if let Some(request) = enable {
        let existing = automatic::read(dir)?;
        if !existing.projects.is_empty() {
            merged_projects(
                &existing.projects,
                &request.projects,
                &secret::load_existing(dir).map_err(error)?,
            )?;
        }
    }
    Ok(())
}

fn show_selection(request: &EnableRequest) {
    println!(
        "Selected {} exact session-start directories:",
        request.projects.len()
    );
    for project in &request.projects {
        println!(
            "  {}",
            agentdust_core::sanitize::escape(&project.to_string_lossy())
        );
    }
}

fn enable_cleanup(dir: &Path, mut guard: PolicyGuard, request: &EnableRequest) -> io::Result<()> {
    let install_secret = secret::load_or_create(dir).map_err(error)?;
    add_projects(&mut guard, &request.projects, &install_secret)?;
    drop(guard);
    if let Err(err) = install_worker(dir) {
        let mut guard = PolicyGuard::acquire(dir)?;
        guard.policy.enabled = false;
        guard.write()?;
        return Err(err);
    }
    println!(
        "Automatic cleanup enabled for {} selected directories. Scope is exact, not recursive.",
        request.projects.len()
    );
    Ok(())
}

fn execute_policy(args: &[&str], dir: &Path, provider: &Provider, guard: PolicyGuard) -> io::Result<()> {
    match args {
        ["resume"] => {
            resume(dir, guard)?;
        }
        ["disable", project] => disable_project(dir, guard, project)?,
        ["pause"] | ["disable"] => disable_all(args, guard)?,
        ["keep", pid] => keep_process(provider, guard, pid)?,
        ["unkeep", pid] => unkeep_process(guard, pid)?,
        _ => unreachable!(),
    }
    Ok(())
}

fn resume(dir: &Path, mut guard: PolicyGuard) -> io::Result<()> {
    if guard.policy.projects.is_empty() {
        return Err(error("enable a project directory first"));
    }
    guard.policy.enabled = true;
    guard.write()?;
    drop(guard);
    if let Err(err) = install_worker(dir) {
        let mut guard = PolicyGuard::acquire(dir)?;
        guard.policy.enabled = false;
        guard.write()?;
        return Err(err);
    }
    println!("Automatic cleanup resumed for the previously enabled directories.");
    Ok(())
}

fn disable_project(dir: &Path, mut guard: PolicyGuard, path: &str) -> io::Result<()> {
    let project = fs::canonicalize(path)?;
    let install_secret = secret::load_existing(dir).map_err(error)?;
    let key = cwd::cwd_key(
        &install_secret,
        project.to_str().ok_or_else(|| error("project is not UTF-8"))?,
    )
    .ok_or_else(|| error("project cannot be recorded"))?;
    guard.policy.projects.retain(|candidate| candidate != &key);
    let stop = guard.policy.projects.is_empty();
    if stop {
        guard.policy.enabled = false;
    }
    guard.write()?;
    drop(guard);
    if stop {
        remove_worker()?;
    }
    println!("Selected directory is outside automatic cleanup scope.");
    Ok(())
}

fn disable_all(args: &[&str], mut guard: PolicyGuard) -> io::Result<()> {
    guard.policy.enabled = false;
    if args == ["disable"] {
        guard.policy.projects.clear();
    }
    guard.write()?;
    drop(guard);
    remove_worker()?;
    println!("Automatic cleanup is off. Kept process identities remain protected.");
    Ok(())
}

fn keep_process(provider: &Provider, mut guard: PolicyGuard, value: &str) -> io::Result<()> {
    let pid: i32 = value.parse().map_err(error)?;
    let identity = match provider.read(pid)? {
        ProcessRead::Present(process) => process.kernel,
        ProcessRead::PathUnreadable(kernel) => kernel,
        ProcessRead::Gone => return Err(error("process is gone")),
    };
    if identity.uid != unsafe { libc::geteuid() } || pid <= 1 {
        return Err(error("select one process owned by this user"));
    }
    if !guard.policy.keep.contains(&identity) {
        guard.policy.keep.push(identity);
    }
    guard.write()?;
    println!("Kept process {pid}. Manual and automatic cleanup will leave this identity alone.");
    Ok(())
}

fn unkeep_process(mut guard: PolicyGuard, value: &str) -> io::Result<()> {
    let pid: i32 = value.parse().map_err(error)?;
    guard.policy.keep.retain(|identity| identity.pid != pid);
    guard.write()?;
    println!(
        "Removed keeps for PID {pid}. Future cleanup still requires fresh ownership and identity checks."
    );
    Ok(())
}

fn human_consent(provider: &Provider, args: &[&str]) -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(error("policy changes require a human foreground terminal"));
    }
    let tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
    let mut terminal = BufReader::new(tty);
    let foreground = unsafe { libc::tcgetpgrp(terminal.get_ref().as_raw_fd()) };
    if foreground < 0
        || foreground != unsafe { libc::getpgrp() }
        || find_agent(provider, std::process::id() as i32).is_some()
    {
        return Err(error(
            "policy changes cannot run under an agent or outside the foreground terminal",
        ));
    }
    writeln!(
        terminal.get_mut(),
        "Requested policy change: {}",
        agentdust_core::sanitize::escape(&args.join(" "))
    )?;
    writeln!(
        terminal.get_mut(),
        "Enabled projects allow one automatic SIGTERM per proven leftover identity after every recorded owner is gone. Uncertain cases require explicit approval. Keeps block manual and automatic cleanup."
    )?;
    write!(
        terminal.get_mut(),
        "Type ENABLE to enable cleanup, or CONFIRM for other policy changes: "
    )?;
    terminal.get_mut().flush()?;
    let mut answer = String::new();
    terminal.read_line(&mut answer)?;
    let expected = if args.first() == Some(&"enable") {
        "ENABLE"
    } else {
        "CONFIRM"
    };
    if answer.trim_end_matches(['\r', '\n']) != expected {
        return Err(error("cancelled"));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn plist_path() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| error("HOME is not set"))?;
    let home = PathBuf::from(home);
    if !home.is_absolute() {
        return Err(error("HOME must be absolute"));
    }
    Ok(home.join("Library/LaunchAgents").join(format!("{LABEL}.plist")))
}

#[cfg(target_os = "macos")]
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(target_os = "macos")]
fn launchctl(args: &[&str]) -> io::Result<()> {
    let output = Command::new("/bin/launchctl").args(args).output()?;
    if !output.status.success() {
        return Err(error(format!(
            "launchctl failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn domain() -> String {
    format!("gui/{}", unsafe { libc::geteuid() })
}

#[cfg(target_os = "macos")]
fn render_plist(label: &str, exe: &Path, dir: &Path) -> io::Result<String> {
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>{label}</string><key>ProgramArguments</key><array><string>{}</string><string>auto</string><string>worker</string></array><key>EnvironmentVariables</key><dict><key>AGENTDUST_DATA_DIR</key><string>{}</string></dict><key>RunAtLoad</key><true/><key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict><key>ThrottleInterval</key><integer>10</integer></dict></plist>",
        xml(exe.to_str().ok_or_else(|| error("binary path is not UTF-8"))?),
        xml(dir.to_str().ok_or_else(|| error("data directory is not UTF-8"))?),
    );
    Ok(plist)
}

#[cfg(target_os = "macos")]
fn install_worker(dir: &Path) -> io::Result<()> {
    let path = plist_path()?;
    let parent = path
        .parent()
        .ok_or_else(|| error("LaunchAgents directory unavailable"))?;
    fs::create_dir_all(parent)?;
    if fs::symlink_metadata(parent)?.file_type().is_symlink()
        || fs::metadata(parent)?.uid() != unsafe { libc::geteuid() }
    {
        return Err(error(
            "LaunchAgents directory must belong to this user and cannot be a link",
        ));
    }
    let exe = agentdust_agents::hook_config::stable_exe(&std::env::current_exe()?);
    let plist = render_plist(LABEL, &exe, dir)?;
    let service = format!("{}/{LABEL}", domain());
    let loaded = Command::new("/bin/launchctl")
        .args(["print", &service])
        .output()?
        .status
        .success();
    if loaded {
        launchctl(&["bootout", &service])?;
    }
    if fs::symlink_metadata(&path).is_ok() {
        safe_open::open_file(&path, safe_open::Access::Read).map_err(error)?;
        fs::remove_file(&path)?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)?;
    file.write_all(plist.as_bytes())?;
    file.sync_all()?;
    launchctl(&[
        "bootstrap",
        &domain(),
        path.to_str().ok_or_else(|| error("plist path is not UTF-8"))?,
    ])
}

#[cfg(target_os = "macos")]
fn remove_worker() -> io::Result<()> {
    let service = format!("{}/{LABEL}", domain());
    if Command::new("/bin/launchctl")
        .args(["print", &service])
        .output()?
        .status
        .success()
    {
        launchctl(&["bootout", &service])?;
    }
    let path = plist_path()?;
    match safe_open::open_file(&path, safe_open::Access::Read) {
        Ok(_) => fs::remove_file(path),
        Err(safe_open::SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(error(err)),
    }
}

#[cfg(target_os = "linux")]
fn require_supported_data_volume(dir: &Path) -> io::Result<()> {
    use agentdust_core::journal::volume::{self, SystemVolume};

    match volume::locate(&SystemVolume, dir)? {
        facts if facts.supported => Ok(()),
        facts => Err(error(format!(
            "automatic cleanup requires writable ext4; found {}",
            facts.describe()
        ))),
    }
}

#[cfg(target_os = "linux")]
fn systemd_user_directory(create: bool) -> io::Result<PathBuf> {
    use std::os::unix::fs::MetadataExt;

    let config_home = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => PathBuf::from(std::env::var_os("HOME").ok_or_else(|| error("HOME is not set"))?).join(".config"),
    };
    if !config_home.is_absolute() {
        return Err(error("XDG_CONFIG_HOME must be absolute"));
    }
    if create {
        fs::create_dir_all(&config_home)?;
    }
    let config_home = match fs::canonicalize(&config_home) {
        Ok(path) => path,
        Err(err) if !create && err.kind() == io::ErrorKind::NotFound => {
            return Ok(config_home.join("systemd/user"));
        }
        Err(err) => return Err(err),
    };
    let systemd_dir = config_home.join("systemd");
    let user_dir = systemd_dir.join("user");
    if create {
        fs::create_dir_all(&user_dir)?;
    }
    let user_dir = match fs::canonicalize(&user_dir) {
        Ok(path) => path,
        Err(err) if !create && err.kind() == io::ErrorKind::NotFound => {
            return Ok(user_dir);
        }
        Err(err) => return Err(err),
    };
    if !user_dir.starts_with(&config_home) {
        return Err(error("systemd user unit directory escapes XDG_CONFIG_HOME"));
    }
    for path in [config_home.as_path(), systemd_dir.as_path(), user_dir.as_path()] {
        let metadata = fs::metadata(path)?;
        if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0
        {
            return Err(error(
                "systemd user unit directories must be user owned and not writable by others",
            ));
        }
    }
    Ok(user_dir)
}

#[cfg(target_os = "linux")]
fn systemd_quote(value: &str) -> String {
    let mut quoted = String::from("\"");
    for character in value.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '%' => quoted.push_str("%%"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(target_os = "linux")]
fn render_systemd_unit(exe: &Path, dir: &Path) -> io::Result<String> {
    let exe = exe.to_str().ok_or_else(|| error("binary path is not UTF-8"))?;
    let dir = dir.to_str().ok_or_else(|| error("data directory is not UTF-8"))?;
    Ok(format!(
        "# Managed by AgentDust\n[Unit]\nDescription=AgentDust automatic cleanup\nAfter=default.target\n\n[Service]\nType=simple\nExecStart={} auto worker\nEnvironment=AGENTDUST_DATA_DIR={}\nRestart=on-failure\nRestartSec=10\n\n[Install]\nWantedBy=default.target\n",
        systemd_quote(exe),
        systemd_quote(dir)
    ))
}

#[cfg(target_os = "linux")]
const SYSTEMD_UNIT: &str = "agentdust-automatic.service";

#[cfg(target_os = "linux")]
fn systemd_user(args: &[&str]) -> io::Result<()> {
    let output = Command::new("systemctl").arg("--user").args(args).output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(error(format!(
            "systemctl --user {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

#[cfg(target_os = "linux")]
fn read_agentdust_unit(path: &Path) -> io::Result<Option<String>> {
    match safe_open::open_file(path, safe_open::Access::Read) {
        Ok(mut file) => {
            let mut text = String::new();
            use std::io::Read;
            file.read_to_string(&mut text)?;
            Ok(Some(text))
        }
        Err(safe_open::SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(error(err)),
    }
}

#[cfg(target_os = "linux")]
fn write_agentdust_unit(path: &Path, contents: &str) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    if let Some(existing) = read_agentdust_unit(path)? {
        if !existing.lines().any(|line| line == "# Managed by AgentDust") {
            return Err(error(
                "an unowned systemd unit already uses the AgentDust service path",
            ));
        }
        fs::remove_file(path)?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()
}

#[cfg(target_os = "linux")]
fn remove_agentdust_unit(path: &Path) -> io::Result<()> {
    if let Some(existing) = read_agentdust_unit(path)? {
        if !existing.lines().any(|line| line == "# Managed by AgentDust") {
            return Err(error(
                "an unowned systemd unit at the AgentDust service path was preserved",
            ));
        }
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn install_worker(dir: &Path) -> io::Result<()> {
    let unit_dir = systemd_user_directory(true)?;
    let path = unit_dir.join(SYSTEMD_UNIT);
    let exe = agentdust_agents::hook_config::stable_exe(&std::env::current_exe()?);
    write_agentdust_unit(&path, &render_systemd_unit(&exe, dir)?)?;
    systemd_user(&["daemon-reload"])?;
    if let Err(err) = systemd_user(&["enable", "--now", SYSTEMD_UNIT]) {
        let _ = systemd_user(&["disable", "--now", SYSTEMD_UNIT]);
        let _ = remove_agentdust_unit(&path);
        let _ = systemd_user(&["daemon-reload"]);
        return Err(err);
    }
    systemd_user(&["restart", SYSTEMD_UNIT])
}

#[cfg(target_os = "linux")]
fn remove_worker() -> io::Result<()> {
    let path = systemd_user_directory(false)?.join(SYSTEMD_UNIT);
    if read_agentdust_unit(&path)?.is_none() {
        return Ok(());
    }
    let stopped = systemd_user(&["disable", "--now", SYSTEMD_UNIT]);
    remove_agentdust_unit(&path)?;
    let reloaded = systemd_user(&["daemon-reload"]);
    stopped.and(reloaded)
}

type JournalStamp = Vec<(String, u64, u64, i64, i64)>;

struct OwnerProbe<'a> {
    provider: &'a Provider,
    gone: &'a RefCell<HashSet<KernelIdentity>>,
}

impl LivenessProbe for OwnerProbe<'_> {
    fn probe(&self, identity: &KernelIdentity) -> Liveness {
        if self.gone.borrow().contains(identity) {
            return Liveness::Gone;
        }
        let result = ProviderLiveness(self.provider).probe(identity);
        if result == Liveness::Gone {
            self.gone.borrow_mut().insert(identity.clone());
        }
        result
    }
}

fn journal_stamp(dir: &Path) -> io::Result<JournalStamp> {
    let mut stamp = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == journal::ACTIVE_FILE || journal::generation_stamp(&name).is_some() {
            let meta = fs::symlink_metadata(entry.path())?;
            stamp.push((name, meta.ino(), meta.len(), meta.mtime(), meta.mtime_nsec()));
        }
    }
    stamp.sort();
    Ok(stamp)
}

fn worker(dir: &Path) -> io::Result<()> {
    if !automatic::read(dir)?.enabled {
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    require_supported_data_volume(dir)?;
    let Some(_worker) =
        IdentityLock::try_acquire(&dir.join(automatic::DIRECTORY).join("worker-lock"), "worker")
            .map_err(error)?
    else {
        return Ok(());
    };
    let provider = Provider::new()?;
    if let ProcessRead::Present(process) = provider.read(std::process::id() as i32)? {
        automatic::write_worker_identity(dir, &process.kernel)?;
    } else {
        return Err(error("worker identity unavailable"));
    }
    let surveyor = Arc::new(LiveSurveyor::new(dir));
    #[cfg(target_os = "macos")]
    let signaller: Box<dyn Signaller> = Box::new(KillSignaller);
    #[cfg(target_os = "linux")]
    let signaller: Box<dyn Signaller> = Box::new(LinuxPidfdSignaller::new()?);
    let executor = Executor::new(
        Deps {
            data_dir: dir.to_path_buf(),
            surveyor,
            provider: Box::new(Provider::new()?),
            signaller,
            timer: Arc::new(SystemTimer::new()),
        },
        Settings::default(),
    );
    let mut stamp = Vec::new();
    let mut records = Vec::new();
    let mut owners: Vec<(KernelIdentity, Liveness)> = Vec::new();
    let gone = RefCell::new(HashSet::new());
    let mut reconcile = 1_u32;
    let mut next_maintenance_check = Instant::now();
    let mut previous_policy = automatic::read(dir)?;
    let mut history: Vec<Value> = automatic::status(dir)
        .ok()
        .and_then(|status| status["last_report"]["recent_results"].as_array().cloned())
        .unwrap_or_default();
    loop {
        let policy = automatic::read(dir)?;
        if !policy.enabled {
            return Ok(());
        }
        if policy != previous_policy {
            reconcile = 16;
            previous_policy = policy;
        }
        if Instant::now() >= next_maintenance_check {
            journal::maintain_if_due(
                dir,
                &journal::retention::Policy::default(),
                clock::wall_ms(),
                &agentdust_core::platform::boot_identity()?,
            )
            .map_err(error)?;
            next_maintenance_check = Instant::now() + Duration::from_secs(60);
        }
        refresh_records(dir, &mut stamp, &mut records, &history)?;
        let identities = owner_identities(&records)?;
        gone.borrow_mut().retain(|identity| identities.contains(identity));
        let probe = OwnerProbe {
            provider: &provider,
            gone: &gone,
        };
        let current = current_owners(&records, &probe);
        if current.len() > 4096 {
            return Err(error("owner watch limit reached"));
        }
        if current != owners {
            reconcile = 16;
            owners = current;
        }
        if reconcile > 0 {
            let pending = reconcile_once(&executor, dir, &probe, &mut history)?;
            reconcile = if pending { reconcile - 1 } else { 0 };
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn refresh_records(
    dir: &Path,
    stamp: &mut Vec<(String, u64, u64, i64, i64)>,
    records: &mut Vec<journal::Record>,
    history: &[Value],
) -> io::Result<()> {
    let fresh_stamp = journal_stamp(dir)?;
    if fresh_stamp == *stamp {
        return Ok(());
    }
    let report = Journal::new(dir).read().map_err(error)?;
    if report.unsupported_version || report.skipped_lines() != 0 {
        automatic::write_report(
            dir,
            &json!({"wall_ms": clock::wall_ms(), "state": "unavailable", "reason": "journal_unavailable", "recent_results": history}),
        )?;
        return Err(error("journal unavailable"));
    }
    *records = report.records;
    *stamp = fresh_stamp;
    Ok(())
}

fn owner_identities(records: &[journal::Record]) -> io::Result<HashSet<KernelIdentity>> {
    let identities: HashSet<_> = records
        .iter()
        .filter_map(|record| {
            record
                .agent_identity
                .as_ref()
                .map(|identity| identity.kernel(&record.boot))
        })
        .collect();
    if identities.len() > 4096 {
        return Err(error("owner watch limit reached"));
    }
    Ok(identities)
}

fn current_owners(
    records: &[journal::Record],
    probe: &impl LivenessProbe,
) -> Vec<(KernelIdentity, Liveness)> {
    let mut current = Vec::new();
    for scope in session::scopes(records, probe) {
        for owner in scope.owner_states() {
            if !current.iter().any(|(identity, _)| identity == &owner.identity) {
                current.push((owner.identity, owner.liveness));
            }
        }
    }
    current
}

fn reconcile_once(
    executor: &Executor,
    dir: &Path,
    probe: &impl LivenessProbe,
    history: &mut Vec<Value>,
) -> io::Result<bool> {
    let guard = PolicyGuard::acquire(dir)?;
    automatic::prune_attempts(dir, probe)?;
    drop(guard);
    sweep(executor, dir, history)
}

fn sweep(executor: &Executor, dir: &Path, history: &mut Vec<Value>) -> io::Result<bool> {
    let findings = executor.surveyor().survey()?;
    let plan = format!("auto-{:x}", clock::wall_ms());
    let mut review = Vec::new();
    let mut review_count = 0;
    let mut attempted = 0;
    let mut pending = false;
    for finding in findings.iter().filter(|finding| {
        matches!(
            finding.class,
            Class::OwnedEnded | Class::Suspect | Class::LikelyOwned | Class::OwnedLive
        )
    }) {
        let model = executor.surveyor().describe(finding);
        if finding.class != Class::OwnedEnded {
            review_count += 1;
            if review.len() < 100 {
                review.push(json!({"item_id": model.item_id, "pid": model.pid, "class": model.class, "result": if finding.class == Class::Suspect { "approval_required" } else { "untouched" }}));
            }
            continue;
        }
        let Some(item) = PlanItem::from_finding(finding, model) else {
            continue;
        };
        let result = if attempted < 10 {
            match executor.execute_automatic(&plan, &item) {
                Ok(verdict) => {
                    if verdict.outcome == agentdust_core::apply::exec::Outcome::HandledElsewhere {
                        if history
                            .iter()
                            .any(|previous| previous["item_id"] == item.model.item_id)
                        {
                            continue;
                        }
                        json!({"item_id": item.model.item_id, "pid": item.model.pid, "result": "approval_required", "reason": "automatic_attempt_or_other_executor", "authorization": "none"})
                    } else {
                        attempted += 1;
                        json!({"item_id": item.model.item_id, "pid": item.model.pid, "result": verdict.outcome, "reason": verdict.reason, "authorization": "automatic_project_policy"})
                    }
                }
                Err(reason) => {
                    json!({"item_id": item.model.item_id, "pid": item.model.pid, "result": if reason == "kept" { "kept" } else { "skipped" }, "reason": reason, "authorization": "none"})
                }
            }
        } else {
            pending = true;
            continue;
        };
        let mut result = result;
        result["recorded_wall_ms"] = json!(clock::wall_ms());
        if let Some(previous) = history
            .iter()
            .position(|previous| previous["item_id"] == result["item_id"])
        {
            history.remove(previous);
        }
        history.push(result);
        if history.len() > 100 {
            history.remove(0);
        }
    }
    automatic::write_report(
        dir,
        &json!({"wall_ms": clock::wall_ms(), "state": "ready", "recent_results": history, "review": review, "review_truncated": review_count > 100, "pending": pending}),
    )?;
    Ok(pending)
}

#[cfg(all(test, target_os = "macos"))]
mod launchd_tests {
    use super::*;

    struct Registration {
        dir: PathBuf,
        service: String,
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            let _ = launchctl(&["bootout", &self.service]);
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agentdust-auto-{label}-{}-{}",
            std::process::id(),
            clock::wall_ms()
        ));
        safe_open::ensure_dir(&dir).unwrap();
        dir
    }

    #[test]
    fn batch_enable_canonicalizes_and_deduplicates_before_consent() {
        let dir = scratch("batch");
        let alias = dir.join("alias");
        std::os::unix::fs::symlink(&dir, &alias).unwrap();
        let request = enable_request(&["enable", "--yes", dir.to_str().unwrap(), alias.to_str().unwrap()])
            .unwrap()
            .unwrap();
        assert!(request.unattended);
        assert_eq!(request.projects, vec![fs::canonicalize(&dir).unwrap()]);
        assert!(
            enable_request(&["enable", dir.to_str().unwrap()])
                .unwrap()
                .is_some_and(|r| !r.unattended)
        );
        assert!(enable_request(&["enable", "--yes", dir.to_str().unwrap(), "/"]).is_err());
        assert!(enable_request(&["enable", "--yes"]).is_err());
        assert!(enable_request(&["enable", "--unexpected", dir.to_str().unwrap()]).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn project_discovery_is_immediate_and_never_follows_git_metadata_or_symlinks() {
        let dir = scratch("discovery");
        let outside = scratch("outside");
        fs::create_dir(outside.join(".git")).unwrap();
        let repo = dir.join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let worktree = dir.join("worktree");
        fs::create_dir(&worktree).unwrap();
        fs::write(worktree.join(".git"), format!("gitdir: {}", outside.display())).unwrap();
        fs::create_dir_all(dir.join("plain/nested/.git")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("external-link")).unwrap();
        let request = enable_request(&["enable", "--projects", dir.to_str().unwrap()])
            .unwrap()
            .unwrap();
        let mut expected: Vec<_> = [dir.clone(), repo, worktree]
            .iter()
            .map(|p| fs::canonicalize(p).unwrap())
            .collect();
        expected.sort();
        assert_eq!(request.projects, expected);
        fs::remove_dir_all(dir).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn discovery_rejects_more_than_128_scopes() {
        let dir = scratch("discovery-limit");
        for i in 0..128 {
            fs::create_dir_all(dir.join(format!("repo-{i}/.git"))).unwrap();
        }
        assert!(enable_request(&["enable", "--projects", dir.to_str().unwrap()]).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn final_scope_limit_preserves_policy_and_deduplicates_existing_scopes() {
        let dir = scratch("limit");
        let install_secret = secret::load_or_create(&dir).unwrap();
        let mut guard = PolicyGuard::acquire(&dir).unwrap();
        let projects: Vec<_> = (0..128).map(|i| PathBuf::from(format!("/project/{i}"))).collect();
        add_projects(&mut guard, &projects, &install_secret).unwrap();
        let kept = KernelIdentity {
            boot_session_uuid: "fixture-boot".into(),
            pid: 12345,
            start_time_us: 42,
            uid: unsafe { libc::geteuid() },
        };
        guard.policy.keep.push(kept.clone());
        guard.write().unwrap();
        let before = fs::read(dir.join("automatic/policy.json")).unwrap();
        add_projects(&mut guard, &projects, &install_secret).unwrap();
        assert_eq!(guard.policy.projects.len(), 128);
        assert_eq!(guard.policy.keep, vec![kept]);
        assert!(add_projects(&mut guard, &[PathBuf::from("/another")], &install_secret).is_err());
        assert_eq!(before, fs::read(dir.join("automatic/policy.json")).unwrap());
        drop(guard);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn launchagent_xml_escapes_paths_and_parses_as_a_plist() {
        let dir = scratch("plist");
        let path = dir.join("test.plist");
        let plist = render_plist(
            "com.agentdust.test",
            Path::new("/tmp/a & b/'agentdust'"),
            Path::new("/tmp/c <d> \"e\""),
        )
        .unwrap();
        assert!(
            plist.contains("&amp;")
                && plist.contains("&lt;")
                && plist.contains("&quot;")
                && plist.contains("&apos;")
        );
        fs::write(&path, plist).unwrap();
        let output = Command::new("/usr/bin/plutil")
            .arg("-lint")
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[ignore = "requires the user's GUI launchd domain and an explicitly built test binary"]
    fn isolated_launchagent_starts_and_unloads_the_worker() {
        let binary = PathBuf::from(
            std::env::var_os("AGENTDUST_TEST_BINARY").expect("provide the built agentdust binary"),
        );
        let dir = scratch("launchd");
        let label = format!(
            "com.agentdust.automatic.test.{}.{}",
            std::process::id(),
            clock::wall_ms()
        );
        let registration = Registration {
            dir: dir.clone(),
            service: format!("{}/{label}", domain()),
        };
        let mut policy = PolicyGuard::acquire(&dir).unwrap();
        policy.policy.enabled = true;
        policy.policy.projects.push("a".repeat(64).try_into().unwrap());
        policy.write().unwrap();
        drop(policy);
        let path = dir.join("worker.plist");
        fs::write(&path, render_plist(&label, &binary, &dir).unwrap()).unwrap();
        launchctl(&["bootstrap", &domain(), path.to_str().unwrap()]).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while !automatic::status(&dir).unwrap()["worker_running"]
            .as_bool()
            .unwrap()
        {
            assert!(
                std::time::Instant::now() < deadline,
                "launchd did not start the isolated worker"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        let mut policy = PolicyGuard::acquire(&dir).unwrap();
        policy.policy.enabled = false;
        policy.write().unwrap();
        drop(policy);
        launchctl(&["bootout", &registration.service]).unwrap();
        assert!(
            !automatic::status(&dir).unwrap()["worker_running"]
                .as_bool()
                .unwrap()
        );
        assert!(!dir.join("audit.log").exists());
        drop(registration);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod systemd_tests {
    use super::{
        read_agentdust_unit, remove_agentdust_unit, render_systemd_unit, systemd_quote, write_agentdust_unit,
    };
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentdust-systemd-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    #[test]
    fn systemd_arguments_escape_unit_syntax() {
        assert_eq!(systemd_quote("a % b\\c\"d\ne"), "\"a %% b\\\\c\\\"d\\ne\"");
        let unit = render_systemd_unit(
            Path::new("/home/user/Agent Dust/%agentdust"),
            Path::new("/home/user/data dir"),
        )
        .unwrap();
        assert!(unit.contains("# Managed by AgentDust\n"));
        assert!(unit.contains("ExecStart=\"/home/user/Agent Dust/%%agentdust\" auto worker\n"));
        assert!(unit.contains("Environment=AGENTDUST_DATA_DIR=\"/home/user/data dir\"\n"));
        assert!(unit.contains("Restart=on-failure\n"));
    }

    #[test]
    fn systemd_unit_updates_only_agentdust_owned_files() {
        let dir = scratch("owned");
        let path = dir.join("agentdust-automatic.service");
        let first = "# Managed by AgentDust\n[Service]\nExecStart=/old/agentdust auto worker\n";
        let second = "# Managed by AgentDust\n[Service]\nExecStart=/new/agentdust auto worker\n";
        write_agentdust_unit(&path, first).unwrap();
        assert_eq!(read_agentdust_unit(&path).unwrap().as_deref(), Some(first));
        write_agentdust_unit(&path, second).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), second);
        remove_agentdust_unit(&path).unwrap();
        assert!(!path.exists());

        fs::write(&path, "[Service]\nExecStart=/user/service\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(write_agentdust_unit(&path, first).is_err());
        assert!(remove_agentdust_unit(&path).is_err());
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[Service]\nExecStart=/user/service\n"
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
