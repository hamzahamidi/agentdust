#![cfg(target_os = "linux")]

mod journal_support;

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::PathBuf;

use agentdust_core::clock;
use agentdust_core::journal::volume::{SystemVolume, VolumeProbe};
use agentdust_core::journal::{Journal, Rotation};
use agentdust_core::platform;
use journal_support::{record, sessions};

#[test]
#[ignore = "requires an ext4 test mount"]
fn ext4_journal_append_rotation_and_torn_tail_recovery() {
    let mount = ext4_root();
    let dir = mount.join(format!("agentdust-journal-ext4-{}", std::process::id()));
    fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();

    let facts = SystemVolume.probe(&dir).unwrap();
    assert_eq!(facts.name, "ext4");
    assert!(facts.local && facts.supported);

    let journal = Journal::new(&dir);
    let boot = platform::boot_identity().unwrap();
    let now = clock::wall_ms();
    for index in 0..3 {
        journal
            .append(&record(&format!("session-{index}"), &boot, now + index, index))
            .unwrap();
    }
    assert_eq!(
        fs::metadata(dir.join("journal.jsonl"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    let mut active = OpenOptions::new()
        .append(true)
        .open(dir.join("journal.jsonl"))
        .unwrap();
    active.write_all(b"\x1e{\"v\":").unwrap();

    let recovered = journal.read().unwrap();
    assert_eq!(
        sessions(&recovered.records),
        ["session-0", "session-1", "session-2"]
    );
    assert_eq!(recovered.torn_frames, 1);

    assert!(matches!(
        journal.rotate(now + 10).unwrap(),
        Rotation::Rotated { .. }
    ));
    journal
        .append(&record("after-rotation", &boot, now + 11, 11))
        .unwrap();
    let recovered = journal.read().unwrap();
    assert_eq!(
        sessions(&recovered.records),
        ["session-0", "session-1", "session-2", "after-rotation"]
    );
    assert_eq!(recovered.torn_frames, 1);

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "requires an ext4 test mount remounted read only"]
fn read_only_ext4_mount_is_not_supported() {
    let facts = SystemVolume
        .probe(&ext4_root())
        .expect("the ext4 test mount must remain readable");
    assert_eq!(facts.name, "ext4");
    assert!(facts.local);
    assert!(!facts.supported);
}

fn ext4_root() -> PathBuf {
    PathBuf::from(std::env::var_os("AGENTDUST_EXT4_TEST_ROOT").unwrap())
}
