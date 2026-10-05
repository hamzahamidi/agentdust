mod journal_support;
mod maintenance_support;
mod scratch;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::thread;

use agentdust_core::journal::volume::{FixedVolume, MNT_LOCAL, classify};
use agentdust_core::journal::{Journal, MaintenanceError, MaintenancePoint, Rotation};
use agentdust_core::safe_open::SafeOpenError;
use journal_support::{frame, frames, generation_name, journal, names_in, plant, sessions};
use maintenance_support::{
    MaintenanceGate, Recorded, T, hold_exclusively, inode_of, lock_is_free, mode_of, numbered, refused_by,
    snapshot, without_lock,
};
use scratch::{TempDir, make_fifo, returns_promptly};

fn with_active(name: &str, records: &[&str]) -> TempDir {
    let dir = TempDir::private(name);
    let records: Vec<_> = records
        .iter()
        .enumerate()
        .map(|(i, session)| numbered(session, i as u64 + 1))
        .collect();
    plant(&dir, "journal.jsonl", &frames(&records));
    dir
}

#[test]
fn a_missing_data_directory_is_left_missing() {
    let dir = TempDir::absent("rotate-missing");
    assert_eq!(journal(&dir).rotate(T).unwrap(), Rotation::Empty);
    assert!(!dir.exists());
}

#[test]
fn an_empty_data_directory_gets_only_the_private_maintenance_lock() {
    let dir = TempDir::private("rotate-empty");
    assert_eq!(journal(&dir).rotate(T).unwrap(), Rotation::Empty);
    assert_eq!(names_in(&dir), ["journal.maint"]);
    assert_eq!(mode_of(&dir.join("journal.maint")), 0o600);
    assert_eq!(fs::metadata(dir.join("journal.maint")).unwrap().len(), 0);
}

#[test]
fn the_active_journal_is_renamed_aside_and_never_truncated() {
    let dir = TempDir::private("rotate-aside");
    for n in 1..=3 {
        journal(&dir).append(&numbered("s", n)).unwrap();
    }
    let active = dir.join("journal.jsonl");
    let (bytes, inode) = (fs::read(&active).unwrap(), inode_of(&active));

    let rotation = journal(&dir).rotate(T).unwrap();

    assert_eq!(rotation, Rotation::Rotated { stamp: T });
    assert_eq!(names_in(&dir), [generation_name(T), "journal.maint".to_owned()]);
    let sealed = dir.join(generation_name(T));
    assert_eq!(fs::read(&sealed).unwrap(), bytes);
    assert_eq!(inode_of(&sealed), inode);
    assert_eq!(mode_of(&sealed), 0o600);
}

#[test]
fn an_empty_active_journal_is_not_rotated() {
    let dir = TempDir::private("rotate-empty-active");
    plant(&dir, "journal.jsonl", b"");
    assert_eq!(journal(&dir).rotate(T).unwrap(), Rotation::Empty);
    assert_eq!(names_in(&dir), ["journal.jsonl", "journal.maint"]);
}

#[test]
fn a_rotation_in_an_occupied_millisecond_takes_the_next_free_stamp() {
    let dir = TempDir::private("rotate-stamp");
    plant(&dir, &generation_name(T), &frame(&numbered("old", 1)));
    plant(&dir, "journal.jsonl", &frame(&numbered("new", 2)));

    assert_eq!(
        journal(&dir).rotate(T).unwrap(),
        Rotation::Rotated { stamp: T + 1 }
    );

    assert_eq!(
        names_in(&dir),
        [
            generation_name(T),
            generation_name(T + 1),
            "journal.maint".to_owned()
        ]
    );
    assert_eq!(sessions(&journal(&dir).read().unwrap().records), ["old", "new"]);
}

#[test]
fn a_dangling_symlink_in_the_place_of_a_stamp_is_neither_followed_nor_replaced() {
    let dir = TempDir::private("rotate-stamp-symlink");
    let target = dir.join("elsewhere");
    symlink(&target, dir.join(generation_name(T))).unwrap();
    plant(&dir, "journal.jsonl", &frame(&numbered("new", 1)));

    assert_eq!(
        journal(&dir).rotate(T).unwrap(),
        Rotation::Rotated { stamp: T + 1 }
    );

    assert!(
        fs::symlink_metadata(dir.join(generation_name(T)))
            .unwrap()
            .is_symlink()
    );
    assert!(!target.exists());
}

