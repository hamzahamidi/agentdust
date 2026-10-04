#![allow(dead_code)]

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use agentdust_core::journal::volume::FixedVolume;
use agentdust_core::secret::{NoProbe, SECRET_FILE, Secret, SecretError, load_or_create_with};

static APFS: LazyLock<FixedVolume> = LazyLock::new(FixedVolume::apfs_local);

pub fn apfs() -> &'static FixedVolume {
    &APFS
}

pub fn load_or_create(dir: &Path) -> Result<Secret, SecretError> {
    load_or_create_with(dir, apfs(), &NoProbe)
}

pub fn names(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect()
}

pub fn only_the_secret() -> BTreeSet<String> {
    BTreeSet::from([SECRET_FILE.to_owned()])
}

pub fn mode_of(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

pub fn place(dir: &Path, content: &[u8], mode: u32) -> PathBuf {
    let path = dir.join(SECRET_FILE);
    fs::write(&path, content).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    path
}
