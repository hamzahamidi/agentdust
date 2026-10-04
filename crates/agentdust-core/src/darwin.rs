use std::ffi::{CStr, OsString};
use std::io;
use std::mem::{MaybeUninit, size_of};
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::ptr;

use crate::ancestry::AncestryProvider;
use crate::identity::{KernelIdentity, ProcessInfo};
use crate::inventory::{ArgsView, Cpu, LiveDetails, ProcessSource, RawIdentity, RawProcess, Tag, read_args};
use crate::procargs;
use crate::provider::{ProcessProvider, ProcessRead};
use crate::secret::Secret;

const LIST_SLACK: usize = 64;
const NANOS_PER_SECOND: u64 = 1_000_000_000;
const CWD_BYTES: usize = 1024;

pub fn boot_session_uuid() -> io::Result<String> {
    let mut buf = [0u8; 64];
    let mut len = buf.len();
    // SAFETY: `buf` and `len` describe a writable buffer of `len` bytes.
    let rc = unsafe {
        libc::sysctlbyname(
            c"kern.bootsessionuuid".as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    let value = CStr::from_bytes_until_nul(&buf[..len]).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "boot session UUID is not NUL terminated",
        )
    })?;
    value
        .to_str()
        .map(str::to_owned)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "boot session UUID is not UTF-8"))
}

pub fn process_info(pid: i32, boot_session_uuid: &str) -> io::Result<Option<ProcessInfo>> {
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `info` is `size` writable bytes, and an all-zero proc_bsdinfo is a valid value.
    let (written, info) = unsafe {
        let written = libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, info.as_mut_ptr().cast(), size);
        (written, info.assume_init())
    };
    if written <= 0 {
        return missing_or(io::Error::last_os_error());
    }
    if written != size {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "short proc_bsdinfo"));
    }
    Ok(Some(ProcessInfo {
        identity: KernelIdentity {
            boot_session_uuid: boot_session_uuid.to_owned(),
            pid,
            start_time_us: info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
            uid: info.pbi_uid,
        },
        ppid: info.pbi_ppid as i32,
        pgid: info.pbi_pgid as i32,
    }))
}

pub fn exe_path(pid: i32) -> io::Result<Option<PathBuf>> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: `buf` is writable for `buf.len()` bytes.
    let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if len <= 0 {
        return missing_or(io::Error::last_os_error());
    }
    buf.truncate(len as usize);
    Ok(Some(PathBuf::from(OsString::from_vec(buf))))
}

pub fn procargs2(pid: i32) -> io::Result<Option<Vec<u8>>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut len: libc::size_t = 0;
    // SAFETY: a null buffer asks the kernel for the required size only.
    let rc = unsafe { libc::sysctl(mib.as_mut_ptr(), 3, ptr::null_mut(), &mut len, ptr::null_mut(), 0) };
    if rc != 0 {
        return missing_or(io::Error::last_os_error());
    }
    let mut buf = vec![0u8; len];
    // SAFETY: `buf` is writable for `len` bytes.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr().cast(),
            &mut len,
            ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return missing_or(io::Error::last_os_error());
    }
    buf.truncate(len);
    Ok(Some(buf))
}

pub fn env_var(pid: i32, name: &str) -> io::Result<Option<Vec<u8>>> {
    let Some(buf) = procargs2(pid)? else {
        return Ok(None);
    };
    let parsed = procargs::parse(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(procargs::env_value(&parsed, name).map(<[u8]>::to_vec))
}

pub struct DarwinProvider {
    boot_session_uuid: String,
}

impl DarwinProvider {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            boot_session_uuid: boot_session_uuid()?,
        })
    }
}

impl ProcessProvider for DarwinProvider {
    fn read(&self, pid: i32) -> io::Result<ProcessRead> {
        let boot = &self.boot_session_uuid;
        let Some(before) = process_info(pid, boot)? else {
            return Ok(ProcessRead::Gone);
        };
        let path = exe_path(pid).ok().flatten();
        let after = process_info(pid, boot)?;
        Ok(ProcessRead::from_samples(
            Some(before.identity),
            path,
            after.map(|info| info.identity),
        ))
    }
}