#[test]
fn an_append_after_a_rotation_starts_a_new_active_file_and_both_are_read() {
    let dir = TempDir::private("rotate-after");
    journal(&dir).append(&numbered("before", 1)).unwrap();
    journal(&dir).rotate(T).unwrap();
    assert!(!dir.join("journal.jsonl").exists());
    journal(&dir).append(&numbered("after", 2)).unwrap();
    assert!(dir.join("journal.jsonl").exists());
    assert_eq!(
        sessions(&journal(&dir).read().unwrap().records),
        ["before", "after"]
    );
}

#[test]
fn a_second_rotation_with_nothing_new_changes_nothing() {
    let dir = with_active("rotate-twice", &["a", "b"]);
    journal(&dir).rotate(T).unwrap();
    let first = snapshot(&dir);
    assert_eq!(journal(&dir).rotate(T + 1).unwrap(), Rotation::Empty);
    assert_eq!(snapshot(&dir), first);
}

#[test]
fn rotation_gives_up_instead_of_looping_when_no_stamp_is_free() {
    let dir = with_active("rotate-no-stamp", &["new"]);
    plant(&dir, &generation_name(u64::MAX), &frame(&numbered("old", 1)));
    let target = dir.to_path_buf();
    let result = returns_promptly(move || journal(&target).rotate(u64::MAX));
    assert!(matches!(result, Err(MaintenanceError::Io(_))), "{result:?}");
    assert!(dir.join("journal.jsonl").exists());
}

#[test]
fn a_lock_held_elsewhere_makes_rotation_busy_and_changes_nothing() {
    let dir = with_active("rotate-busy", &["a"]);
    let held = hold_exclusively(&dir);
    let before = snapshot(&dir);

    let result = journal(&dir).rotate(T);

    assert!(matches!(result, Err(MaintenanceError::Busy)), "{result:?}");
    assert_eq!(snapshot(&dir), before);
    drop(held);
    assert_eq!(journal(&dir).rotate(T).unwrap(), Rotation::Rotated { stamp: T });
}

#[test]
fn the_lock_is_free_again_when_rotation_returns() {
    let dir = with_active("rotate-release", &["a"]);
    journal(&dir).rotate(T).unwrap();
    assert!(lock_is_free(&dir));
    journal(&dir).rotate(T + 1).unwrap();
    assert!(lock_is_free(&dir));
}

#[test]
fn an_appender_and_a_reader_take_no_lock() {
    let dir = with_active("rotate-no-appender-lock", &["a"]);
    let held = hold_exclusively(&dir);

    journal(&dir).append(&numbered("b", 2)).unwrap();
    let report = journal(&dir).read().unwrap();

    assert_eq!(sessions(&report.records), ["a", "b"]);
    assert_eq!(names_in(&dir), ["journal.jsonl", "journal.maint"]);
    drop(held);
}

#[test]
fn a_volume_that_is_not_local_apfs_is_refused_before_anything_is_created() {
    let cases = [
        ("apfs", 0),
        ("hfs", MNT_LOCAL),
        ("nfs", 0),
        ("smbfs", 0),
        ("devfs", MNT_LOCAL),
        ("autofs", 0),
    ];
    for (name, flags) in cases {
        let dir = with_active("rotate-volume", &["a"]);
        let before = snapshot(&dir);
        let volume = FixedVolume::new(classify(name, flags));

        let result = Journal::with_volume(&dir, &volume).rotate(T);

        match result {
            Err(MaintenanceError::UnsupportedFilesystem(facts)) => {
                assert_eq!(facts.name, name);
                assert!(!facts.supported, "{name}");
            }
            other => panic!("{name}: {other:?}"),
        }
        assert_eq!(snapshot(&dir), before, "{name}");
    }
}

