use std::cell::RefCell;
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, IsTerminal, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::Arc;
use std::time::Duration;

use agentdust_core::ancestry::find_agent;
use agentdust_core::apply::exec::{Deps, Executor, Settings};
use agentdust_core::apply::lock::IdentityLock;
use agentdust_core::apply::signal::KillSignaller;
use agentdust_core::apply::timer::SystemTimer;
use agentdust_core::automatic::{self, PolicyGuard};
use agentdust_core::class::Class;
use agentdust_core::darwin::DarwinProvider;
use agentdust_core::identity::KernelIdentity;
use agentdust_core::journal::{self, Journal};
use agentdust_core::live::LiveSurveyor;
use agentdust_core::plan::PlanItem;
use agentdust_core::provider::{ProcessProvider, ProcessRead};
use agentdust_core::session::{self, Liveness, LivenessProbe, ProviderLiveness};
use agentdust_core::{clock, cwd, paths, safe_open, secret};
use serde_json::{Value, json};

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
    if let Some(request) = &enable {
        let existing = automatic::read(&dir)?;
        if !existing.projects.is_empty() {
            merged_projects(
                &existing.projects,
                &request.projects,
                &secret::load_existing(&dir).map_err(error)?,
            )?;
        }
    }
    let provider = DarwinProvider::new()?;
    if let Some(request) = &enable {
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
    if !enable.as_ref().is_some_and(|request| request.unattended) {
        human_consent(&provider, args)?;
    }
    safe_open::ensure_dir(&dir).map_err(error)?;
    let mut guard = PolicyGuard::acquire(&dir)?;
    if let Some(request) = enable {
        let install_secret = secret::load_or_create(&dir).map_err(error)?;
        add_projects(&mut guard, &request.projects, &install_secret)?;
        drop(guard);
        if let Err(err) = install_worker(&dir) {
            let mut guard = PolicyGuard::acquire(&dir)?;
            guard.policy.enabled = false;
            guard.write()?;
            return Err(err);
        }
        println!(
            "Automatic cleanup enabled for {} selected directories. Scope is exact, not recursive.",
            request.projects.len()
        );
        return Ok(());
    }
    match args {
        ["resume"] => {
            if guard.policy.projects.is_empty() {
                return Err(error("enable a project directory first"));
            }
            guard.policy.enabled = true;
            guard.write()?;
            drop(guard);
            if let Err(err) = install_worker(&dir) {
                let mut guard = PolicyGuard::acquire(&dir)?;
                guard.policy.enabled = false;
                guard.write()?;
                return Err(err);
            }
            println!("Automatic cleanup resumed for the previously enabled directories.");
        }
        ["disable", project] => {
            let project = fs::canonicalize(project)?;
            let install_secret = secret::load_existing(&dir).map_err(error)?;
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
        }
        ["pause"] | ["disable"] => {
            guard.policy.enabled = false;
            if args == ["disable"] {
                guard.policy.projects.clear();
            }
            guard.write()?;
            drop(guard);
            remove_worker()?;
            println!("Automatic cleanup is off. Kept process identities remain protected.");
        }
        ["keep", pid] => {
            let pid: i32 = pid.parse().map_err(error)?;
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
        }
        ["unkeep", pid] => {
            let pid: i32 = pid.parse().map_err(error)?;
            guard.policy.keep.retain(|identity| identity.pid != pid);
            guard.write()?;
            println!(
                "Removed keeps for PID {pid}. Future cleanup still requires fresh ownership and identity checks."
            );
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn human_consent(provider: &DarwinProvider, args: &[&str]) -> io::Result<()> {
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

fn plist_path() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| error("HOME is not set"))?;
    let home = PathBuf::from(home);
    if !home.is_absolute() {
        return Err(error("HOME must be absolute"));
    }
    Ok(home.join("Library/LaunchAgents").join(format!("{LABEL}.plist")))
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

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

fn domain() -> String {
    format!("gui/{}", unsafe { libc::geteuid() })
}

fn render_plist(label: &str, exe: &Path, dir: &Path) -> io::Result<String> {
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>{label}</string><key>ProgramArguments</key><array><string>{}</string><string>auto</string><string>worker</string></array><key>EnvironmentVariables</key><dict><key>AGENTDUST_DATA_DIR</key><string>{}</string></dict><key>RunAtLoad</key><true/><key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict><key>ThrottleInterval</key><integer>10</integer></dict></plist>",
        xml(exe.to_str().ok_or_else(|| error("binary path is not UTF-8"))?),
        xml(dir.to_str().ok_or_else(|| error("data directory is not UTF-8"))?),
    );
    Ok(plist)
}

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

type JournalStamp = Vec<(String, u64, u64, i64, i64)>;

struct OwnerProbe<'a> {
    provider: &'a DarwinProvider,
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
    let Some(_worker) =
        IdentityLock::try_acquire(&dir.join(automatic::DIRECTORY).join("worker-lock"), "worker")
            .map_err(error)?
    else {
        return Ok(());
    };
    let provider = DarwinProvider::new()?;
    if let ProcessRead::Present(process) = provider.read(std::process::id() as i32)? {
        automatic::write_worker_identity(dir, &process.kernel)?;
    } else {
        return Err(error("worker identity unavailable"));
    }
    let surveyor = Arc::new(LiveSurveyor::new(dir));
    let executor = Executor::new(
        Deps {
            data_dir: dir.to_path_buf(),
            surveyor,
            provider: Box::new(DarwinProvider::new()?),
            signaller: Box::new(KillSignaller),
            timer: Arc::new(SystemTimer::new()),
        },
        Settings::default(),
    );
    let mut stamp = Vec::new();
    let mut records = Vec::new();
    let mut owners: Vec<(KernelIdentity, Liveness)> = Vec::new();
    let gone = RefCell::new(HashSet::new());
    let mut reconcile = 1_u32;
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
        let fresh_stamp = journal_stamp(dir)?;
        if fresh_stamp != stamp {
            let report = Journal::new(dir).read().map_err(error)?;
            if report.unsupported_version || report.skipped_lines() != 0 {
                automatic::write_report(
                    dir,
                    &json!({"wall_ms": clock::wall_ms(), "state": "unavailable", "reason": "journal_unavailable", "recent_results": history}),
                )?;
                return Err(error("journal unavailable"));
            }
            records = report.records;
            stamp = fresh_stamp;
        }
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
        gone.borrow_mut().retain(|identity| identities.contains(identity));
        let probe = OwnerProbe {
            provider: &provider,
            gone: &gone,
        };
        let scopes = session::scopes(&records, &probe);
        let mut current = Vec::new();
        for scope in scopes {
            for owner in scope.owner_states() {
                if !current.iter().any(|(identity, _)| identity == &owner.identity) {
                    current.push((owner.identity, owner.liveness));
                }
            }
        }
        if current.len() > 4096 {
            return Err(error("owner watch limit reached"));
        }
        if current != owners {
            reconcile = 16;
            owners = current;
        }
        if reconcile > 0 {
            let guard = PolicyGuard::acquire(dir)?;
            automatic::prune_attempts(dir, &probe)?;
            drop(guard);
            let pending = sweep(&executor, dir, &mut history)?;
            reconcile = if pending { reconcile - 1 } else { 0 };
        }
        std::thread::sleep(Duration::from_secs(1));
    }
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

#[cfg(test)]
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
