use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::fs;
use std::io::{self, BufRead, BufReader, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::ancestry::AncestryProvider;
use crate::apply::signal::{SignalResult, Signaller, signalable};
use crate::identity::{KernelIdentity, ProcessIdentity, ProcessInfo};
use crate::inventory::{ArgsView, Cpu, LiveDetails, ProcessSource, RawIdentity, RawProcess, Tag};
use crate::provider::{ProcessProvider, ProcessRead};
use crate::revalidate::{Revalidation, revalidate};
use crate::secret::Secret;
use crate::tag::{self, CODEX_ENV_NAME, ENV_NAME};

const PROC: &str = "/proc";
const NANOS_PER_SECOND: u64 = 1_000_000_000;
const MICROS_PER_SECOND: u64 = 1_000_000;
const MAX_PROCESSES: usize = 32_768;
const MAX_STAT_BYTES: usize = 8 * 1024;
const MAX_STATUS_BYTES: usize = 64 * 1024;
const MAX_ENV_BYTES: u64 = 64 * 1024;
const MAX_ENV_VALUE_BYTES: usize = 1024;
const MAX_CMDLINE_BYTES: usize = 256 * 1024;
const MAX_PROC_NET_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Default)]
struct SessionEnv {
    claude: Vec<Vec<u8>>,
    codex: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcStat {
    state: u8,
    ppid: i32,
    pgid: i32,
    user_ticks: u64,
    system_ticks: u64,
    start_ticks: u64,
}

fn parse_number<T: std::str::FromStr>(value: &[u8]) -> io::Result<T> {
    std::str::from_utf8(value)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "proc value is not ASCII"))?
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid proc value"))
}

fn parse_stat(bytes: &[u8]) -> io::Result<ProcStat> {
    let close = bytes
        .iter()
        .rposition(|byte| *byte == b')')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid proc stat command"))?;
    let fields: Vec<&[u8]> = bytes
        .get(close + 1..)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid proc stat fields"))?
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|field| !field.is_empty())
        .collect();
    if fields.len() <= 19 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "short proc stat"));
    }
    if fields[0].len() != 1 || !fields[0][0].is_ascii() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid proc stat state",
        ));
    }
    Ok(ProcStat {
        state: fields[0][0],
        ppid: parse_number(fields[1])?,
        pgid: parse_number(fields[2])?,
        user_ticks: parse_number(fields[11])?,
        system_ticks: parse_number(fields[12])?,
        start_ticks: parse_number(fields[19])?,
    })
}

fn process_uid(proc_root: &Path, pid: i32) -> io::Result<u32> {
    let status = read_bounded(proc_root.join(pid.to_string()).join("status"), MAX_STATUS_BYTES)?;
    parse_uid_status(&status)
}

fn parse_uid_status(status: &[u8]) -> io::Result<u32> {
    let line = status
        .split(|byte| *byte == b'\n')
        .find(|line| line.starts_with(b"Uid:"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "proc status has no Uid field"))?;
    let fields: Vec<&[u8]> = line[4..]
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|field| !field.is_empty())
        .collect();
    if fields.len() != 4 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "proc status has a short Uid field",
        ));
    }
    let uids = fields
        .iter()
        .map(|field| parse_number(field))
        .collect::<io::Result<Vec<u32>>>()?;
    if uids.windows(2).any(|pair| pair[0] != pair[1]) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "process credentials are mixed",
        ));
    }
    Ok(uids[1])
}

fn process_info(
    proc_root: &Path,
    pid: i32,
    boot_id: &str,
    boot_epoch_us: u64,
    ticks_per_second: u64,
) -> io::Result<Option<ProcessInfo>> {
    match read_info(proc_root, pid, boot_id, boot_epoch_us, ticks_per_second) {
        Ok(info) => Ok(Some(info)),
        Err(err) if missing(&err) => Ok(None),
        Err(err) => Err(err),
    }
}

fn read_info(
    proc_root: &Path,
    pid: i32,
    boot_id: &str,
    boot_epoch_us: u64,
    ticks_per_second: u64,
) -> io::Result<ProcessInfo> {
    if pid <= 0 || ticks_per_second == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid process identity input",
        ));
    }
    let stat = parse_stat(&read_bounded(
        proc_root.join(pid.to_string()).join("stat"),
        MAX_STAT_BYTES,
    )?)?;
    if matches!(stat.state, b'Z' | b'X') {
        return Err(io::Error::from_raw_os_error(libc::ESRCH));
    }
    let uid = process_uid(proc_root, pid)?;
    let since_boot_us =
        u128::from(stat.start_ticks) * u128::from(MICROS_PER_SECOND) / u128::from(ticks_per_second);
    let start_time_us = boot_epoch_us.saturating_add(u64::try_from(since_boot_us).unwrap_or(u64::MAX));
    Ok(ProcessInfo {
        identity: KernelIdentity {
            boot_session_uuid: boot_id.to_owned(),
            pid,
            start_time_us,
            uid,
        },
        ppid: stat.ppid,
        pgid: stat.pgid,
    })
}

pub fn boot_session_uuid() -> io::Result<String> {
    boot_session_uuid_at(Path::new(PROC))
}

fn boot_session_uuid_at(proc_root: &Path) -> io::Result<String> {
    let id = fs::read_to_string(proc_root.join("sys/kernel/random/boot_id"))?;
    let id = id.trim();
    if id.len() != 36 || !id.bytes().all(|byte| byte.is_ascii_hexdigit() || byte == b'-') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid Linux boot ID",
        ));
    }
    Ok(id.to_owned())
}

fn boot_epoch_us_at(proc_root: &Path) -> io::Result<u64> {
    let stat = fs::read(proc_root.join("stat"))?;
    let line = stat
        .split(|byte| *byte == b'\n')
        .find(|line| line.starts_with(b"btime "))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "proc stat has no btime"))?;
    let seconds: u64 = parse_number(&line[6..])?;
    Ok(seconds.saturating_mul(MICROS_PER_SECOND))
}

fn ticks_per_second() -> io::Result<u64> {
    // SAFETY: sysconf reads a process-wide constant and does not access caller memory.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    u64::try_from(ticks)
        .ok()
        .filter(|ticks| *ticks > 0)
        .ok_or_else(|| io::Error::other("Linux clock tick frequency unavailable"))
}

pub fn current_uid() -> u32 {
    // SAFETY: geteuid takes no arguments and cannot fail.
    unsafe { libc::geteuid() }
}

pub fn list_pids() -> io::Result<Vec<i32>> {
    list_pids_at(Path::new(PROC))
}

fn list_pids_at(proc_root: &Path) -> io::Result<Vec<i32>> {
    let mut pids = Vec::new();
    for entry in fs::read_dir(proc_root)? {
        let entry = entry?;
        if let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
            && pid > 0
        {
            if pids.len() == MAX_PROCESSES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "process inventory exceeds the read limit",
                ));
            }
            pids.push(pid);
        }
    }
    Ok(pids)
}

fn exe_path(proc_root: &Path, pid: i32) -> io::Result<Option<PathBuf>> {
    match fs::read_link(proc_root.join(pid.to_string()).join("exe")) {
        Ok(path) => Ok(Some(path)),
        Err(err) if missing(&err) => Ok(None),
        Err(err) => Err(err),
    }
}

