mod scratch;
mod secret_support;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use agentdust_core::journal::volume::{FixedVolume, FsFacts, MNT_LOCAL, VolumeProbe, classify};
use agentdust_core::secret::{NoProbe, SECRET_FILE, SecretError, load_or_create_with};
use scratch::{private_dir, scratch_dir};
use secret_support::{apfs, names, place};

struct Recording {
    asked: Mutex<Vec<PathBuf>>,
}

impl VolumeProbe for Recording {
    fn probe(&self, path: &Path) -> io::Result<FsFacts> {
        self.asked.lock().unwrap().push(path.to_path_buf());
        Ok(classify("apfs", MNT_LOCAL))
    }
}

struct Failing;

impl VolumeProbe for Failing {
    fn probe(&self, _path: &Path) -> io::Result<FsFacts> {
        Err(io::Error::from(io::ErrorKind::PermissionDenied))
    }
}

fn unsupported() -> Vec<FsFacts> {
    vec![
        classify("nfs", 0),
        classify("smbfs", 0),
        classify("apfs", 0),
        classify("hfs", MNT_LOCAL),
        classify("devfs", MNT_LOCAL),
        classify("unknown", 0),
    ]
}

fn refused(result: Result<agentdust_core::secret::Secret, SecretError>, facts: &FsFacts) {
    match result {
        Err(SecretError::UnsupportedFilesystem(found)) => assert_eq!(&found, facts),
        other => panic!("{facts:?}: expected a refusal of the volume, got {other:?}"),
    }
}

#[test]
fn a_directory_on_an_unsupported_volume_gets_no_directory_and_no_secret() {
    for facts in unsupported() {
        let dir = scratch_dir("secret-volume-missing");
        let volume = FixedVolume::new(facts.clone());
        refused(load_or_create_with(&dir, &volume, &NoProbe), &facts);
        assert!(!dir.exists(), "{facts:?}");
    }
}

#[test]
fn nothing_is_created_below_an_existing_directory_on_an_unsupported_volume() {
    for facts in unsupported() {
        let parent = private_dir("secret-volume-parent");
        let volume = FixedVolume::new(facts.clone());
        refused(
            load_or_create_with(&parent.join("a/b"), &volume, &NoProbe),
            &facts,
        );
        assert!(names(&parent).is_empty(), "{facts:?}");
        fs::remove_dir_all(&parent).unwrap();
    }
}

#[test]
fn an_existing_data_directory_on_an_unsupported_volume_gets_no_secret() {
    for facts in unsupported() {
        let dir = private_dir("secret-volume-existing");
        let volume = FixedVolume::new(facts.clone());
        refused(load_or_create_with(&dir, &volume, &NoProbe), &facts);
        assert!(names(&dir).is_empty(), "{facts:?}");
        fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn an_existing_secret_on_an_unsupported_volume_is_neither_read_nor_changed() {
    let dir = private_dir("secret-volume-secret");
    let path = place(&dir, &[8u8; 32], 0o600);
    let facts = classify("nfs", 0);
    let volume = FixedVolume::new(facts.clone());
    refused(load_or_create_with(&dir, &volume, &NoProbe), &facts);
    assert_eq!(fs::read(&path).unwrap(), vec![8u8; 32]);
    assert_eq!(names(&dir).len(), 1);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_volume_is_probed_once_on_the_nearest_existing_ancestor() {
    let parent = private_dir("secret-volume-ancestor");
    let volume = Recording {
        asked: Mutex::new(Vec::new()),
    };
    load_or_create_with(&parent.join("data"), &volume, &NoProbe).unwrap();
    assert_eq!(
        volume.asked.lock().unwrap().as_slice(),
        std::slice::from_ref(&parent)
    );
    assert!(parent.join("data").join(SECRET_FILE).exists());
    fs::remove_dir_all(&parent).unwrap();
}

#[test]
fn a_directory_below_a_missing_parent_is_probed_on_the_nearest_ancestor_and_not_created() {
    let parent = private_dir("secret-volume-missing-parent");
    let volume = Recording {
        asked: Mutex::new(Vec::new()),
    };
    let result = load_or_create_with(&parent.join("a/b"), &volume, &NoProbe);
    assert!(
        matches!(&result, Err(SecretError::Io(err)) if err.kind() == io::ErrorKind::NotFound),
        "{result:?}"
    );
    assert_eq!(
        volume.asked.lock().unwrap().as_slice(),
        std::slice::from_ref(&parent)
    );
    assert!(!parent.join("a").exists());
    fs::remove_dir_all(&parent).unwrap();
}

#[test]
fn a_probe_that_fails_stops_the_call_before_anything_is_created() {
    let dir = scratch_dir("secret-volume-failing");
    let result = load_or_create_with(&dir, &Failing, &NoProbe);
    assert!(
        matches!(&result, Err(SecretError::Io(err)) if err.kind() == io::ErrorKind::PermissionDenied),
        "{result:?}"
    );
    assert!(!dir.exists());
}

#[test]
fn the_supported_volume_creates_the_secret() {
    let dir = scratch_dir("secret-volume-apfs");
    load_or_create_with(&dir, apfs(), &NoProbe).unwrap();
    assert!(dir.join(SECRET_FILE).exists());
    fs::remove_dir_all(&dir).unwrap();
}

#[cfg(target_os = "macos")]
mod system {
    use super::*;
    use agentdust_core::secret::load_or_create;

    #[test]
    fn the_temporary_directory_of_this_machine_is_a_supported_volume() {
        let dir = scratch_dir("secret-volume-system");
        let secret = load_or_create(&dir).unwrap();
        assert_eq!(fs::read(dir.join(SECRET_FILE)).unwrap(), secret.as_bytes());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn devfs_is_refused_before_any_directory_is_attempted() {
        let dir = Path::new("/dev/agentdust-secret-volume-test");
        match load_or_create(dir) {
            Err(SecretError::UnsupportedFilesystem(facts)) => assert_eq!(facts.name, "devfs"),
            other => panic!("expected the refusal of the volume, got {other:?}"),
        }
        assert!(!dir.exists());
    }
}
