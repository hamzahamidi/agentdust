use agentdust_bench::fsinfo::{FsFacts, MNT_LOCAL, classify};

#[test]
fn a_local_apfs_volume_is_supported() {
    let facts = classify("apfs", MNT_LOCAL);
    assert_eq!(
        facts,
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
    for name in ["hfs", "exfat", "msdos", "udf", "tmpfs", "ext4"] {
        let facts = classify(name, MNT_LOCAL);
        assert!(facts.local, "{name}");
        assert!(!facts.supported, "{name}");
    }
}

#[test]
fn network_volumes_are_not_supported_whatever_flags_they_report() {
    for name in ["nfs", "smbfs", "afpfs", "webdav", "autofs", "fuse"] {
        let without = classify(name, 0);
        assert!(!without.local && !without.supported, "{name}");
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
fn other_flag_bits_do_not_count_as_local() {
    assert!(!classify("apfs", !MNT_LOCAL).local);
    assert!(classify("apfs", MNT_LOCAL | 0x0000_4000).local);
}

#[test]
fn the_description_names_the_file_system_and_both_flags() {
    assert_eq!(classify("apfs", MNT_LOCAL).describe(), "apfs, local, supported");
    assert_eq!(classify("nfs", 0).describe(), "nfs, not local, not supported");
    assert_eq!(classify("hfs", MNT_LOCAL).describe(), "hfs, local, not supported");
}

#[cfg(target_os = "macos")]
mod on_macos {
    use agentdust_bench::fsinfo::{MNT_LOCAL, probe};

    #[test]
    fn the_constant_matches_the_system_header() {
        assert_eq!(MNT_LOCAL, libc::MNT_LOCAL as u32);
    }

    #[test]
    fn the_temporary_directory_is_a_local_apfs_volume() {
        let facts = probe(&std::env::temp_dir()).unwrap();
        assert_eq!(facts.name, "apfs");
        assert!(facts.local && facts.supported);
    }

    #[test]
    fn the_device_file_system_is_not_a_supported_journal_location() {
        let facts = probe(std::path::Path::new("/dev")).unwrap();
        assert_eq!(facts.name, "devfs");
        assert!(!facts.supported);
    }

    #[test]
    fn a_path_that_does_not_exist_is_an_error() {
        let err = probe(std::path::Path::new("/definitely/not/here")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}

#[cfg(not(target_os = "macos"))]
#[test]
fn off_macos_the_probe_reports_an_unsupported_unknown_volume() {
    let facts = agentdust_bench::fsinfo::probe(&std::env::temp_dir()).unwrap();
    assert_eq!(facts.name, "unknown");
    assert!(!facts.supported);
}
