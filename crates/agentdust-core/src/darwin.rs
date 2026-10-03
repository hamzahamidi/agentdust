use std::ffi::{CStr, OsString};
use std::io;
use std::mem::{MaybeUninit, size_of};
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::ptr;

use crate::identity::{KernelIdentity, ProcessInfo};

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
    // SAFETY: `info` points to `size` writable bytes for a proc_bsdinfo.
    let written =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, info.as_mut_ptr().cast(), size) };
    if written <= 0 {
        return missing_or(io::Error::last_os_error());
    }
    if written != size {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "short proc_bsdinfo"));
    }
    // SAFETY: the kernel filled all `size` bytes, checked above.
    let info = unsafe { info.assume_init() };
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

fn missing_or<T>(err: io::Error) -> io::Result<Option<T>> {
    match err.raw_os_error() {
        Some(libc::ESRCH) | Some(libc::EINVAL) => Ok(None),
        _ => Err(err),
    }
}