#[test]
fn a_symlinked_active_journal_is_refused_and_not_renamed() {
    let dir = TempDir::private("rotate-refuse-symlink");
    let elsewhere = TempDir::private("rotate-refuse-symlink-target");
    plant(&elsewhere, "kept", &frame(&numbered("x", 1)));
    let target = elsewhere.join("kept");
    symlink(&target, dir.join("journal.jsonl")).unwrap();

    let (path, source) = refused_by(journal(&dir).rotate(T));

    assert_eq!(path, dir.join("journal.jsonl"));
    assert!(matches!(source, SafeOpenError::Symlink), "{source:?}");
    assert!(
        fs::symlink_metadata(dir.join("journal.jsonl"))
            .unwrap()
            .is_symlink()
    );
    assert_eq!(names_in(&dir), ["journal.jsonl", "journal.maint"]);
    assert_eq!(fs::read(&target).unwrap(), frame(&numbered("x", 1)));
}

#[test]
fn a_hard_linked_active_journal_is_refused_and_not_renamed() {
    let dir = with_active("rotate-refuse-hardlink", &["a"]);
    fs::hard_link(dir.join("journal.jsonl"), dir.join("alias")).unwrap();
    let before = without_lock(snapshot(&dir));

    let (path, source) = refused_by(journal(&dir).rotate(T));

    assert_eq!(path, dir.join("journal.jsonl"));
    assert!(
        matches!(source, SafeOpenError::HardLinked { links: 2 }),
        "{source:?}"
    );
    assert_eq!(without_lock(snapshot(&dir)), before);
}

#[test]
fn an_active_journal_with_a_loose_mode_is_refused_and_not_renamed() {
    let dir = with_active("rotate-refuse-loose", &["a"]);
    fs::set_permissions(dir.join("journal.jsonl"), fs::Permissions::from_mode(0o644)).unwrap();
    let before = without_lock(snapshot(&dir));

    let (_, source) = refused_by(journal(&dir).rotate(T));

    assert!(
        matches!(source, SafeOpenError::LooseMode { mode: 0o644, .. }),
        "{source:?}"
    );
    assert_eq!(without_lock(snapshot(&dir)), before);
}

#[test]
fn a_fifo_in_the_place_of_the_active_journal_is_refused_without_blocking() {
    let dir = TempDir::private("rotate-refuse-fifo");
    make_fifo(&dir.join("journal.jsonl"));
    let target = dir.to_path_buf();
    let result = returns_promptly(move || journal(&target).rotate(T));
    let (path, source) = refused_by(result);
    assert_eq!(path, dir.join("journal.jsonl"));
    assert!(matches!(source, SafeOpenError::NotRegular), "{source:?}");
}

#[test]
fn a_directory_in_the_place_of_the_active_journal_is_refused() {
    let dir = TempDir::private("rotate-refuse-directory");
    fs::create_dir(dir.join("journal.jsonl")).unwrap();
    let (_, source) = refused_by(journal(&dir).rotate(T));
    assert!(matches!(source, SafeOpenError::NotRegular), "{source:?}");
    assert!(dir.join("journal.jsonl").is_dir());
}

#[test]
fn an_unsafe_lock_file_is_refused_and_nothing_is_rotated() {
    let elsewhere = TempDir::private("rotate-lock-target");
    plant(&elsewhere, "kept", b"x");

    let dir = with_active("rotate-lock-symlink", &["a"]);
    symlink(elsewhere.join("kept"), dir.join("journal.maint")).unwrap();
    let (path, source) = refused_by(journal(&dir).rotate(T));
    assert_eq!(path, dir.join("journal.maint"));
    assert!(matches!(source, SafeOpenError::Symlink), "{source:?}");
    assert!(dir.join("journal.jsonl").exists());

    let dir = with_active("rotate-lock-loose", &["a"]);
    plant(&dir, "journal.maint", b"");
    fs::set_permissions(dir.join("journal.maint"), fs::Permissions::from_mode(0o666)).unwrap();
    let (_, source) = refused_by(journal(&dir).rotate(T));
    assert!(matches!(source, SafeOpenError::LooseMode { .. }), "{source:?}");
    assert!(dir.join("journal.jsonl").exists());

    let dir = with_active("rotate-lock-hardlink", &["a"]);
    plant(&dir, "journal.maint", b"");
    fs::hard_link(dir.join("journal.maint"), dir.join("alias")).unwrap();
    let (_, source) = refused_by(journal(&dir).rotate(T));
    assert!(matches!(source, SafeOpenError::HardLinked { .. }), "{source:?}");
    assert!(dir.join("journal.jsonl").exists());

    assert_eq!(fs::read(elsewhere.join("kept")).unwrap(), b"x");
}