fn read_bounded(path: impl AsRef<Path>, max_bytes: usize) -> io::Result<Vec<u8>> {
    let mut file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(u64::try_from(max_bytes).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "proc file exceeds the read limit",
        ));
    }
    Ok(bytes)
}

fn command_line(proc_root: &Path, pid: i32) -> io::Result<Option<Vec<Vec<u8>>>> {
    match read_bounded(proc_root.join(pid.to_string()).join("cmdline"), MAX_CMDLINE_BYTES) {
        Ok(bytes) if bytes.is_empty() => Ok(Some(Vec::new())),
        Ok(bytes) => Ok(Some(
            bytes
                .split(|byte| *byte == 0)
                .filter(|arg| !arg.is_empty())
                .map(<[u8]>::to_vec)
                .collect(),
        )),
        Err(err) if missing(&err) => Ok(None),
        Err(err) => Err(err),
    }
}

fn listener_inodes_for_port(proc_root: &Path, process: &str, port: u16) -> io::Result<BTreeSet<u64>> {
    if port == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid TCP port"));
    }
    let mut inodes = BTreeSet::new();
    for table in [
        proc_root.join(process).join("net/tcp"),
        proc_root.join(process).join("net/tcp6"),
    ] {
        let bytes = read_bounded(table, MAX_PROC_NET_BYTES)?;
        for line in bytes
            .split(|byte| *byte == b'\n')
            .skip(1)
            .filter(|line| !line.is_empty())
        {
            let fields: Vec<&[u8]> = line
                .split(|byte| byte.is_ascii_whitespace())
                .filter(|part| !part.is_empty())
                .collect();
            if fields.len() < 10 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "short proc TCP table row",
                ));
            }
            if fields[3] != b"0A" {
                continue;
            }
            let port_field = fields[1].rsplit(|byte| *byte == b':').next().unwrap_or_default();
            let local_port = u16::from_str_radix(
                std::str::from_utf8(port_field)
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid proc TCP port"))?,
                16,
            )
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid proc TCP port"))?;
            if local_port == port {
                inodes.insert(parse_number(fields[9])?);
            }
        }
    }
    Ok(inodes)
}

fn network_namespace_inode(proc_root: &Path, process: &str) -> io::Result<u64> {
    Ok(fs::metadata(proc_root.join(process).join("ns/net"))?.ino())
}

fn require_current_network_namespace(
    namespace: u64,
    current_namespace: u64,
    inodes: &BTreeSet<u64>,
) -> io::Result<()> {
    if namespace != current_namespace && !inodes.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "port inventory found a listener in another network namespace",
        ));
    }
    Ok(())
}

pub fn tcp_listeners(port: u16) -> io::Result<Vec<ProcessIdentity>> {
    let provider = LinuxProvider::new()?;
    let current_namespace = network_namespace_inode(Path::new(PROC), "self")?;
    let mut by_namespace: BTreeMap<u64, Vec<ListenerProcess>> = BTreeMap::new();
    for pid in list_pids()? {
        let process = match provider.read(pid) {
            Ok(ProcessRead::Present(identity)) if identity.kernel.uid == current_uid() => {
                ListenerProcess::identified(identity)
            }
            Ok(ProcessRead::PathUnreadable(kernel)) if kernel.uid == current_uid() => {
                ListenerProcess::unreadable(kernel)
            }
            Ok(ProcessRead::Present(_)) | Ok(ProcessRead::PathUnreadable(_)) | Ok(ProcessRead::Gone) => {
                continue;
            }
            Err(error) if missing(&error) => continue,
            Err(error) => return Err(error),
        };
        let pid_path = process.kernel.pid.to_string();
        let namespace = match network_namespace_inode(Path::new(PROC), &pid_path) {
            Ok(namespace) => namespace,
            Err(error) if missing(&error) => continue,
            Err(error) => return Err(error),
        };
        by_namespace.entry(namespace).or_default().push(process);
    }

    let mut inodes = BTreeSet::new();
    let mut candidates = Vec::new();
    for (namespace, mut identities) in by_namespace {
        let mut namespace_inodes = None;
        for process in &mut identities {
            let pid = process.kernel.pid;
            if !process.refresh(&provider)? {
                continue;
            }
            let pid_path = pid.to_string();
            match network_namespace_inode(Path::new(PROC), &pid_path) {
                Ok(found) if found == namespace => {}
                Ok(_) => {
                    return Err(io::Error::other(
                        "process network namespace changed during inventory",
                    ));
                }
                Err(error) if missing(&error) => continue,
                Err(error) => return Err(error),
            }
            let found = match listener_inodes_for_port(Path::new(PROC), &pid_path, port) {
                Ok(found) => found,
                Err(error) if missing(&error) => continue,
                Err(error) => return Err(error),
            };
            if !process.refresh(&provider)?
                || network_namespace_inode(Path::new(PROC), &pid_path)? != namespace
            {
                return Err(io::Error::other(
                    "process identity or network namespace changed during inventory",
                ));
            }
            namespace_inodes = Some(found);
            break;
        }
        let Some(namespace_inodes) = namespace_inodes else {
            continue;
        };
        if namespace_inodes.is_empty() {
            continue;
        }
        require_current_network_namespace(namespace, current_namespace, &namespace_inodes)?;
        inodes = namespace_inodes;
        candidates = identities;
    }
    if inodes.is_empty() {
        return Ok(Vec::new());
    }

    let mut found = Vec::new();
    for mut process in candidates {
        let pid = process.kernel.pid;
        let pid_path = pid.to_string();
        if !process.refresh(&provider)? {
            continue;
        }
        if network_namespace_inode(Path::new(PROC), &pid_path)? != current_namespace {
            return Err(io::Error::other(
                "process network namespace changed during inventory",
            ));
        }
        let fd_dir = Path::new(PROC).join(pid.to_string()).join("fd");
        let descriptors = match fs::read_dir(&fd_dir) {
            Ok(descriptors) => descriptors,
            Err(error) if missing(&error) => continue,
            Err(error) => return Err(error),
        };
        let mut owns_listener = false;
        for descriptor in descriptors {
            let descriptor = descriptor?;
            let target = match fs::read_link(descriptor.path()) {
                Ok(target) => target,
                Err(error) if missing(&error) => continue,
                Err(error) => return Err(error),
            };
            if let Some(inode) = socket_inode(&target)
                && inodes.contains(&inode)
            {
                owns_listener = true;
                break;
            }
        }
        if !owns_listener {
            continue;
        }
        if let Some(fresh) = listener_identity(&process.kernel, provider.read(pid)?)? {
            if network_namespace_inode(Path::new(PROC), &pid_path)? != current_namespace {
                return Err(io::Error::other(
                    "listener identity or network namespace changed during inventory",
                ));
            }
            found.push(fresh);
        }
    }
    if network_namespace_inode(Path::new(PROC), "self")? != current_namespace {
        return Err(io::Error::other(
            "AgentDust network namespace changed during inventory",
        ));
    }
    let current_inodes = listener_inodes_for_port(Path::new(PROC), "self", port)?;
    let mut stable = Vec::new();
    for identity in found {
        let pid = identity.kernel.pid;
        let pid_path = pid.to_string();
        if network_namespace_inode(Path::new(PROC), &pid_path)? != current_namespace {
            return Err(io::Error::other(
                "process network namespace changed during inventory",
            ));
        }
        let fd_dir = Path::new(PROC).join(&pid_path).join("fd");
        let descriptors = match fs::read_dir(fd_dir) {
            Ok(descriptors) => descriptors,
            Err(error) if missing(&error) => continue,
            Err(error) => return Err(error),
        };
        let mut still_listening = false;
        for descriptor in descriptors {
            let descriptor = descriptor?;
            let target = match fs::read_link(descriptor.path()) {
                Ok(target) => target,
                Err(error) if missing(&error) => continue,
                Err(error) => return Err(error),
            };
            if socket_inode(&target).is_some_and(|inode| current_inodes.contains(&inode)) {
                still_listening = true;
                break;
            }
        }
        if !still_listening {
            continue;
        }
        match provider.read(pid)? {
            ProcessRead::Present(fresh)
                if fresh == identity
                    && network_namespace_inode(Path::new(PROC), &pid_path)? == current_namespace =>
            {
                stable.push(identity)
            }
            ProcessRead::Gone => {}
            _ => {
                return Err(io::Error::other(
                    "listener identity or network namespace changed during inventory",
                ));
            }
        }
    }
    Ok(stable)
}

