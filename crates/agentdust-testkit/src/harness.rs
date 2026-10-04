use std::fs::{self, DirBuilder, File};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use agentdust_core::darwin::{self, DarwinProvider};
use agentdust_core::identity::ProcessIdentity;
use agentdust_core::provider::{ProcessProvider, ProcessRead};
use agentdust_core::revalidate::{Revalidation, revalidate};

use crate::report::{self, Report, ReportError};
use crate::spec::ProcSpec;
use crate::wait_until;

static NEXT_HARNESS: AtomicU64 = AtomicU64::new(1);
const READY: Duration = Duration::from_secs(60);
const GONE: Duration = Duration::from_secs(60);
const REPARENTED: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Term,
    Kill,
}

impl Signal {
    fn number(self) -> i32 {
        match self {
            Self::Term => libc::SIGTERM,
            Self::Kill => libc::SIGKILL,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalEvent {
    pub pid: i32,
    pub signal: Signal,
    pub at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcHandle {
    harness: u64,
    index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    pub parent: ProcHandle,
    pub children: Vec<ProcHandle>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Report(#[from] ReportError),
    #[error("the fixture exited with {status} before it reported: {stderr}")]
    ExitedBeforeReady { status: ExitStatus, stderr: String },
    #[error("the fixture did not report within {0:?}")]
    NotReady(Duration),
    #[error("pid {pid} could not be identified: {read:?}")]
    Unidentified { pid: i32, read: ProcessRead },
    #[error("the handle belongs to another harness")]
    ForeignHandle,
    #[error("pid {0} was not spawned by this harness")]
    Unregistered(i32),
    #[error("pid {0} is not running")]
    NotRunning(i32),
    #[error("pid {pid} is no longer the process that was registered: {found:?}")]
    IdentityChanged { pid: i32, found: Revalidation },
    #[error("kill of pid {pid} failed: {source}")]
    Signal { pid: i32, source: io::Error },
    #[error("spawn starts one process, use spawn_tree for a spec with children")]
    TreeRequested,
    #[error("pid {child} is not a child of pid {parent} that this harness started: {reason}")]
    UnexpectedChild {
        parent: i32,
        child: i32,
        reason: &'static str,
    },
    #[error("pid {0} was not started by this harness as its own child, so it cannot be orphaned")]
    NotADirectChild(i32),
    #[error("the children of pid {0} were not reparented to launchd in time")]
    NotReparented(i32),
    #[error("processes survived teardown: {0:?}")]
    Survivors(Vec<i32>),
}

struct Entry {
    pid: i32,
    identity: ProcessIdentity,
    report: Report,
    child: Option<Child>,
    parent: Option<usize>,
}

impl Entry {
    fn direct(pid: i32, identity: ProcessIdentity, report: Report, child: Child) -> Self {
        Self {
            pid,
            identity,
            report,
            child: Some(child),
            parent: None,
        }
    }

    fn adopted(pid: i32, identity: ProcessIdentity, report: Report, parent: usize) -> Self {
        Self {
            pid,
            identity,
            report,
            child: None,
            parent: Some(parent),
        }
    }
}

struct Launched {
    child: Option<Child>,
    report_path: PathBuf,
    stderr_path: PathBuf,
}

impl Launched {
    fn child(&mut self) -> &mut Child {
        self.child.as_mut().expect("the child is only taken on success")
    }

    fn into_child(mut self) -> Child {
        self.child.take().expect("the child is only taken once")
    }
}

impl Drop for Launched {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub struct Harness {
    id: u64,
    fixture: PathBuf,
    scratch: PathBuf,
    provider: DarwinProvider,
    entries: Vec<Entry>,
    log: Vec<SignalEvent>,
    launched: u32,
    ready: Duration,
    torn_down: bool,
}

impl Harness {
    pub fn new(fixture: impl AsRef<Path>) -> Result<Self, Error> {
        let fixture = fixture.as_ref().canonicalize()?;
        let provider = DarwinProvider::new()?;
        let id = NEXT_HARNESS.fetch_add(1, Ordering::Relaxed);
        let scratch = std::env::temp_dir().join(format!("agentdust-harness-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&scratch);
        DirBuilder::new().mode(0o700).create(&scratch)?;
        Ok(Self {
            id,
            fixture,
            scratch,
            provider,
            entries: Vec::new(),
            log: Vec::new(),
            launched: 0,
            ready: READY,
            torn_down: false,
        })
    }

    pub fn spawn(&mut self, spec: &ProcSpec) -> Result<ProcHandle, Error> {
        if spec.spawn > 0 {
            return Err(Error::TreeRequested);
        }
        Ok(self.spawn_tree(spec)?.parent)
    }

    pub fn spawn_tree(&mut self, spec: &ProcSpec) -> Result<Tree, Error> {
        let mut launched = self.launch(spec)?;
        let report = self.await_report(&mut launched)?;
        let pid = launched.child().id() as i32;
        let identity = self.identify(pid)?;
        let spec = spec.clone().report_file(&launched.report_path);
        self.entries
            .push(Entry::direct(pid, identity, report, launched.into_child()));
        let parent = self.entries.len() - 1;
        let mut children = Vec::with_capacity(spec.spawn);
        for index in 1..=spec.spawn {
            let path = spec.child(index).report_file.unwrap_or_default();
            let report = report::read(&path)?.ok_or(ReportError::Malformed(
                "the parent reported before one of its children",
            ))?;
            children.push(self.adopt(parent, report)?);
        }
        Ok(Tree {
            parent: self.handle(parent),
            children,
        })
    }

    pub fn orphan(&mut self, handle: ProcHandle) -> Result<(), Error> {
        let index = handle.index;
        let pid = self.entry(handle)?.pid;
        if self.entries[index].child.is_none() {
            return Err(Error::NotADirectChild(pid));
        }
        self.deliver(pid, Signal::Kill)?;
        if let Some(child) = self.entries[index].child.as_mut() {
            child.wait()?;
        }
        let orphans: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|entry| entry.parent == Some(index))
            .collect();
        let reparented = wait_until(
            || {
                orphans.iter().all(|entry| {
                    let boot = &entry.identity.kernel.boot_session_uuid;
                    match darwin::process_info(entry.pid, boot) {
                        Ok(Some(info)) => info.ppid == 1,
                        Ok(None) => true,
                        Err(_) => false,
                    }
                })
            },
            REPARENTED,
        );
        if reparented {
            Ok(())
        } else {
            Err(Error::NotReparented(pid))
        }
    }

    pub fn signal(&mut self, handle: ProcHandle, signal: Signal) -> Result<(), Error> {
        let pid = self.entry(handle)?.pid;
        self.deliver(pid, signal)
    }

    pub fn signal_log(&self) -> &[SignalEvent] {
        &self.log
    }

    pub fn pid(&self, handle: ProcHandle) -> i32 {
        self.known(handle).pid
    }

    pub fn identity(&self, handle: ProcHandle) -> &ProcessIdentity {
        &self.known(handle).identity
    }

    pub fn report(&self, handle: ProcHandle) -> &Report {
        &self.known(handle).report
    }

    pub fn provider(&self) -> &DarwinProvider {
        &self.provider
    }

    pub fn scratch_dir(&self) -> &Path {
        &self.scratch
    }

    pub fn revalidate(&self, handle: ProcHandle) -> Revalidation {
        revalidate(&self.known(handle).identity, &self.provider)
    }

    pub fn ppid(&self, handle: ProcHandle) -> Result<i32, Error> {
        let entry = self.entry(handle)?;
        self.require_same_process(entry.pid, &entry.identity)?;
        let boot = &entry.identity.kernel.boot_session_uuid;
        match darwin::process_info(entry.pid, boot)? {
            Some(info) => Ok(info.ppid),
            None => Err(Error::NotRunning(entry.pid)),
        }
    }

    pub fn shutdown(mut self) -> Result<(), Error> {
        let survivors = self.teardown();
        if survivors.is_empty() {
            Ok(())
        } else {
            Err(Error::Survivors(survivors))
        }
    }

    fn handle(&self, index: usize) -> ProcHandle {
        ProcHandle {
            harness: self.id,
            index,
        }
    }

    fn entry(&self, handle: ProcHandle) -> Result<&Entry, Error> {
        match self.entries.get(handle.index) {
            Some(entry) if handle.harness == self.id => Ok(entry),
            _ => Err(Error::ForeignHandle),
        }
    }

    fn known(&self, handle: ProcHandle) -> &Entry {
        self.entry(handle)
            .expect("a handle can only be used with the harness that returned it")
    }

    fn launch(&mut self, spec: &ProcSpec) -> Result<Launched, Error> {
        self.launched += 1;
        let report_path = self.scratch.join(format!("{}.report", self.launched));
        let stderr_path = self.scratch.join(format!("{}.stderr", self.launched));
        let spec = spec.clone().report_file(&report_path);
        let child = Command::new(&self.fixture)
            .args(spec.args())
            .envs(spec.env.iter().map(|(name, value)| (name, value)))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(File::create(&stderr_path)?)
            .spawn()?;
        Ok(Launched {
            child: Some(child),
            report_path,
            stderr_path,
        })
    }

    fn await_report(&self, launched: &mut Launched) -> Result<Report, Error> {
        let deadline = Instant::now() + self.ready;
        loop {
            if let Some(report) = report::read(&launched.report_path)? {
                return Ok(report);
            }
            if let Some(status) = launched.child().try_wait()? {
                if let Some(report) = report::read(&launched.report_path)? {
                    return Ok(report);
                }
                let stderr = fs::read_to_string(&launched.stderr_path).unwrap_or_default();
                return Err(Error::ExitedBeforeReady { status, stderr });
            }
            if Instant::now() >= deadline {
                return Err(Error::NotReady(self.ready));
            }
            thread::sleep(POLL);
        }
    }

    fn identify(&self, pid: i32) -> Result<ProcessIdentity, Error> {
        match self.provider.read(pid)? {
            ProcessRead::Present(identity) => Ok(identity),
            read => Err(Error::Unidentified { pid, read }),
        }
    }

    fn adopt(&mut self, parent: usize, report: Report) -> Result<ProcHandle, Error> {
        let parent_pid = self.entries[parent].pid;
        let pid = report.pid;
        let unexpected = |reason| Error::UnexpectedChild {
            parent: parent_pid,
            child: pid,
            reason,
        };
        if report.ppid != parent_pid {
            return Err(unexpected("its report names another parent"));
        }
        let identity = self.identify(pid)?;
        if identity.evidence.exe_path.canonicalize().ok().as_deref() != Some(self.fixture.as_path()) {
            return Err(unexpected("it is not running the fixture binary"));
        }
        let boot = &identity.kernel.boot_session_uuid;
        match darwin::process_info(pid, boot)? {
            Some(info) if info.ppid == parent_pid => {}
            _ => return Err(unexpected("its live parent is not the registered parent")),
        }
        self.entries.push(Entry::adopted(pid, identity, report, parent));
        Ok(self.handle(self.entries.len() - 1))
    }

    fn require_same_process(&self, pid: i32, identity: &ProcessIdentity) -> Result<(), Error> {
        match revalidate(identity, &self.provider) {
            Revalidation::Match => Ok(()),
            Revalidation::Gone => Err(Error::NotRunning(pid)),
            found => Err(Error::IdentityChanged { pid, found }),
        }
    }

    fn deliver(&mut self, pid: i32, signal: Signal) -> Result<(), Error> {
        let index = (pid > 0)
            .then(|| self.entries.iter().rposition(|entry| entry.pid == pid))
            .flatten()
            .ok_or(Error::Unregistered(pid))?;
        self.require_same_process(pid, &self.entries[index].identity)?;
        // SAFETY: pid is positive, was registered by this harness and was just revalidated.
        let rc = unsafe { libc::kill(pid, signal.number()) };
        if rc != 0 {
            return Err(Error::Signal {
                pid,
                source: io::Error::last_os_error(),
            });
        }
        self.log.push(SignalEvent {
            pid,
            signal,
            at: Instant::now(),
        });
        Ok(())
    }

    fn teardown(&mut self) -> Vec<i32> {
        self.torn_down = true;
        for index in (0..self.entries.len()).rev() {
            let pid = self.entries[index].pid;
            if self.deliver(pid, Signal::Kill).is_err()
                && let Some(child) = self.entries[index].child.as_mut()
            {
                let _ = child.kill();
            }
        }
        for entry in &mut self.entries {
            if let Some(child) = entry.child.as_mut() {
                let _ = child.wait();
            }
        }
        let mut survivors = Vec::new();
        wait_until(
            || {
                survivors = self
                    .entries
                    .iter()
                    .filter(|entry| {
                        !matches!(
                            revalidate(&entry.identity, &self.provider),
                            Revalidation::Gone | Revalidation::Changed(_)
                        )
                    })
                    .map(|entry| entry.pid)
                    .collect();
                survivors.is_empty()
            },
            GONE,
        );
        survivors
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let survivors = if self.torn_down {
            Vec::new()
        } else {
            self.teardown()
        };
        let _ = fs::remove_dir_all(&self.scratch);
        if !survivors.is_empty() {
            let message = format!("fixture processes survived teardown: {survivors:?}");
            if thread::panicking() {
                eprintln!("{message}");
            } else {
                panic!("{message}");
            }
        }
    }
}

#[cfg(test)]
mod tests;
