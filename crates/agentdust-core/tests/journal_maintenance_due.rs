mod journal_support;
mod maintenance_support;
mod scratch;

use agentdust_core::journal::retention::Policy;
use agentdust_core::journal::{ACTIVE_ROTATE_BYTES, Rotation};
use journal_support::{frame, journal, plant, record};
use maintenance_support::{BOOT, T, hold_exclusively};
use scratch::TempDir;
use std::fs::FileTimes;
use std::time::{Duration, UNIX_EPOCH};

#[test]
fn first_use_runs_maintenance_then_waits_for_the_interval() {
    let dir = TempDir::private("maintenance-first-use");
    let journal = journal(&dir);
    journal.append(&record("session", BOOT, T, 1)).unwrap();

    let first = journal
        .maintain_if_due(&Policy::default(), T, BOOT)
        .unwrap()
        .unwrap();
    assert_eq!(first.rotation, Rotation::Empty);
    assert!(dir.join("journal.jsonl").exists());

    assert!(
        journal
            .maintain_if_due(&Policy::default(), T, BOOT)
            .unwrap()
            .is_none()
    );

    let later = journal
        .maintain_if_due(
            &Policy::default(),
            T + agentdust_core::journal::MAINTENANCE_INTERVAL_MS,
            BOOT,
        )
        .unwrap()
        .unwrap();
    assert_eq!(later.rotation, Rotation::Empty);
}

#[test]
fn maintenance_skips_when_another_run_holds_the_lock() {
    let dir = TempDir::private("maintenance-busy");
    let _held = hold_exclusively(&dir);

    assert!(
        journal(&dir)
            .maintain_if_due(&Policy::default(), T, BOOT)
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_large_active_journal_runs_maintenance_before_the_interval() {
    let dir = TempDir::private("maintenance-large-active");
    let marker = agentdust_core::safe_open::open_lock_file(&dir.join("journal.maint")).unwrap();
    marker
        .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_millis(T)))
        .unwrap();

    let item = frame(&record("session", BOOT, T, 1));
    let mut active = Vec::with_capacity(ACTIVE_ROTATE_BYTES as usize + item.len());
    while active.len() < ACTIVE_ROTATE_BYTES as usize {
        active.extend_from_slice(&item);
    }
    plant(&dir, "journal.jsonl", &active);

    let result = journal(&dir)
        .maintain_if_due(&Policy::default(), T + 1, BOOT)
        .unwrap()
        .unwrap();

    assert_eq!(result.rotation, Rotation::Rotated { stamp: T + 1 });
}

#[test]
fn a_missing_data_directory_stays_missing() {
    let dir = TempDir::absent("maintenance-missing");
    assert!(
        journal(&dir)
            .maintain_if_due(&Policy::default(), T, BOOT)
            .unwrap()
            .is_none()
    );
    assert!(!dir.exists());
}
