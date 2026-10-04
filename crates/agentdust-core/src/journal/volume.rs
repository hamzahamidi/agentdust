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

#[cfg(not(target_os = "macos"))]
impl VolumeProbe for SystemVolume {
    fn probe(&self, _path: &Path) -> io::Result<FsFacts> {
        Ok(FsFacts {
            name: "unknown".to_owned(),
            local: false,
            supported: false,
        })
    }
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
