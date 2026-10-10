use std::fs;
use std::io;
use std::path::Path;

pub const MNT_LOCAL: u32 = 0x0000_1000;
const SUPPORTED_NAME: &str = "apfs";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsFacts {
    pub name: String,
    pub local: bool,
    pub supported: bool,
}

impl FsFacts {
    pub fn describe(&self) -> String {
        format!(
            "{}, {}, {}",
            self.name,
            if self.local { "local" } else { "not local" },
            if self.supported {
                "supported"
            } else {
                "not supported"
            }
        )
    }
}

pub fn classify(name: &str, flags: u32) -> FsFacts {
    let local = flags & MNT_LOCAL != 0;
    FsFacts {
        name: name.to_owned(),
        local,
        supported: local && name == SUPPORTED_NAME,
    }
}

pub trait VolumeProbe: Sync {
    fn probe(&self, path: &Path) -> io::Result<FsFacts>;
}

pub struct SystemVolume;

#[cfg(target_os = "macos")]
impl VolumeProbe for SystemVolume {
    fn probe(&self, path: &Path) -> io::Result<FsFacts> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let c_path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "the path holds a NUL byte"))?;
        // SAFETY: statfs is plain old data and all zero bytes is a valid value.
        let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: `c_path` is NUL terminated and `stat` is a writable statfs for the whole call.
        if unsafe { libc::statfs(c_path.as_ptr(), &mut stat) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let name: Vec<u8> = stat
            .f_fstypename
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| *byte as u8)
            .collect();
        Ok(classify(&String::from_utf8_lossy(&name), stat.f_flags))
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
impl VolumeProbe for SystemVolume {
    fn probe(&self, _path: &Path) -> io::Result<FsFacts> {
        Ok(FsFacts {
            name: "unknown".to_owned(),
            local: false,
            supported: false,
        })
    }
}

#[cfg(target_os = "linux")]
impl VolumeProbe for SystemVolume {
    fn probe(&self, path: &Path) -> io::Result<FsFacts> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        const EXT4_SUPER_MAGIC: u64 = 0xef53;
        const XFS_SUPER_MAGIC: u64 = 0x5846_5342;
        const BTRFS_SUPER_MAGIC: u64 = 0x9123_683e;

        let c_path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "the path holds a NUL byte"))?;
        // SAFETY: statfs is plain old data and all zero bytes is a valid value.
        let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: c_path is NUL terminated and stat is a writable statfs for the whole call.
        if unsafe { libc::statfs(c_path.as_ptr(), &mut stat) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let filesystem = match mount_filesystem(path) {
            Ok(Some(filesystem)) => filesystem,
            Ok(None) => {
                return Ok(FsFacts {
                    name: "unknown".into(),
                    local: false,
                    supported: false,
                });
            }
            Err(_) => {
                return Ok(FsFacts {
                    name: "unknown".into(),
                    local: false,
                    supported: false,
                });
            }
        };
        let name = String::from_utf8_lossy(&filesystem).into_owned();
        let local = matches!(
            name.as_str(),
            "ext2" | "ext3" | "ext4" | "xfs" | "btrfs" | "f2fs" | "tmpfs" | "overlay"
        );
        let magic_matches = match name.as_str() {
            "ext4" => stat.f_type as u64 == EXT4_SUPER_MAGIC,
            "xfs" => stat.f_type as u64 == XFS_SUPER_MAGIC,
            "btrfs" => stat.f_type as u64 == BTRFS_SUPER_MAGIC,
            _ => false,
        };
        if matches!(name.as_str(), "ext4" | "xfs" | "btrfs") && !magic_matches {
            return Ok(FsFacts {
                name: "unknown".into(),
                local: false,
                supported: false,
            });
        }
        Ok(FsFacts {
            name,
            local,
            supported: false,
        })
    }
}

#[cfg(target_os = "linux")]
fn mount_filesystem(path: &Path) -> io::Result<Option<Vec<u8>>> {
    use std::io::Read;
    use std::os::unix::ffi::OsStrExt;

    let canonical = std::fs::canonicalize(path)?;
    let target = canonical.as_os_str().as_bytes();
    let mut file = std::fs::File::open("/proc/self/mountinfo")?;
    let mut mounts = Vec::new();
    file.by_ref().take(4 * 1024 * 1024 + 1).read_to_end(&mut mounts)?;
    if mounts.len() > 4 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "mount inventory exceeds the read limit",
        ));
    }
    let mut best: Option<(usize, Vec<u8>)> = None;
    for line in mounts
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let fields: Vec<&[u8]> = line
            .split(|byte| byte.is_ascii_whitespace())
            .filter(|field| !field.is_empty())
            .collect();
        let Some(separator) = fields.iter().position(|field| *field == b"-") else {
            continue;
        };
        let (Some(mount), Some(filesystem)) = (fields.get(4), fields.get(separator + 1)) else {
            continue;
        };
        let Some(mount) = decode_mount_path(mount) else {
            continue;
        };
        let below_mount = if mount == b"/" {
            target.starts_with(b"/")
        } else {
            target == mount.as_slice()
                || target
                    .strip_prefix(mount.as_slice())
                    .is_some_and(|rest| rest.starts_with(b"/"))
        };
        if below_mount && best.as_ref().is_none_or(|(length, _)| mount.len() > *length) {
            best = Some((mount.len(), filesystem.to_vec()));
        }
    }
    Ok(best.map(|(_, filesystem)| filesystem))
}

#[cfg(target_os = "linux")]
fn decode_mount_path(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'\\' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        let digits = bytes.get(index + 1..index + 4)?;
        if !digits.iter().all(|digit| (b'0'..=b'7').contains(digit)) {
            return None;
        }
        let value =
            u16::from(digits[0] - b'0') * 64 + u16::from(digits[1] - b'0') * 8 + u16::from(digits[2] - b'0');
        decoded.push(u8::try_from(value).ok()?);
        index += 4;
    }
    Some(decoded)
}

pub struct FixedVolume(FsFacts);

impl FixedVolume {
    pub fn new(facts: FsFacts) -> Self {
        Self(facts)
    }

    pub fn apfs_local() -> Self {
        Self(classify(SUPPORTED_NAME, MNT_LOCAL))
    }
}

impl VolumeProbe for FixedVolume {
    fn probe(&self, _path: &Path) -> io::Result<FsFacts> {
        Ok(self.0.clone())
    }
}

pub fn locate(probe: &dyn VolumeProbe, dir: &Path) -> io::Result<FsFacts> {
    for ancestor in dir.ancestors() {
        let target = if ancestor.as_os_str().is_empty() {
            Path::new(".")
        } else {
            ancestor
        };
        match fs::symlink_metadata(target) {
            Ok(_) => return probe.probe(target),
            Err(err) if matches!(err.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) => {}
            Err(err) => return Err(err),
        }
    }
    probe.probe(Path::new("."))
}