#[derive(Clone)]
struct ListenerProcess {
    kernel: KernelIdentity,
    identity: Option<ProcessIdentity>,
}

impl ListenerProcess {
    fn identified(identity: ProcessIdentity) -> Self {
        Self {
            kernel: identity.kernel.clone(),
            identity: Some(identity),
        }
    }

    fn unreadable(kernel: KernelIdentity) -> Self {
        Self {
            kernel,
            identity: None,
        }
    }

    fn refresh<P: ProcessProvider + ?Sized>(&mut self, provider: &P) -> io::Result<bool> {
        let read = match provider.read(self.kernel.pid) {
            Ok(read) => read,
            Err(error) if missing(&error) => return Ok(false),
            Err(error) => return Err(error),
        };
        match read {
            ProcessRead::Present(identity) if identity.kernel == self.kernel => {
                self.identity = Some(identity);
                Ok(true)
            }
            ProcessRead::PathUnreadable(kernel) if self.kernel == kernel => Ok(true),
            ProcessRead::Present(_) | ProcessRead::PathUnreadable(_) | ProcessRead::Gone => Ok(false),
        }
    }
}

fn listener_identity(expected: &KernelIdentity, read: ProcessRead) -> io::Result<Option<ProcessIdentity>> {
    match read {
        ProcessRead::Present(identity) if identity.kernel == *expected => Ok(Some(identity)),
        ProcessRead::Gone => Ok(None),
        ProcessRead::PathUnreadable(kernel) if kernel == *expected => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("listener process identity is unreadable for PID {}", expected.pid),
        )),
        ProcessRead::Present(_) | ProcessRead::PathUnreadable(_) => {
            Err(io::Error::other("listener identity changed during inventory"))
        }
    }
}

#[cfg(test)]
mod listener_process_tests {
    use super::*;
    use crate::identity::IdentityEvidence;

    struct ScriptedProvider(ProcessRead);

    impl ProcessProvider for ScriptedProvider {
        fn read(&self, _pid: i32) -> io::Result<ProcessRead> {
            Ok(self.0.clone())
        }
    }

    fn kernel(start_time_us: u64) -> KernelIdentity {
        KernelIdentity {
            boot_session_uuid: "a23b2a10-71df-4f5d-a941-4a18eaac32a8".into(),
            pid: 42,
            start_time_us,
            uid: 1000,
        }
    }

    fn identity(kernel: KernelIdentity, exe_path: &str) -> ProcessIdentity {
        ProcessIdentity {
            kernel,
            evidence: IdentityEvidence {
                exe_path: PathBuf::from(exe_path),
            },
        }
    }

    #[test]
    fn unreadable_executable_is_retained_only_as_kernel_identity_during_discovery() {
        let expected = kernel(100);
        let mut process = ListenerProcess::unreadable(expected.clone());
        assert!(
            process
                .refresh(&ScriptedProvider(ProcessRead::PathUnreadable(expected)))
                .unwrap()
        );
        assert!(process.identity.is_none());
    }

    #[test]
    fn exec_refreshes_path_for_the_same_kernel_process() {
        let expected = kernel(100);
        let mut process = ListenerProcess::unreadable(expected.clone());
        let fresh = identity(expected, "/usr/bin/fixture");
        assert!(
            process
                .refresh(&ScriptedProvider(ProcessRead::Present(fresh.clone())))
                .unwrap()
        );
        assert_eq!(process.identity, Some(fresh));
    }

    #[test]
    fn a_reused_pid_does_not_refresh_the_discovered_process() {
        let expected = kernel(100);
        let mut process = ListenerProcess::unreadable(expected);
        assert!(
            !process
                .refresh(&ScriptedProvider(ProcessRead::PathUnreadable(kernel(101))))
                .unwrap()
        );
        assert!(process.identity.is_none());
    }

    #[test]
    fn unreadable_identity_of_the_actual_listener_fails_closed() {
        let expected = kernel(100);
        let error = listener_identity(&expected, ProcessRead::PathUnreadable(expected.clone())).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn the_verified_listener_identity_keeps_its_executable_evidence() {
        let expected = kernel(100);
        let fresh = identity(expected.clone(), "/usr/bin/fixture");
        assert_eq!(
            listener_identity(&expected, ProcessRead::Present(fresh.clone())).unwrap(),
            Some(fresh)
        );
    }

    #[test]
    fn a_gone_listener_is_not_an_identity_error() {
        assert!(
            listener_identity(&kernel(100), ProcessRead::Gone)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_reused_pid_cannot_become_a_listener_identity() {
        let error = listener_identity(
            &kernel(100),
            ProcessRead::Present(identity(kernel(101), "/usr/bin/fixture")),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
    }
}

fn socket_inode(target: &Path) -> Option<u64> {
    let text = target.to_str()?;
    text.strip_prefix("socket:[")?.strip_suffix(']')?.parse().ok()
}

fn script_argument(args: &[Vec<u8>]) -> Option<&[u8]> {
    const VALUE_OPTIONS: [&[u8]; 7] = [
        b"-r",
        b"--require",
        b"--import",
        b"--loader",
        b"--experimental-loader",
        b"-C",
        b"--conditions",
    ];
    const CODE_OPTIONS: [&[u8]; 4] = [b"-e", b"--eval", b"-p", b"--print"];
    let mut args = args.iter().skip(1);
    while let Some(arg) = args.next() {
        if arg == b"--" {
            return args.next().map(Vec::as_slice);
        }
        if CODE_OPTIONS.contains(&arg.as_slice()) {
            return None;
        }
        if VALUE_OPTIONS.contains(&arg.as_slice()) {
            args.next();
            continue;
        }
        if !arg.starts_with(b"-") {
            return Some(arg);
        }
    }
    None
}

fn environment_tags(proc_root: &Path, pid: i32) -> io::Result<SessionEnv> {
    let file = fs::File::open(proc_root.join(pid.to_string()).join("environ"))?;
    parse_environment(file)
}

fn parse_environment(input: impl Read) -> io::Result<SessionEnv> {
    let mut reader = BufReader::new(input).take(MAX_ENV_BYTES + 1);
    let mut env = SessionEnv::default();
    let mut entry = Vec::new();
    let mut total = 0_u64;
    loop {
        entry.clear();
        let read = reader.read_until(0, &mut entry)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > MAX_ENV_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "proc environment exceeds the read limit",
            ));
        }
        let value = entry.strip_suffix(&[0]).unwrap_or(&entry);
        for (name, values) in [
            (ENV_NAME.as_bytes(), &mut env.claude),
            (CODEX_ENV_NAME.as_bytes(), &mut env.codex),
        ] {
            if let Some(value) = value.strip_prefix(name).and_then(|tail| tail.strip_prefix(b"=")) {
                if value.len() > MAX_ENV_VALUE_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "AgentDust environment marker exceeds the read limit",
                    ));
                }
                values.push(value.to_vec());
            }
        }
    }
    Ok(env)
}

