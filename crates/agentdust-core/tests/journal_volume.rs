mod scratch;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use agentdust_core::journal::volume::{FixedVolume, FsFacts, MNT_LOCAL, VolumeProbe, classify, locate};
use scratch::{private_dir, scratch_dir};

struct Recording {
    asked: Mutex<Vec<PathBuf>>,
    facts: FsFacts,
}

impl Recording {
    fn new() -> Self {
        Self {
            asked: Mutex::new(Vec::new()),
            facts: classify("apfs", MNT_LOCAL),
        }
    }

    fn asked(&self) -> Vec<PathBuf> {
        self.asked.lock().unwrap().clone()
    }
}

impl VolumeProbe for Recording {
    fn probe(&self, path: &Path) -> io::Result<FsFacts> {
        self.asked.lock().unwrap().push(path.to_path_buf());
        Ok(self.facts.clone())
    }
}

struct Failing;

impl VolumeProbe for Failing {
    fn probe(&self, _path: &Path) -> io::Result<FsFacts> {
        Err(io::Error::from(io::ErrorKind::PermissionDenied))
    }
}

#[test]
fn a_local_apfs_volume_is_supported() {
    assert_eq!(
        classify("apfs", MNT_LOCAL),
        FsFacts {
            name: "apfs".to_owned(),
            local: true,
            supported: true
        }
    );
}

#[test]
fn apfs_without_the_local_flag_is_not_supported() {
    let facts = classify("apfs", 0);
    assert!(!facts.local && !facts.supported);
    assert_eq!(facts.name, "apfs");
}

#[test]
fn local_volumes_that_are_not_apfs_are_not_supported() {
    for name in ["hfs", "exfat", "msdos", "udf", "tmpfs", "ext4", "devfs"] {
        let facts = classify(name, MNT_LOCAL);
        assert!(facts.local, "{name}");
        assert!(!facts.supported, "{name}");
    }
}

#[test]
fn network_volumes_are_not_supported_whatever_flags_they_report() {
    for name in ["nfs", "smbfs", "afpfs", "webdav", "autofs", "fuse"] {
        assert!(!classify(name, 0).supported, "{name}");
        assert!(!classify(name, MNT_LOCAL).supported, "{name}");
    }
}

#[test]
fn the_file_system_name_is_compared_exactly() {
    for name in ["APFS", "apfs ", "apfs2", " apfs", "", "apf"] {
        assert!(!classify(name, MNT_LOCAL).supported, "{name:?}");
    }
}

#[test]
fn only_the_local_bit_counts_as_local() {
    assert!(!classify("apfs", !MNT_LOCAL).local);
    assert!(classify("apfs", MNT_LOCAL | 0x0000_4000).local);
}

#[test]
fn the_injected_volume_matrix_of_s24_decides_support() {
    let matrix = [
        ("apfs", MNT_LOCAL, "apfs, local, supported"),
        ("apfs", 0, "apfs, not local, not supported"),
        ("hfs", MNT_LOCAL, "hfs, local, not supported"),
        ("devfs", MNT_LOCAL, "devfs, local, not supported"),
        ("nfs", 0, "nfs, not local, not supported"),
        ("smbfs", 0, "smbfs, not local, not supported"),
        ("autofs", 0, "autofs, not local, not supported"),
    ];
    for (name, flags, described) in matrix {
        let facts = classify(name, flags);
        assert_eq!(facts.describe(), described, "{name}");
        assert_eq!(facts.supported, described.ends_with(", supported"), "{name}");
    }
}

#[test]
fn a_fixed_volume_answers_for_any_path_even_one_that_does_not_exist() {
    let volume = FixedVolume::new(classify("nfs", 0));
    for path in ["/", "/definitely/not/here", "relative/path"] {
        assert_eq!(volume.probe(Path::new(path)).unwrap(), classify("nfs", 0));
    }
    assert_eq!(
        FixedVolume::apfs_local().probe(Path::new("/x")).unwrap(),
        classify("apfs", MNT_LOCAL)
    );
}

#[test]
fn locate_asks_about_the_directory_itself_when_it_exists() {
    let dir = private_dir("volume-existing");
    let probe = Recording::new();
    locate(&probe, &dir).unwrap();
    assert_eq!(probe.asked(), [dir.clone()]);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn locate_asks_about_the_nearest_existing_ancestor_of_a_missing_directory() {
    let root = private_dir("volume-missing");
    let probe = Recording::new();
    locate(&probe, &root.join("a/b/data")).unwrap();
    assert_eq!(probe.asked(), [root.clone()]);
    assert!(!root.join("a").exists());
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn locate_walks_past_a_regular_file_in_the_path() {
    let root = private_dir("volume-file-in-path");
    let file = root.join("state");
    fs::write(&file, b"x").unwrap();
    let probe = Recording::new();
    locate(&probe, &file.join("child/leaf")).unwrap();
    assert_eq!(probe.asked(), [file.clone()]);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn locate_resolves_a_bare_relative_name_against_the_working_directory() {
    let probe = Recording::new();
    locate(&probe, Path::new("agentdust-no-such-directory")).unwrap();
    assert_eq!(probe.asked(), [PathBuf::from(".")]);
}

#[test]
fn locate_reports_what_the_probe_reports() {
    let missing = scratch_dir("volume-nfs");
    let facts = locate(&FixedVolume::new(classify("nfs", 0)), &missing).unwrap();
    assert_eq!(facts, classify("nfs", 0));
}

#[test]
fn locate_passes_a_probe_failure_through() {
    let dir = private_dir("volume-failing");
    let err = locate(&Failing, &dir).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    fs::remove_dir_all(&dir).unwrap();
}

#[cfg(target_os = "macos")]
mod on_macos {
    use std::path::Path;

    use agentdust_core::journal::volume::{MNT_LOCAL, SystemVolume, VolumeProbe, locate};

    #[test]
    fn the_constant_matches_the_system_header() {
        assert_eq!(MNT_LOCAL, libc::MNT_LOCAL as u32);
    }

    #[test]
    fn the_temporary_directory_is_a_local_apfs_volume() {
        let facts = SystemVolume.probe(&std::env::temp_dir()).unwrap();
        assert_eq!(facts.name, "apfs");
        assert!(facts.local && facts.supported);
    }

    #[test]
    fn the_device_file_system_is_not_a_supported_journal_location() {
        let facts = SystemVolume.probe(Path::new("/dev")).unwrap();
        assert_eq!(facts.name, "devfs");
        assert!(!facts.supported);
    }

    #[test]
    fn a_path_that_does_not_exist_is_an_error_for_the_system_probe() {
        let err = SystemVolume.probe(Path::new("/definitely/not/here")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn a_directory_that_does_not_exist_yet_takes_the_volume_of_its_parent() {
        let facts = locate(
            &SystemVolume,
            &std::env::temp_dir().join("agentdust-not-yet/data"),
        )
        .unwrap();
        assert!(facts.supported);
        let facts = locate(&SystemVolume, Path::new("/dev/agentdust-not-yet")).unwrap();
        assert_eq!(facts.name, "devfs");
    }
}

#[cfg(not(target_os = "macos"))]
#[test]
fn off_macos_the_system_probe_reports_an_unsupported_unknown_volume() {
    use agentdust_core::journal::volume::SystemVolume;

    let facts = SystemVolume.probe(&std::env::temp_dir()).unwrap();
    assert_eq!(facts.name, "unknown");
    assert!(!facts.local && !facts.supported);
}
