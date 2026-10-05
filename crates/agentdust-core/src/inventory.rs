use std::collections::BTreeSet;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use thiserror::Error;

use crate::ancestry;
use crate::identity::KernelIdentity;
use crate::journal::SessionTagKey;
use crate::procargs;
use crate::secret::Secret;
use crate::tag::{self, ENV_NAME};

pub const IDLE_SAMPLE_GAP: Duration = Duration::from_secs(2);
pub const LAUNCHCTL: &str = "/bin/launchctl";
pub const LAUNCHCTL_TIMEOUT: Duration = Duration::from_secs(10);
const LIST_HEADER: &str = "PID\tStatus\tLabel";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tag {
    Absent,
    Keyed(SessionTagKey),
    Unkeyed,
    Unreadable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cpu {
    pub first_ns: Option<u64>,
    pub later_ns: Option<u64>,
}

impl Cpu {
    pub fn idle(&self) -> bool {
        matches!((self.first_ns, self.later_ns), (Some(first), Some(later)) if first == later)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawIdentity {
    pub kernel: KernelIdentity,
    pub exe_path: Option<PathBuf>,
    pub ppid: i32,
    pub pgid: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawProcess {
    pub identity: RawIdentity,
    pub tag: Tag,
    pub agent_script: bool,
    pub cpu: Cpu,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launchd {
    Known(BTreeSet<i32>),
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub taken_at_us: u64,
    pub processes: Vec<RawProcess>,
    pub launchd: Launchd,
}

pub trait ProcessSource {
    fn scan(&self) -> io::Result<Vec<RawProcess>>;

    fn cpu_time_ns(&self, identity: &KernelIdentity) -> io::Result<Option<u64>>;
}

pub trait LaunchdSource {
    fn pids(&self) -> io::Result<BTreeSet<i32>>;
}

pub trait LiveDetails {
    fn command(&self, identity: &KernelIdentity) -> Option<Vec<Vec<u8>>>;

    fn cwd(&self, identity: &KernelIdentity) -> Option<PathBuf>;
}

pub trait Clock {
    fn now_us(&self) -> u64;

    fn sleep(&self, duration: Duration);
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_us(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_micros() as u64)
            .unwrap_or(0)
    }

    fn sleep(&self, duration: Duration) {
        thread::sleep(duration);
    }
}

pub fn take(
    source: &dyn ProcessSource,
    launchd: &dyn LaunchdSource,
    clock: &dyn Clock,
    gap: Duration,
) -> io::Result<Snapshot> {
    let mut processes = source.scan()?;
    clock.sleep(gap);
    for process in &mut processes {
        if process.cpu.first_ns.is_some() {
            process.cpu.later_ns = source.cpu_time_ns(&process.identity.kernel).ok().flatten();
        }
    }
    let launchd = launchd.pids().map_or(Launchd::Unavailable, Launchd::Known);
    Ok(Snapshot {
        taken_at_us: clock.now_us(),
        processes,
        launchd,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgsView {
    pub tag: Tag,
    pub agent_script: bool,
}

pub fn read_args(buf: Option<&[u8]>, exe_path: Option<&Path>, secret: Option<&Secret>) -> ArgsView {
    let Some(buf) = buf else {
        return ArgsView {
            tag: Tag::Absent,
            agent_script: false,
        };
    };
    let tag = match procargs::parse(buf) {
        Err(_) => Tag::Unreadable,
        Ok(parsed) => match (procargs::env_value(&parsed, ENV_NAME), secret) {
            (None, _) => Tag::Absent,
            (Some(_), None) => Tag::Unkeyed,
            (Some(value), Some(secret)) => Tag::Keyed(tag::key_of(secret, value)),
        },
    };
    let agent_script = exe_path.is_some_and(ancestry::is_node)
        && procargs::script_argument(buf)
            .ok()
            .flatten()
            .is_some_and(ancestry::script_names_agent);
    ArgsView { tag, agent_script }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ListError {
    #[error("the output does not start with the PID, Status and Label header")]
    NoHeader,
    #[error("line {line} is not a PID, a status and a label")]
    BadRow { line: usize },
}

pub fn parse_launchctl_list(text: &str) -> Result<BTreeSet<i32>, ListError> {
    let mut lines = text.lines();
    if lines.next() != Some(LIST_HEADER) {
        return Err(ListError::NoHeader);
    }
    let mut pids = BTreeSet::new();
    for (index, line) in lines.enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let bad = ListError::BadRow { line: index + 2 };
        let columns: Vec<&str> = line.split('\t').collect();
        let [pid, status, _label] = columns[..] else {
            return Err(bad);
        };
        if status != "-" && status.parse::<i32>().is_err() {
            return Err(bad);
        }
        if pid == "-" {
            continue;
        }
        match pid.parse::<i32>() {
            Ok(pid) if pid > 0 && pid.to_string() == *columns[0] => {
                pids.insert(pid);
            }
            _ => return Err(bad),
        }
    }
    Ok(pids)
}

pub struct LaunchctlList;

impl LaunchdSource for LaunchctlList {
    fn pids(&self) -> io::Result<BTreeSet<i32>> {
        let text = run_launchctl()?;
        parse_launchctl_list(&text).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
    }
}

fn run_launchctl() -> io::Result<String> {
    let mut child = Command::new(LAUNCHCTL)
        .arg("list")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = sender.send(stdout.read_to_end(&mut bytes).map(|_| bytes));
    });
    match receiver.recv_timeout(LAUNCHCTL_TIMEOUT) {
        Ok(Ok(bytes)) => {
            if !child.wait()?.success() {
                return Err(io::Error::other("launchctl list failed"));
            }
            String::from_utf8(bytes)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "launchctl list is not UTF-8"))
        }
        Ok(Err(err)) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(err)
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "launchctl list did not finish",
            ))
        }
    }
}