fn read_linux_args(
    env: io::Result<SessionEnv>,
    exe_path: Option<&Path>,
    cmdline: Option<&[Vec<u8>]>,
    secret: Option<&Secret>,
) -> ArgsView {
    let Ok(env) = env else {
        return ArgsView {
            tag: Tag::Unreadable,
            agent_script: false,
        };
    };
    let tag = match (env.claude.as_slice(), env.codex.as_slice(), secret) {
        ([], [], _) => Tag::Absent,
        ([value], [], Some(secret)) => Tag::Keyed(tag::key_of(secret, value)),
        ([], [value], Some(secret)) => tag::codex_key_of(secret, value).map_or(Tag::Unreadable, Tag::Keyed),
        ([_], [], None) | ([], [_], None) => Tag::Unkeyed,
        _ => Tag::Unreadable,
    };
    let agent_script = exe_path.is_some_and(crate::ancestry::is_node)
        && cmdline
            .and_then(|args| script_argument(args))
            .is_some_and(crate::ancestry::script_names_agent);
    ArgsView { tag, agent_script }
}

pub struct LinuxPidfdSignaller {
    provider: LinuxProvider,
}

impl LinuxPidfdSignaller {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            provider: LinuxProvider::new()?,
        })
    }
}

impl Signaller for LinuxPidfdSignaller {
    fn sigterm(&self, _pid: i32) -> SignalResult {
        SignalResult::Refused
    }

    fn sigterm_identity(&self, expected: &ProcessIdentity) -> SignalResult {
        pidfd_signal(expected, &self.provider, &SystemPidfdOps)
    }
}

trait PidfdOps {
    type Handle;

    fn open(&self, pid: i32) -> io::Result<Self::Handle>;
    fn verify_unmanaged_unit(&self, handle: &Self::Handle, expected_uid: u32) -> io::Result<()>;
    fn sigterm(&self, handle: &Self::Handle) -> SignalResult;
}

struct SystemPidfdOps;

impl PidfdOps for SystemPidfdOps {
    type Handle = OwnedFd;

    fn open(&self, pid: i32) -> io::Result<Self::Handle> {
        // SAFETY: pidfd_open reads only its scalar arguments and returns a new descriptor.
        let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0_u32) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = i32::try_from(raw).map_err(|_| io::Error::other("pidfd is outside the descriptor range"))?;
        // SAFETY: pidfd_open returned a new owned descriptor.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn verify_unmanaged_unit(&self, handle: &Self::Handle, expected_uid: u32) -> io::Result<()> {
        systemd_unit_for_pidfd(handle.as_raw_fd(), expected_uid)
    }

    fn sigterm(&self, handle: &Self::Handle) -> SignalResult {
        // SAFETY: pidfd_send_signal uses the open pidfd and a null optional siginfo pointer.
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                handle.as_raw_fd(),
                libc::SIGTERM,
                std::ptr::null::<libc::siginfo_t>(),
                0_u32,
            )
        };
        if result == 0 {
            return SignalResult::Delivered;
        }
        signal_error(io::Error::last_os_error())
    }
}

fn pidfd_signal<P: ProcessProvider, O: PidfdOps>(
    expected: &ProcessIdentity,
    provider: &P,
    ops: &O,
) -> SignalResult {
    let pid = expected.kernel.pid;
    if !signalable(pid) {
        return SignalResult::Refused;
    }
    let handle = match ops.open(pid) {
        Ok(handle) => handle,
        Err(error) => return signal_error(error),
    };
    if revalidate(expected, provider) != Revalidation::Match {
        return SignalResult::Refused;
    }
    if ops.verify_unmanaged_unit(&handle, expected.kernel.uid).is_err() {
        return SignalResult::Refused;
    }
    if revalidate(expected, provider) != Revalidation::Match {
        return SignalResult::Refused;
    }
    ops.sigterm(&handle)
}

fn signal_error(error: io::Error) -> SignalResult {
    match error.raw_os_error() {
        Some(libc::ESRCH) => SignalResult::NoSuchProcess,
        Some(libc::EPERM) => SignalResult::Refused,
        other => SignalResult::Failed(other.unwrap_or(0)),
    }
}

type SdPidfdGetText = unsafe extern "C" fn(c_int, *mut *mut c_char) -> c_int;

#[derive(Clone, Copy)]
enum SystemdUnitKind {
    User,
    System,
}

fn systemd_unit_for_pidfd(pidfd: c_int, expected_uid: u32) -> io::Result<()> {
    const USER_UNIT: &CStr = c"sd_pidfd_get_user_unit";
    const SYSTEM_UNIT: &CStr = c"sd_pidfd_get_unit";
    const CGROUP: &CStr = c"sd_pidfd_get_cgroup";
    let get_user_unit = systemd_function(USER_UNIT).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "libsystemd user pidfd unit lookup is unavailable",
        )
    })?;
    let get_system_unit = systemd_function(SYSTEM_UNIT).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "libsystemd system pidfd unit lookup is unavailable",
        )
    })?;
    let get_cgroup = systemd_function(CGROUP).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "libsystemd pidfd cgroup lookup is unavailable",
        )
    })?;
    let cgroup = unsafe { call_systemd_text(get_cgroup, pidfd) }?;
    verify_systemd_unit_lookups(
        unsafe { call_systemd_unit(get_user_unit, pidfd) },
        unsafe { call_systemd_unit(get_system_unit, pidfd) },
        &cgroup,
        expected_uid,
    )?;
    let mut record = b"0::".to_vec();
    record.extend_from_slice(&cgroup);
    record.push(b'\n');
    if cgroup_unit_evidence(&record)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "process belongs to a managed systemd unit",
        ));
    }
    Ok(())
}

fn verify_systemd_unit_lookups(
    user_unit: io::Result<Vec<u8>>,
    system_unit: io::Result<Vec<u8>>,
    cgroup: &[u8],
    expected_uid: u32,
) -> io::Result<()> {
    let mut found_unit = false;
    for (kind, result) in [
        (SystemdUnitKind::User, user_unit),
        (SystemdUnitKind::System, system_unit),
    ] {
        match result {
            Ok(unit) => {
                found_unit = true;
                if systemd_unit_is_managed(kind, &unit, cgroup, expected_uid)? {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "process belongs to a managed systemd unit",
                    ));
                }
            }
            Err(error) if error.raw_os_error() == Some(libc::ENODATA) => {}
            Err(error) => return Err(error),
        }
    }
    if found_unit {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "systemd did not report authoritative unit membership",
        ))
    }
}

