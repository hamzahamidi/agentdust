use std::collections::BTreeSet;
use std::fs;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use crate::ancestry::AncestryProvider;
use crate::identity::{KernelIdentity, ProcessInfo};
use crate::inventory::{ArgsView, Cpu, LiveDetails, ProcessSource, RawIdentity, RawProcess, Tag};
use crate::provider::{ProcessProvider, ProcessRead};
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

#[derive(Debug, Default)]
struct SessionEnv {
    claude: Vec<Vec<u8>>,
    codex: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcStat {
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
    Ok(ProcStat {
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
            return Ok(cgroup_unit_component(path).is_some());
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
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{
        LinuxProvider, ProcStat, cgroup_unit_component, cgroup_unit_evidence, parse_environment, parse_stat,
        parse_uid_status,
    };
    use crate::ancestry::AncestryProvider;

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

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
        let fields = [
            "S".to_owned(),
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
                ppid: 7,
                pgid: 3,
                user_ticks: 11,
                system_ticks: 13,
                start_ticks: 91,
            }
        );
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
        assert!(!cgroup_unit_evidence(b"0::/user.slice/session-2\n").unwrap());
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