impl AncestryProvider for DarwinProvider {
    fn parent(&self, identity: &KernelIdentity) -> io::Result<Option<i32>> {
        match process_info(identity.pid, &self.boot_session_uuid)? {
            Some(info) if info.identity == *identity => Ok(Some(info.ppid)),
            _ => Ok(None),
        }
    }

    fn script_argument(&self, pid: i32) -> io::Result<Option<Vec<u8>>> {
        let Some(buf) = procargs2(pid)? else {
            return Ok(None);
        };
        let script =
            procargs::script_argument(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(script.map(<[u8]>::to_vec))
    }
}

pub fn list_pids() -> io::Result<Vec<i32>> {
    // SAFETY: a null buffer of size 0 only asks for the number of PIDs.
    let counted = unsafe { libc::proc_listallpids(ptr::null_mut(), 0) };
    if counted < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut capacity = counted as usize + LIST_SLACK;
    loop {
        let mut pids = vec![0i32; capacity];
        let bytes = (capacity * size_of::<i32>()) as libc::c_int;
        // SAFETY: `pids` is writable for `bytes` bytes.
        let filled = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        if filled < 0 {
            return Err(io::Error::last_os_error());
        }
        let filled = filled as usize;
        if filled < capacity {
            pids.truncate(filled);
            pids.retain(|pid| *pid > 0);
            return Ok(pids);
        }
        capacity *= 2;
    }
}

pub fn tick_frequency() -> u64 {
    let mut value: u64 = 0;
    let mut len = size_of::<u64>();
    // SAFETY: `value` and `len` describe a writable 8 byte buffer.
    let rc = unsafe {
        libc::sysctlbyname(
            c"hw.tbfrequency".as_ptr(),
            ptr::from_mut(&mut value).cast(),
            &mut len,
            ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || len != size_of::<u64>() || value == 0 {
        return NANOS_PER_SECOND;
    }
    value
}

pub fn ticks_to_ns(ticks: u64, frequency: u64) -> u64 {
    if frequency == 0 {
        return ticks;
    }
    let nanos = u128::from(ticks) * u128::from(NANOS_PER_SECOND) / u128::from(frequency);
    u64::try_from(nanos).unwrap_or(u64::MAX)
}

fn task_cpu_ticks(pid: i32) -> io::Result<Option<u64>> {
    let mut info = MaybeUninit::<libc::proc_taskinfo>::zeroed();
    let size = size_of::<libc::proc_taskinfo>() as libc::c_int;
    // SAFETY: `info` is `size` writable bytes, and an all-zero proc_taskinfo is a valid value.
    let (written, info) = unsafe {
        let written = libc::proc_pidinfo(pid, libc::PROC_PIDTASKINFO, 0, info.as_mut_ptr().cast(), size);
        (written, info.assume_init())
    };
    if written <= 0 {
        return missing_or(io::Error::last_os_error());
    }
    if written != size {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "short proc_taskinfo"));
    }
    Ok(Some(info.pti_total_user.saturating_add(info.pti_total_system)))
}

pub fn cwd_path(pid: i32) -> io::Result<Option<PathBuf>> {
    let mut info = MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
    let size = size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    // SAFETY: `info` is `size` writable bytes, and an all-zero proc_vnodepathinfo is a valid value.
    let (written, info) = unsafe {
        let written = libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        );
        (written, info.assume_init())
    };
    if written <= 0 {
        return missing_or(io::Error::last_os_error());
    }
    // SAFETY: vip_path is a contiguous array of CWD_BYTES bytes inside `info`.
    let bytes =
        unsafe { std::slice::from_raw_parts(info.pvi_cdir.vip_path.as_ptr().cast::<u8>(), CWD_BYTES) };
    let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(CWD_BYTES);
    if end == 0 {
        return Ok(None);
    }
    Ok(Some(PathBuf::from(OsString::from_vec(bytes[..end].to_vec()))))
}