fn unit_matches_user_slice_cgroup(unit: &[u8], cgroup: &[u8], uid: u32) -> bool {
    let prefix = format!("/user.slice/user-{uid}.slice/");
    cgroup.starts_with(prefix.as_bytes())
        && cgroup
            .split(|byte| *byte == b'/')
            .any(|component| component == unit)
}

fn systemd_unit_is_managed(
    kind: SystemdUnitKind,
    unit: &[u8],
    cgroup: &[u8],
    expected_uid: u32,
) -> io::Result<bool> {
    if unit.ends_with(b".service") {
        let user_manager = format!("user@{expected_uid}.service");
        if matches!(kind, SystemdUnitKind::System)
            && unit == user_manager.as_bytes()
            && unit_matches_user_slice_cgroup(unit, cgroup, expected_uid)
        {
            return Ok(false);
        }
        return Ok(true);
    }
    if unit.ends_with(b".scope") {
        return Ok(!unit_matches_user_slice_cgroup(unit, cgroup, expected_uid));
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "systemd returned an unsupported unit name",
    ))
}

fn systemd_function(symbol: &CStr) -> Option<SdPidfdGetText> {
    static USER: OnceLock<Option<SdPidfdGetText>> = OnceLock::new();
    static SYSTEM: OnceLock<Option<SdPidfdGetText>> = OnceLock::new();
    static CGROUP: OnceLock<Option<SdPidfdGetText>> = OnceLock::new();
    let slot = match symbol.to_bytes() {
        b"sd_pidfd_get_user_unit" => &USER,
        b"sd_pidfd_get_unit" => &SYSTEM,
        b"sd_pidfd_get_cgroup" => &CGROUP,
        _ => return None,
    };
    *slot.get_or_init(|| {
        const LIBSYSTEMD: &CStr = c"libsystemd.so.0";
        // SAFETY: the library handle remains loaded for the process lifetime.
        let library = unsafe { libc::dlopen(LIBSYSTEMD.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if library.is_null() {
            return None;
        }
        // SAFETY: dlsym resolves the requested systemd function by its C symbol name.
        let function = unsafe { libc::dlsym(library, symbol.as_ptr()) };
        if function.is_null() {
            return None;
        }
        // SAFETY: the selected sd_pidfd_get_* function returns an allocated string through this signature.
        Some(unsafe { std::mem::transmute::<*mut c_void, SdPidfdGetText>(function) })
    })
}

unsafe fn call_systemd_unit(get_unit: SdPidfdGetText, pidfd: c_int) -> io::Result<Vec<u8>> {
    unsafe { call_systemd_text(get_unit, pidfd) }
}

unsafe fn call_systemd_text(get_text: SdPidfdGetText, pidfd: c_int) -> io::Result<Vec<u8>> {
    let mut text = std::ptr::null_mut();
    // SAFETY: pidfd is an open pidfd and text points to writable output storage.
    let result = unsafe { get_text(pidfd, &mut text) };
    if result < 0 {
        return Err(io::Error::from_raw_os_error(-result));
    }
    if text.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "systemd returned an empty value",
        ));
    }
    // SAFETY: systemd returned a NUL terminated allocation on success.
    let value = unsafe { CStr::from_ptr(text) };
    let bytes = value.to_bytes();
    let valid = !bytes.is_empty() && std::str::from_utf8(bytes).is_ok();
    let copied = bytes.to_vec();
    // SAFETY: systemd documents that returned unit strings are freed with libc free.
    unsafe { libc::free(text.cast()) };
    if valid {
        Ok(copied)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "systemd returned an invalid value",
        ))
    }
}

pub struct LinuxProvider {
    proc_root: PathBuf,
    boot_id: String,
    boot_epoch_us: u64,
    ticks_per_second: u64,
}

impl LinuxProvider {
    pub fn new() -> io::Result<Self> {
        Self::with_proc_root(PathBuf::from(PROC))
    }

    fn with_proc_root(proc_root: PathBuf) -> io::Result<Self> {
        Ok(Self {
            boot_id: boot_session_uuid_at(&proc_root)?,
            boot_epoch_us: boot_epoch_us_at(&proc_root)?,
            ticks_per_second: ticks_per_second()?,
            proc_root,
        })
    }

    fn info(&self, pid: i32) -> io::Result<Option<ProcessInfo>> {
        process_info(
            &self.proc_root,
            pid,
            &self.boot_id,
            self.boot_epoch_us,
            self.ticks_per_second,
        )
    }

    fn verified<T>(
        &self,
        identity: &KernelIdentity,
        read: impl FnOnce(i32) -> io::Result<Option<T>>,
    ) -> Option<T> {
        if identity.boot_session_uuid != self.boot_id || identity.pid <= 0 {
            return None;
        }
        let before = self.info(identity.pid).ok()??;
        if before.identity != *identity {
            return None;
        }
        let value = read(identity.pid).ok()??;
        let after = self.info(identity.pid).ok()??;
        (after.identity == *identity).then_some(value)
    }
}

impl ProcessProvider for LinuxProvider {
    fn read(&self, pid: i32) -> io::Result<ProcessRead> {
        let Some(before) = self.info(pid)? else {
            return Ok(ProcessRead::Gone);
        };
        let path = exe_path(&self.proc_root, pid).ok().flatten();
        let after = self.info(pid)?;
        Ok(ProcessRead::from_samples(
            Some(before.identity),
            path,
            after.map(|info| info.identity),
        ))
    }
}

impl AncestryProvider for LinuxProvider {
    fn parent(&self, identity: &KernelIdentity) -> io::Result<Option<i32>> {
        match self.info(identity.pid)? {
            Some(info) if info.identity == *identity => Ok(Some(info.ppid)),
            _ => Ok(None),
        }
    }

    fn script_argument(&self, pid: i32) -> io::Result<Option<Vec<u8>>> {
        let Some(identity) = self.info(pid)?.map(|info| info.identity) else {
            return Ok(None);
        };
        self.script_argument_for(&identity)
    }

    fn script_argument_for(&self, identity: &KernelIdentity) -> io::Result<Option<Vec<u8>>> {
        if identity.boot_session_uuid != self.boot_id {
            return Ok(None);
        }
        let Some(before) = self.info(identity.pid)? else {
            return Ok(None);
        };
        if before.identity != *identity {
            return Ok(None);
        }
        let argument = command_line(&self.proc_root, identity.pid)?
            .and_then(|args| script_argument(&args).map(<[u8]>::to_vec));
        let Some(after) = self.info(identity.pid)? else {
            return Ok(None);
        };
        if after.identity != *identity {
            return Ok(None);
        }
        Ok(argument)
    }
}

pub struct LinuxSource<'a> {
    provider: LinuxProvider,
    secret: Option<&'a Secret>,
}

impl<'a> LinuxSource<'a> {
    pub fn new(secret: Option<&'a Secret>) -> io::Result<Self> {
        Ok(Self {
            provider: LinuxProvider::new()?,
            secret,
        })
    }

