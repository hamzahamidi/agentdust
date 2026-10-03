use std::ffi::{CStr, OsString};
use std::io;
use std::mem::{MaybeUninit, size_of};
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::ptr;

use crate::identity::{KernelIdentity, ProcessInfo};
use crate::procargs;

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

fn missing_or<T>(err: io::Error) -> io::Result<Option<T>> {
    match err.raw_os_error() {
        Some(libc::ESRCH) | Some(libc::EINVAL) => Ok(None),
        _ => Err(err),
    }
}