fn current_uid() -> u32 {
    // SAFETY: geteuid takes no arguments and cannot fail.
    unsafe { libc::geteuid() }
}

pub struct DarwinSource<'a> {
    boot: String,
    secret: Option<&'a Secret>,
    frequency: u64,
}

impl<'a> DarwinSource<'a> {
    pub fn new(secret: Option<&'a Secret>) -> io::Result<Self> {
        Ok(Self {
            boot: boot_session_uuid()?,
            secret,
            frequency: tick_frequency(),
        })
    }

    fn cpu_ns(&self, pid: i32) -> io::Result<Option<u64>> {
        Ok(task_cpu_ticks(pid)?.map(|ticks| ticks_to_ns(ticks, self.frequency)))
    }

    fn read_process(&self, pid: i32) -> Option<RawProcess> {
        let before = process_info(pid, &self.boot).ok()??;
        let exe_path = exe_path(pid).ok().flatten();
        let first_ns = self.cpu_ns(pid).ok().flatten();
        let unreadable = ArgsView {
            tag: Tag::Unreadable,
            agent_script: false,
        };
        let view = if before.identity.uid == current_uid() {
            match procargs2(pid) {
                Ok(buf) => read_args(buf.as_deref(), exe_path.as_deref(), self.secret),
                Err(_) => unreadable,
            }
        } else {
            unreadable
        };
        let after = process_info(pid, &self.boot).ok()??;
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
                first_ns,
                later_ns: None,
            },
        })
    }
}

impl ProcessSource for DarwinSource<'_> {
    fn scan(&self) -> io::Result<Vec<RawProcess>> {
        Ok(list_pids()?
            .into_iter()
            .filter_map(|pid| self.read_process(pid))
            .collect())
    }

    fn cpu_time_ns(&self, identity: &KernelIdentity) -> io::Result<Option<u64>> {
        if identity.boot_session_uuid != self.boot || identity.pid <= 0 {
            return Ok(None);
        }
        let Some(before) = process_info(identity.pid, &self.boot)? else {
            return Ok(None);
        };
        if before.identity != *identity {
            return Ok(None);
        }
        let Some(ns) = self.cpu_ns(identity.pid)? else {
            return Ok(None);
        };
        match process_info(identity.pid, &self.boot)? {
            Some(after) if after.identity == *identity => Ok(Some(ns)),
            _ => Ok(None),
        }
    }
}

pub struct DarwinLive {
    boot: String,
}

impl DarwinLive {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            boot: boot_session_uuid()?,
        })
    }

    fn verified<T>(
        &self,
        identity: &KernelIdentity,
        read: impl FnOnce(i32) -> io::Result<Option<T>>,
    ) -> Option<T> {
        if identity.boot_session_uuid != self.boot || identity.pid <= 0 {
            return None;
        }
        let before = process_info(identity.pid, &self.boot).ok()??;
        if before.identity != *identity {
            return None;
        }
        let value = read(identity.pid).ok()??;
        let after = process_info(identity.pid, &self.boot).ok()??;
        (after.identity == *identity).then_some(value)
    }
}

impl LiveDetails for DarwinLive {
    fn command(&self, identity: &KernelIdentity) -> Option<Vec<Vec<u8>>> {
        self.verified(identity, |pid| {
            let Some(buf) = procargs2(pid)? else {
                return Ok(None);
            };
            let parsed = procargs::parse(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            Ok(Some(parsed.args.iter().map(|arg| arg.to_vec()).collect()))
        })
    }

    fn cwd(&self, identity: &KernelIdentity) -> Option<PathBuf> {
        self.verified(identity, cwd_path)
    }
}

fn missing_or<T>(err: io::Error) -> io::Result<Option<T>> {
    match err.raw_os_error() {
        Some(libc::ESRCH) | Some(libc::EINVAL) => Ok(None),
        _ => Err(err),
    }
}