    fn cpu_time(&self, identity: &KernelIdentity) -> io::Result<Option<u64>> {
        if identity.boot_session_uuid != self.provider.boot_id {
            return Ok(None);
        }
        let Some(before) = self.provider.info(identity.pid)? else {
            return Ok(None);
        };
        if before.identity != *identity {
            return Ok(None);
        }
        let stat = parse_stat(&read_bounded(
            self.provider
                .proc_root
                .join(identity.pid.to_string())
                .join("stat"),
            MAX_STAT_BYTES,
        )?)?;
        let ticks = stat.user_ticks.saturating_add(stat.system_ticks);
        let ns =
            u128::from(ticks) * u128::from(NANOS_PER_SECOND) / u128::from(self.provider.ticks_per_second);
        match self.provider.info(identity.pid)? {
            Some(after) if after.identity == *identity => Ok(Some(u64::try_from(ns).unwrap_or(u64::MAX))),
            _ => Ok(None),
        }
    }

    fn read_process(&self, pid: i32) -> Option<RawProcess> {
        let before = self.provider.info(pid).ok()??;
        let exe_path = exe_path(&self.provider.proc_root, pid).ok().flatten();
        let stat = parse_stat(
            &read_bounded(
                self.provider.proc_root.join(pid.to_string()).join("stat"),
                MAX_STAT_BYTES,
            )
            .ok()?,
        )
        .ok()?;
        let env = if before.identity.uid == current_uid() {
            environment_tags(&self.provider.proc_root, pid)
        } else {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "foreign process"))
        };
        let cmdline = (before.identity.uid == current_uid())
            .then(|| command_line(&self.provider.proc_root, pid).ok().flatten())
            .flatten();
        let view = if before.identity.uid == current_uid() {
            read_linux_args(env, exe_path.as_deref(), cmdline.as_deref(), self.secret)
        } else {
            ArgsView {
                tag: Tag::Unreadable,
                agent_script: false,
            }
        };
        let after = self.provider.info(pid).ok()??;
        (after.identity == before.identity).then_some(RawProcess {
            identity: RawIdentity {
                kernel: before.identity,
                exe_path,
                ppid: before.ppid,
                pgid: before.pgid,
            },
            tag: view.tag,
            agent_script: view.agent_script,
            cpu: Cpu {
                first_ns: Some(self.ticks_to_ns(stat.user_ticks.saturating_add(stat.system_ticks))),
                later_ns: None,
            },
        })
    }

    fn ticks_to_ns(&self, ticks: u64) -> u64 {
        let ns =
            u128::from(ticks) * u128::from(NANOS_PER_SECOND) / u128::from(self.provider.ticks_per_second);
        u64::try_from(ns).unwrap_or(u64::MAX)
    }
}

impl ProcessSource for LinuxSource<'_> {
    fn scan(&self) -> io::Result<Vec<RawProcess>> {
        let mut processes = Vec::new();
        for pid in list_pids_at(&self.provider.proc_root)? {
            if let Some(process) = self.read_process(pid) {
                processes.push(process);
            }
        }
        Ok(processes)
    }

    fn cpu_time_ns(&self, identity: &KernelIdentity) -> io::Result<Option<u64>> {
        self.cpu_time(identity)
    }
}

pub struct LinuxLive {
    provider: LinuxProvider,
}

impl LinuxLive {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            provider: LinuxProvider::new()?,
        })
    }
}

impl LiveDetails for LinuxLive {
    fn command(&self, identity: &KernelIdentity) -> Option<Vec<Vec<u8>>> {
        self.provider
            .verified(identity, |pid| command_line(&self.provider.proc_root, pid))
    }

    fn cwd(&self, identity: &KernelIdentity) -> Option<PathBuf> {
        self.provider.verified(identity, |pid| {
            match fs::read_link(self.provider.proc_root.join(pid.to_string()).join("cwd")) {
                Ok(path) => Ok(Some(path)),
                Err(err) if missing(&err) => Ok(None),
                Err(err) => Err(err),
            }
        })
    }
}

pub struct SystemdCgroupEvidence;

fn cgroup_unit_component(path: &[u8]) -> Option<&[u8]> {
    let units: Vec<&[u8]> = path
        .split(|byte| *byte == b'/')
        .filter(|component| component.ends_with(b".service") || component.ends_with(b".scope"))
        .collect();
    units
        .iter()
        .rev()
        .find(|unit| !unit.starts_with(b"user@"))
        .copied()
        .or_else(|| units.last().copied())
}

fn cgroup_unit_evidence(cgroup: &[u8]) -> io::Result<bool> {
    for line in cgroup
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        if let Some(path) = line.strip_prefix(b"0::") {
            let Some(unit) = cgroup_unit_component(path) else {
                return Ok(false);
            };
            if unit.ends_with(b".service") {
                return Ok(!unit.starts_with(b"user@"));
            }
            return Ok(unit.ends_with(b".scope") && path.starts_with(b"/system.slice/"));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "unified cgroup v2 is required for systemd unit evidence",
    ))
}

impl crate::inventory::LaunchdSource for SystemdCgroupEvidence {
    fn pids(&self) -> io::Result<BTreeSet<i32>> {
        let mut pids = BTreeSet::new();
        for pid in list_pids()? {
            let cgroup = match read_bounded(
                Path::new(PROC).join(pid.to_string()).join("cgroup"),
                MAX_STAT_BYTES,
            ) {
                Ok(cgroup) => cgroup,
                Err(err) if missing(&err) => continue,
                Err(err) => return Err(err),
            };
            if cgroup_unit_evidence(&cgroup)? {
                pids.insert(pid);
            }
        }
        Ok(pids)
    }
}

