const HEADER: &str = "!#acl";

pub fn has_allow_entry(text: &str) -> bool {
    text.lines()
        .filter(|line| !line.is_empty() && !line.starts_with(HEADER))
        .any(|line| !is_deny_entry(line))
}

fn is_deny_entry(line: &str) -> bool {
    let mut fields = line.rsplit(':');
    let _permissions = fields.next();
    fields.next().and_then(|decision| decision.split(',').next()) == Some("deny")
}

#[cfg(target_os = "macos")]
mod darwin {
    use std::ffi::{CStr, c_char, c_int, c_void};
    use std::io;

    const ACL_TYPE_EXTENDED: c_int = 0x100;

    unsafe extern "C" {
        fn acl_get_fd_np(fd: c_int, acl_type: c_int) -> *mut c_void;
        fn acl_to_text(acl: *mut c_void, len: *mut libc::ssize_t) -> *mut c_char;
        fn acl_free(object: *mut c_void) -> c_int;
    }

    pub(super) fn text_of(fd: c_int) -> io::Result<Option<String>> {
        let mut len: libc::ssize_t = 0;
        // SAFETY: the calls follow acl(3): a non-null acl_t and text are each freed once, the text is NUL terminated, and nothing is used after its free.
        unsafe {
            let acl = acl_get_fd_np(fd, ACL_TYPE_EXTENDED);
            if acl.is_null() {
                let err = io::Error::last_os_error();
                return match err.raw_os_error() {
                    Some(libc::ENOENT) => Ok(None),
                    _ => Err(err),
                };
            }
            let text = acl_to_text(acl, &mut len);
            let found = if text.is_null() {
                Err(io::Error::last_os_error())
            } else {
                let owned = CStr::from_ptr(text).to_string_lossy().into_owned();
                acl_free(text.cast());
                Ok(Some(owned))
            };
            acl_free(acl);
            found
        }
    }
}

#[cfg(target_os = "macos")]
pub fn extended_acl_has_allow_entry(fd: std::os::fd::RawFd) -> std::io::Result<bool> {
    Ok(darwin::text_of(fd)?.is_some_and(|text| has_allow_entry(&text)))
}

#[cfg(not(target_os = "macos"))]
pub fn extended_acl_has_allow_entry(_fd: std::os::fd::RawFd) -> std::io::Result<bool> {
    Ok(false)
}