#[test]
fn a_symlinked_or_loose_data_directory_is_refused() {
    let dir = TempDir::private("rotate-dir");
    let real = dir.join("real");
    fs::DirBuilder::new().create(&real).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&real, dir.join("link")).unwrap();

    let (path, source) = refused_by(journal(&dir.join("link")).rotate(T));
    assert_eq!(path, dir.join("link"));
    assert!(matches!(source, SafeOpenError::Symlink), "{source:?}");

    fs::set_permissions(&real, fs::Permissions::from_mode(0o755)).unwrap();
    let (_, source) = refused_by(journal(&real).rotate(T));
    assert!(
        matches!(source, SafeOpenError::LooseMode { mode: 0o755, .. }),
        "{source:?}"
    );
    assert!(names_in(&real).is_empty());
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn the_lock_is_free_again_after_a_refusal() {
    let dir = with_active("rotate-refusal-release", &["a"]);
    fs::set_permissions(dir.join("journal.jsonl"), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(journal(&dir).rotate(T).is_err());
    assert!(lock_is_free(&dir));
}

#[test]
fn a_rotation_holding_the_lock_makes_a_second_one_busy_while_appends_and_reads_go_on() {
    let dir = with_active("rotate-probe-locked", &["seed"]);
    let gate = MaintenanceGate::at(MaintenancePoint::Locked);

    let first = thread::scope(|scope| {
        let first = scope.spawn(|| journal(&dir).with_probe(&gate).rotate(T));
        gate.wait_reached();

        let second = journal(&dir).rotate(T + 1);
        assert!(matches!(second, Err(MaintenanceError::Busy)), "{second:?}");
        journal(&dir).append(&numbered("during", 9)).unwrap();
        let report = journal(&dir).read().unwrap();
        assert_eq!(sessions(&report.records), ["seed", "during"]);

        gate.release();
        first.join().unwrap()
    });

    assert_eq!(first.unwrap(), Rotation::Rotated { stamp: T });
    assert_eq!(
        sessions(&journal(&dir).read().unwrap().records),
        ["seed", "during"]
    );
}

#[test]
fn a_rotation_paused_after_the_rename_leaves_every_record_readable_once() {
    let dir = with_active("rotate-probe-rotated", &["a", "b"]);
    let gate = MaintenanceGate::at(MaintenancePoint::Rotated);

    thread::scope(|scope| {
        let rotating = scope.spawn(|| journal(&dir).with_probe(&gate).rotate(T));
        gate.wait_reached();

        assert!(!dir.join("journal.jsonl").exists());
        assert!(dir.join(generation_name(T)).exists());
        let report = journal(&dir).read().unwrap();
        assert_eq!(sessions(&report.records), ["a", "b"]);
        assert_eq!(report.duplicates_removed, 0);

        gate.release();
        rotating.join().unwrap().unwrap();
    });
}

#[test]
fn a_rotation_reports_each_point_in_order() {
    let dir = with_active("rotate-points", &["a"]);
    let recorded = Recorded::new();
    journal(&dir).with_probe(&recorded).rotate(T).unwrap();
    assert_eq!(
        recorded.points(),
        [MaintenancePoint::Locked, MaintenancePoint::Rotated]
    );

    let quiet = TempDir::private("rotate-points-empty");
    let recorded = Recorded::new();
    journal(&quiet).with_probe(&recorded).rotate(T).unwrap();
    assert_eq!(recorded.points(), [MaintenancePoint::Locked]);
}

#[cfg(target_os = "macos")]
#[test]
fn the_free_function_rotates_on_the_real_volume() {
    let dir = TempDir::private("rotate-system-volume");
    plant(&dir, "journal.jsonl", &frame(&numbered("a", 1)));
    assert_eq!(
        agentdust_core::journal::rotate(&dir, T).unwrap(),
        Rotation::Rotated { stamp: T }
    );
    assert!(dir.join(generation_name(T)).exists());
}