fn missing(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::ENOENT) | Some(libc::ESRCH))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::io;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use super::{
        LinuxProvider, PidfdOps, ProcStat, SystemdUnitKind, cgroup_unit_component, cgroup_unit_evidence,
        parse_environment, parse_stat, parse_uid_status, pidfd_signal, process_info,
        require_current_network_namespace, systemd_unit_is_managed, verify_systemd_unit_lookups,
    };
    use crate::ancestry::AncestryProvider;
    use crate::apply::signal::SignalResult;
    use crate::identity::{IdentityEvidence, KernelIdentity, ProcessIdentity};
    use crate::provider::{ProcessProvider, ProcessRead};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct ScriptedProvider {
        identity: ProcessIdentity,
        reads: AtomicUsize,
        changed_on_second_read: bool,
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    impl ProcessProvider for ScriptedProvider {
        fn read(&self, _pid: i32) -> io::Result<ProcessRead> {
            self.events.lock().unwrap().push("read");
            let count = self.reads.fetch_add(1, Ordering::Relaxed);
            let mut identity = self.identity.clone();
            if self.changed_on_second_read && count > 0 {
                identity.kernel.start_time_us += 1;
            }
            Ok(ProcessRead::Present(identity))
        }
    }

    struct FakePidfdOps {
        has_unit: bool,
        managed_unit: bool,
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    impl PidfdOps for FakePidfdOps {
        type Handle = ();

        fn open(&self, _pid: i32) -> io::Result<Self::Handle> {
            self.events.lock().unwrap().push("open");
            Ok(())
        }

        fn verify_unmanaged_unit(&self, _handle: &Self::Handle, _expected_uid: u32) -> io::Result<()> {
            self.events.lock().unwrap().push("unit");
            match (self.has_unit, self.managed_unit) {
                (true, false) => Ok(()),
                (false, _) => Err(io::Error::new(io::ErrorKind::NotFound, "no unit")),
                (true, true) => Err(io::Error::new(io::ErrorKind::PermissionDenied, "managed unit")),
            }
        }

        fn sigterm(&self, _handle: &Self::Handle) -> SignalResult {
            self.events.lock().unwrap().push("signal");
            SignalResult::Delivered
        }
    }

    fn expected_identity() -> ProcessIdentity {
        ProcessIdentity {
            kernel: KernelIdentity {
                boot_session_uuid: "boot".into(),
                pid: 42,
                start_time_us: 100,
                uid: 1000,
            },
            evidence: IdentityEvidence {
                exe_path: "/usr/bin/node".into(),
            },
        }
    }

    fn scripted_action(
        has_unit: bool,
        managed_unit: bool,
        changed_on_second_read: bool,
    ) -> (SignalResult, Vec<&'static str>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let provider = ScriptedProvider {
            identity: expected_identity(),
            reads: AtomicUsize::new(0),
            changed_on_second_read,
            events: events.clone(),
        };
        let ops = FakePidfdOps {
            has_unit,
            managed_unit,
            events: events.clone(),
        };
        let result = pidfd_signal(&expected_identity(), &provider, &ops);
        let events = events.lock().unwrap().clone();
        (result, events)
    }

    #[test]
    fn pidfd_signal_revalidates_before_and_after_authoritative_unit_lookup() {
        let (result, events) = scripted_action(true, false, false);
        assert_eq!(result, SignalResult::Delivered);
        assert_eq!(events, ["open", "read", "unit", "read", "signal"]);
    }

    #[test]
    fn pidfd_signal_fails_closed_without_unit_or_for_a_managed_unit() {
        let (result, events) = scripted_action(false, false, false);
        assert_eq!(result, SignalResult::Refused);
        assert_eq!(events, ["open", "read", "unit"]);

        let (result, events) = scripted_action(true, true, false);
        assert_eq!(result, SignalResult::Refused);
        assert_eq!(events, ["open", "read", "unit"]);

        let (result, events) = scripted_action(true, false, true);
        assert_eq!(result, SignalResult::Refused);
        assert_eq!(events, ["open", "read", "unit", "read"]);
    }

    #[test]
    fn authoritative_unit_exceptions_match_the_target_users_cgroup() {
        assert!(
            systemd_unit_is_managed(
                SystemdUnitKind::System,
                b"ssh.service",
                b"/system.slice/ssh.service",
                1000
            )
            .unwrap()
        );
        assert!(
            systemd_unit_is_managed(
                SystemdUnitKind::User,
                b"agentdust-automatic.service",
                b"/user.slice/user-1000.slice/agentdust-automatic.service",
                1000
            )
            .unwrap()
        );
        assert!(
            !systemd_unit_is_managed(
                SystemdUnitKind::System,
                b"user@1000.service",
                b"/user.slice/user-1000.slice/user@1000.service",
                1000
            )
            .unwrap()
        );
        assert!(
            !systemd_unit_is_managed(
                SystemdUnitKind::System,
                b"session-2.scope",
                b"/user.slice/user-1000.slice/session-2.scope",
                1000
            )
            .unwrap()
        );
        assert!(
            systemd_unit_is_managed(
                SystemdUnitKind::System,
                b"build.scope",
                b"/machine.slice/build.scope",
                1000
            )
            .unwrap()
        );
        assert!(
            systemd_unit_is_managed(
                SystemdUnitKind::System,
                b"user@1000.service",
                b"/user.slice/user-1001.slice/user@1000.service",
                1001
            )
            .unwrap()
        );
        assert!(
            systemd_unit_is_managed(
                SystemdUnitKind::System,
                b"user@1001.service",
                b"/user.slice/user-1000.slice/user@1001.service",
                1000
            )
            .unwrap()
        );
        assert!(
            systemd_unit_is_managed(
                SystemdUnitKind::System,
                b"user@1000.service",
                b"/user.slice/user-1000.slice/session-2.scope",
                1000
            )
            .unwrap()
        );
        assert!(!cgroup_unit_evidence(b"0::/user.slice/user-1000.slice/session-2.scope\n").unwrap());
        assert_eq!(
            systemd_unit_is_managed(
                SystemdUnitKind::System,
                b"unexpected.target",
                b"/system.slice/unexpected.target",
                1000
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn systemd_unit_lookups_fail_closed_on_unexpected_or_missing_membership() {
        assert!(
            verify_systemd_unit_lookups(
                Ok(b"session-2.scope".to_vec()),
                Err(io::Error::from_raw_os_error(libc::ENODATA)),
                b"/user.slice/user-1000.slice/session-2.scope",
                1000,
            )
            .is_ok()
        );

        for results in [
            (
                Ok(b"session-2.scope".to_vec()),
                Err(io::Error::from_raw_os_error(libc::EIO)),
            ),
            (
                Err(io::Error::from_raw_os_error(libc::EIO)),
                Ok(b"session-2.scope".to_vec()),
            ),
        ] {
            assert_eq!(
                verify_systemd_unit_lookups(
                    results.0,
                    results.1,
                    b"/user.slice/user-1000.slice/session-2.scope",
                    1000,
                )
                .unwrap_err()
                .raw_os_error(),
                Some(libc::EIO)
            );
        }

        assert_eq!(
            verify_systemd_unit_lookups(
                Err(io::Error::from_raw_os_error(libc::ENODATA)),
                Err(io::Error::from_raw_os_error(libc::ENODATA)),
                b"/user.slice/user-1000.slice/session-2.scope",
                1000,
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::Unsupported
        );
        assert_eq!(
            verify_systemd_unit_lookups(
                Ok(b"agentdust.service".to_vec()),
                Err(io::Error::from_raw_os_error(libc::ENODATA)),
                b"/user.slice/user-1000.slice/agentdust.service",
                1000,
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::PermissionDenied
        );

        for cgroup in [
            &b"/machine.slice/session-2.scope"[..],
            &b"/user.slice/user-1001.slice/session-2.scope"[..],
            &b"/user.slice/user-1000.slice/other.scope"[..],
        ] {
            assert_eq!(
                verify_systemd_unit_lookups(
                    Ok(b"session-2.scope".to_vec()),
                    Err(io::Error::from_raw_os_error(libc::ENODATA)),
                    cgroup,
                    1000,
                )
                .unwrap_err()
                .kind(),
                io::ErrorKind::PermissionDenied
            );
        }
    }

    #[test]
    fn port_inventory_refuses_an_active_listener_outside_the_callers_network_namespace() {
        let listener = BTreeSet::from([123_u64]);
        assert_eq!(
            require_current_network_namespace(2, 1, &listener)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Unsupported
        );
        assert!(require_current_network_namespace(2, 1, &BTreeSet::new()).is_ok());
        assert!(require_current_network_namespace(1, 1, &listener).is_ok());
    }

    struct ProcFixture(PathBuf);

    impl ProcFixture {
        fn new() -> Self {
            let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("agentdust-proc-fixture-{}-{serial}", std::process::id()));
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join("stat"), b"btime 100\n").unwrap();
            fs::create_dir_all(root.join("sys/kernel/random")).unwrap();
            fs::write(
                root.join("sys/kernel/random/boot_id"),
                b"01234567-89ab-cdef-0123-456789abcdef\n",
            )
            .unwrap();
            Self(root)
        }

        fn process(&self, pid: i32, start_ticks: u64, ppid: i32) {
            let dir = self.0.join(pid.to_string());
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("stat"),
                stat_line(pid, "node (worker)", ppid, start_ticks),
            )
            .unwrap();
            fs::write(dir.join("status"), b"Name:\tnode\nUid:\t1000\t1000\t1000\t1000\n").unwrap();
        }
    }

    impl Drop for ProcFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn stat_line(pid: i32, comm: &str, ppid: i32, start_ticks: u64) -> Vec<u8> {
        stat_line_with_state(pid, comm, ppid, start_ticks, b'S')
    }

    fn stat_line_with_state(pid: i32, comm: &str, ppid: i32, start_ticks: u64, state: u8) -> Vec<u8> {
        let fields = [
            char::from(state).to_string(),
            ppid.to_string(),
            "3".to_owned(),
            "0".to_owned(),
            "0".to_owned(),
            "-1".to_owned(),
            "0".to_owned(),
            "0".to_owned(),
            "0".to_owned(),
            "0".to_owned(),
            "0".to_owned(),
            "11".to_owned(),
            "13".to_owned(),
            "0".to_owned(),
            "0".to_owned(),
            "20".to_owned(),
            "0".to_owned(),
            "1".to_owned(),
            "0".to_owned(),
            start_ticks.to_string(),
        ];
        format!("{pid} ({comm}) {}\n", fields.join(" ")).into_bytes()
    }

    #[test]
    fn stat_parsing_handles_spaces_and_parentheses_in_comm() {
        let stat = stat_line(42, "node (worker) pool", 7, 91);
        let parsed = parse_stat(&stat).unwrap();
        assert_eq!(
            parsed,
            ProcStat {
                state: b'S',
                ppid: 7,
                pgid: 3,
                user_ticks: 11,
                system_ticks: 13,
                start_ticks: 91,
            }
        );
    }

    #[test]
    fn zombie_processes_are_gone_for_owner_liveness() {
        let fixture = ProcFixture::new();
        fixture.process(42, 91, 7);
        let path = fixture.0.join("42/stat");
        assert!(process_info(&fixture.0, 42, "boot", 0, 100).unwrap().is_some());
        fs::write(&path, stat_line_with_state(42, "node", 7, 91, b'Z')).unwrap();
        assert!(process_info(&fixture.0, 42, "boot", 0, 100).unwrap().is_none());
    }

    #[test]
    fn credentials_require_all_uid_columns_to_match() {
        assert_eq!(
            parse_uid_status(b"Name:\tnode\nUid:\t1000\t1000\t1000\t1000\n").unwrap(),
            1000
        );
        assert_eq!(
            parse_uid_status(b"Uid:\t1000\t0\t1000\t1000\n")
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            parse_uid_status(b"Uid:\t1000\t1000\n").unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn environment_read_keeps_only_bounded_agent_markers() {
        let input = Vec::from(&b"PATH=/private\0AGENTDUST_SESSION=claude\0CODEX_SESSION_ID=codex\0"[..]);
        let parsed = parse_environment(std::io::Cursor::new(input)).unwrap();
        assert_eq!(parsed.claude, [b"claude".to_vec()]);
        assert_eq!(parsed.codex, [b"codex".to_vec()]);

        let oversized = vec![b'x'; super::MAX_ENV_BYTES as usize + 1];
        assert_eq!(
            parse_environment(std::io::Cursor::new(oversized))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn cgroup_paths_are_labeled_evidence_and_user_scopes_are_selected() {
        let scope =
            cgroup_unit_component(b"/user.slice/user-1000.slice/user@1000.service/app.slice/app-foo.scope")
                .unwrap();
        assert_eq!(scope, b"app-foo.scope");
        assert!(cgroup_unit_component(b"/system.slice/ssh.service").is_some());
        assert!(cgroup_unit_component(b"/user.slice/user@1000.service").is_some());
        assert!(cgroup_unit_component(b"/user.slice/session-2.scope/delegated.service").is_some());
        assert!(cgroup_unit_component(b"/user.slice/session-2").is_none());
    }

    #[test]
    fn managed_inventory_requires_a_unified_cgroup_record() {
        assert!(cgroup_unit_evidence(b"0::/system.slice/ssh.service\n").unwrap());
        assert!(cgroup_unit_evidence(b"0::/system.slice/transient-2.scope\n").unwrap());
        assert!(!cgroup_unit_evidence(b"0::/user.slice/session-2.scope\n").unwrap());
        assert!(!cgroup_unit_evidence(b"0::/user.slice/user-1000.slice/user@1000.service\n").unwrap());
        assert_eq!(
            cgroup_unit_evidence(b"2:cpu:/system.slice/ssh.service\n")
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::Unsupported
        );
    }

    #[test]
    fn stale_identity_is_rejected_before_reading_live_details() {
        let fixture = ProcFixture::new();
        fixture.process(4001, 500, 1);
        let provider = LinuxProvider::with_proc_root(fixture.0.clone()).unwrap();
        let current = provider.info(4001).unwrap().unwrap().identity;
        let mut stale = current;
        stale.start_time_us = stale.start_time_us.saturating_sub(1);
        let mut read = false;
        assert_eq!(
            provider.verified(&stale, |_| {
                read = true;
                Ok(Some(()))
            }),
            None
        );
        assert!(!read);
    }

    #[test]
    fn script_arguments_are_bound_to_the_observed_identity() {
        use crate::ancestry::AncestryProvider;

        let fixture = ProcFixture::new();
        fixture.process(4005, 500, 1);
        fs::write(fixture.0.join("4005/cmdline"), b"node\0claude.js\0").unwrap();
        let provider = LinuxProvider::with_proc_root(fixture.0.clone()).unwrap();
        let current = provider.info(4005).unwrap().unwrap().identity;
        assert_eq!(
            provider.script_argument_for(&current).unwrap(),
            Some(b"claude.js".to_vec())
        );
        let mut stale = current;
        stale.start_time_us = stale.start_time_us.saturating_sub(1);
        assert_eq!(provider.script_argument_for(&stale).unwrap(), None);
    }

    #[test]
    fn identity_change_during_a_live_read_is_rejected() {
        let fixture = ProcFixture::new();
        fixture.process(4002, 500, 1);
        let provider = LinuxProvider::with_proc_root(fixture.0.clone()).unwrap();
        let current = provider.info(4002).unwrap().unwrap().identity;
        assert_eq!(
            provider.verified(&current, |pid| {
                fs::write(
                    fixture.0.join(pid.to_string()).join("stat"),
                    stat_line(pid, "node (worker)", 1, 501),
                )?;
                Ok(Some(()))
            }),
            None
        );
    }

    #[test]
    fn unreadable_process_identity_is_not_reported_as_gone() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = ProcFixture::new();
        fixture.process(4004, 500, 1);
        fs::set_permissions(fixture.0.join("4004/status"), fs::Permissions::from_mode(0o0)).unwrap();
        let provider = LinuxProvider::with_proc_root(fixture.0.clone()).unwrap();
        let error = provider.info(4004).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn reparenting_preserves_process_identity_and_reports_the_new_parent() {
        let fixture = ProcFixture::new();
        fixture.process(4003, 500, 1);
        let provider = LinuxProvider::with_proc_root(fixture.0.clone()).unwrap();
        let current = provider.info(4003).unwrap().unwrap().identity;
        fixture.process(4003, 500, 12);
        assert_eq!(provider.parent(&current).unwrap(), Some(12));
    }
}
