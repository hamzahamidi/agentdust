use std::io;

pub(crate) fn bytes<const N: usize>() -> io::Result<[u8; N]> {
    let mut bytes = [0u8; N];
    // SAFETY: the pointer and length describe the writable array, and N is within the 256 byte limit.
    let status = unsafe { libc::getentropy(bytes.as_mut_ptr().cast(), N) };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(bytes)
}
