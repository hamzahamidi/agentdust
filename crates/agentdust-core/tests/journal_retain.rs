mod journal_support;
mod maintenance_support;
mod scratch;

use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use agentdust_core::journal::volume::{FixedVolume, classify};
use agentdust_core::journal::{Journal, MaintenanceError, PruneReport, RetainReport, Rotation};
use journal_support::{
    copies_on_disk, count_of, frame, frames, generation_name, join, journal, names_in, plant, sessions,
};
use maintenance_support::{
    BOOT, T, corrupt_copies, expired, hold_exclusively, inode_of, keep_everything, lock_is_free, mode_of,
    numbered, on_boot, plant_generation, refused_by, snapshot, without_lock,
};
use scratch::{TempDir, returns_promptly};

fn retain(dir: &Path) -> RetainReport {
    journal(dir).retain(&keep_everything(), T, BOOT).unwrap()
}

fn prune(dir: &Path) -> PruneReport {
    journal(dir).prune(&keep_everything(), T, BOOT).unwrap()
}

fn generation(name: &str, stamp: u64, records: &[agentdust_core::journal::Record]) -> TempDir {
    let dir = TempDir::private(name);
    plant_generation(&dir, stamp, records);
    dir
}

#[test]
fn a_missing_data_directory_is_left_missing() {
    let dir = TempDir::absent("retain-missing");
    assert_eq!(retain(&dir), RetainReport::default());
    let pruned = prune(&dir);
    assert_eq!(pruned.rotation, Rotation::Empty);
    assert_eq!(pruned.retained, RetainReport::default());
    assert!(!dir.exists());
}

#[test]
fn an_empty_data_directory_gets_only_the_private_maintenance_lock() {
    let dir = TempDir::private("retain-empty");
    assert_eq!(retain(&dir), RetainReport::default());
    assert_eq!(names_in(&dir), ["journal.maint"]);
    assert_eq!(mode_of(&dir.join("journal.maint")), 0o600);
}

#[test]
fn an_empty_boot_is_refused_before_anything_is_touched() {
    let dir = generation("retain-empty-boot", 100, &[numbered("a", 1)]);
    plant(&dir, "journal.jsonl", &frame(&numbered("b", 2)));
    let before = snapshot(&dir);

    let retained = journal(&dir).retain(&keep_everything(), T, "");
    let pruned = journal(&dir).prune(&keep_everything(), T, "");

    assert!(
        matches!(retained, Err(MaintenanceError::EmptyBoot)),
        "{retained:?}"
    );
    assert!(matches!(pruned, Err(MaintenanceError::EmptyBoot)), "{pruned:?}");
    assert_eq!(snapshot(&dir), before);
}

#[test]
fn a_generation_with_nothing_to_drop_is_left_as_it_is() {
    let dir = generation("retain-clean", 100, &[numbered("a", 1), numbered("b", 2)]);
    let path = dir.join(generation_name(100));
    let (bytes, inode) = (fs::read(&path).unwrap(), inode_of(&path));

    let report = retain(&dir);

    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(inode_of(&path), inode);
    assert_eq!((report.generations, report.untouched), (1, 1));
    assert_eq!((report.rewritten, report.deleted), (0, 0));
    assert_eq!(report.kept_records, 2);
}

#[test]
fn generations_stay_separate_files_each_replaced_under_its_own_name() {
    let dir = TempDir::private("retain-separate");
    for (stamp, keeper) in [(100, "a"), (200, "b"), (300, "c")] {
        plant_generation(
            &dir,
            stamp,
            &[expired(&format!("gone-{keeper}"), 1), numbered(keeper, 2)],
        );
    }
    let inodes: Vec<u64> = [100, 200, 300]
        .iter()
        .map(|stamp| inode_of(&dir.join(generation_name(*stamp))))
        .collect();

    let report = retain(&dir);

    assert_eq!(
        names_in(&dir),
        [
            generation_name(100),
            generation_name(200),
            generation_name(300),
            "journal.maint".to_owned()
        ]
    );
    for (n, (stamp, keeper)) in [(100, "a"), (200, "b"), (300, "c")].into_iter().enumerate() {
        let path = dir.join(generation_name(stamp));
        assert_eq!(fs::read(&path).unwrap(), frame(&numbered(keeper, 2)), "{stamp}");
        assert_ne!(inode_of(&path), inodes[n], "{stamp}");
        assert_eq!(mode_of(&path), 0o600, "{stamp}");
    }
    assert_eq!((report.generations, report.rewritten, report.deleted), (3, 3, 0));
    assert_eq!(report.dropped_earlier_boot, 3);
    assert_eq!(report.kept_records, 3);
    assert_eq!(sessions(&journal(&dir).read().unwrap().records), ["a", "b", "c"]);
}

#[test]
fn a_generation_that_keeps_nothing_is_deleted_and_nothing_is_written() {
    let dir = TempDir::private("retain-delete");
    plant_generation(&dir, 100, &[expired("a", 1)]);
    plant_generation(&dir, 200, &[expired("b", 2), expired("c", 3)]);

    let report = retain(&dir);

    assert_eq!(names_in(&dir), ["journal.maint"]);
    assert_eq!((report.generations, report.deleted, report.rewritten), (2, 2, 0));
    assert_eq!(report.kept_records, 0);
}

#[test]
fn a_kept_line_keeps_its_bytes_and_its_unknown_fields_and_gains_the_frame() {
    let dir = TempDir::private("retain-bytes");
    let spaced = b"{\"v\":1,\"kind\":\"shell_start\",\"agent\":\"claude\",\"session_id\":\"x\\u0041\",\"wall_ts\":5,\"mono_ts\":6,\"boot\":\"boot\",\"later_field\":[1, 2,  {\"k\":null}]}";
    let bare = [spaced.as_slice(), b"\n"].concat();
    plant(
        &dir,
        &generation_name(100),
        &join(&[&bare, &frame(&expired("gone", 1))]),
    );

    let report = retain(&dir);

    let expected = join(&[&[0x1e], spaced, b"\n"]);
    assert_eq!(fs::read(dir.join(generation_name(100))).unwrap(), expected);
    assert_eq!(report.rewritten, 1);
    assert_eq!(sessions(&journal(&dir).read().unwrap().records), ["xA"]);
}

#[test]
fn a_replaced_generation_is_a_new_file_and_the_old_one_is_never_rewritten_in_place() {
    let dir = generation("retain-new-file", 100, &[expired("gone", 1), numbered("kept", 2)]);
    let path = dir.join(generation_name(100));
    let (original, inode) = (fs::read(&path).unwrap(), inode_of(&path));
    let mut reader = File::open(&path).unwrap();

    retain(&dir);

    let mut seen = Vec::new();
    reader.read_to_end(&mut seen).unwrap();
    assert_eq!(seen, original);
    assert_ne!(inode_of(&path), inode);
}

#[test]
fn a_second_run_changes_nothing() {
    let dir = TempDir::private("retain-idempotent");
    plant_generation(&dir, 100, &[expired("gone", 1), numbered("a", 2)]);
    plant_generation(&dir, 200, &[numbered("b", 3)]);
    plant(&dir, "journal.jsonl", &frame(&numbered("c", 4)));

    prune(&dir);
    let first = snapshot(&dir);
    let again = prune(&dir);

    assert_eq!(snapshot(&dir), first);
    assert_eq!(again.rotation, Rotation::Empty);
    assert_eq!(
        (
            again.retained.rewritten,
            again.retained.deleted,
            again.retained.untouched
        ),
        (0, 0, again.retained.generations)
    );
}

#[test]
fn a_run_that_drops_nothing_does_not_change_what_a_read_returns() {
    let dir = TempDir::private("retain-same-read");
    plant_generation(&dir, 100, &[numbered("a", 1), numbered("b", 5)]);
    plant_generation(&dir, 200, &[numbered("c", 3)]);
    plant(&dir, "journal.jsonl", &frame(&numbered("d", 4)));
    let before = journal(&dir).read().unwrap();
    prune(&dir);
    assert_eq!(journal(&dir).read().unwrap(), before);
}

#[test]
fn an_exact_duplicate_of_a_record_in_an_older_generation_is_dropped_from_the_newer_one() {
    let dir = TempDir::private("retain-duplicates");
    plant_generation(&dir, 100, &[numbered("a", 1), numbered("b", 2)]);
    plant_generation(&dir, 200, &[numbered("b", 2), numbered("c", 3)]);
    plant_generation(&dir, 300, &[numbered("a", 1)]);
    let older = dir.join(generation_name(100));
    let (bytes, inode) = (fs::read(&older).unwrap(), inode_of(&older));
    assert_eq!(journal(&dir).read().unwrap().duplicates_removed, 2);

    let report = retain(&dir);

    assert_eq!(fs::read(&older).unwrap(), bytes);
    assert_eq!(inode_of(&older), inode);
    assert_eq!(
        fs::read(dir.join(generation_name(200))).unwrap(),
        frame(&numbered("c", 3))
    );
    assert!(!dir.join(generation_name(300)).exists());
    assert_eq!(report.duplicates_removed, 2);
    assert_eq!((report.untouched, report.rewritten, report.deleted), (1, 1, 1));
    assert_eq!(report.kept_records, 3);
    let read = journal(&dir).read().unwrap();
    assert_eq!(sessions(&read.records), ["a", "b", "c"]);
    assert_eq!(read.duplicates_removed, 0);
    for session in ["a", "b", "c"] {
        assert_eq!(copies_on_disk(&dir, session), 1, "{session}");
    }
}

#[test]
fn a_duplicate_of_a_record_in_the_active_file_is_left_for_the_reader_to_collapse() {
    let dir = generation("retain-duplicate-active", 100, &[numbered("a", 1)]);
    plant(&dir, "journal.jsonl", &frame(&numbered("a", 1)));
    let before = snapshot(&dir);

    let report = retain(&dir);

    assert_eq!(without_lock(snapshot(&dir)), without_lock(before));
    assert_eq!(report.duplicates_removed, 0);
    assert_eq!(journal(&dir).read().unwrap().duplicates_removed, 1);
}

#[test]
fn retention_never_visits_the_active_file_and_no_record_is_moved_into_it() {
    let dir = TempDir::private("retain-active");
    plant_generation(&dir, 100, &[expired("gone", 1), numbered("kept", 2)]);
    plant(
        &dir,
        "journal.jsonl",
        &frames(&[expired("active-old", 3), numbered("active", 4)]),
    );
    let active = dir.join("journal.jsonl");
    let (bytes, inode) = (fs::read(&active).unwrap(), inode_of(&active));

    let report = retain(&dir);

    assert_eq!(fs::read(&active).unwrap(), bytes);
    assert_eq!(inode_of(&active), inode);
    assert_eq!(report.dropped_earlier_boot, 1);
    assert_eq!(copies_on_disk(&dir, "kept"), 1);
    assert_eq!(copies_on_disk(&dir, "active-old"), 1);
    assert_eq!(report.kept_records, 3);
}

#[test]
fn retention_never_creates_the_active_file() {
    let dir = generation(
        "retain-no-active",
        100,
        &[expired("gone", 1), numbered("kept", 2)],
    );
    retain(&dir);
    assert!(!dir.join("journal.jsonl").exists());
    assert_eq!(names_in(&dir), [generation_name(100), "journal.maint".to_owned()]);
}

#[test]
fn a_generation_sealed_in_the_millisecond_of_the_run_is_processed_at_once() {
    let dir = generation("retain-same-ms", T, &[expired("gone", 1), numbered("kept", 2)]);
    let report = retain(&dir);
    assert_eq!(report.rewritten, 1);
    assert_eq!(
        fs::read(dir.join(generation_name(T))).unwrap(),
        frame(&numbered("kept", 2))
    );
}

#[test]
fn a_generation_stamped_in_the_future_is_processed_like_any_other() {
    let dir = generation(
        "retain-future",
        T + 5_000_000,
        &[expired("gone", 1), numbered("kept", 2)],
    );
    let report = retain(&dir);
    assert_eq!(report.rewritten, 1);
}

#[test]
fn prune_rotates_the_active_file_and_retains_the_sealed_generation_in_the_same_run() {
    let dir = TempDir::private("prune-rotates");
    plant(
        &dir,
        "journal.jsonl",
        &frames(&[expired("gone", 1), numbered("kept", 2)]),
    );

    let report = prune(&dir);

    assert_eq!(report.rotation, Rotation::Rotated { stamp: T });
    assert_eq!((report.retained.generations, report.retained.rewritten), (1, 1));
    assert_eq!(names_in(&dir), [generation_name(T), "journal.maint".to_owned()]);
    assert_eq!(
        fs::read(dir.join(generation_name(T))).unwrap(),
        frame(&numbered("kept", 2))
    );
}

#[test]
fn prune_counts_an_active_file_created_after_the_rotation_but_never_changes_it() {
    let dir = TempDir::private("prune-new-active");
    plant(&dir, "journal.jsonl", &frame(&numbered("before", 1)));
    journal(&dir).prune(&keep_everything(), T, BOOT).unwrap();
    journal(&dir).append(&numbered("after", 2)).unwrap();
    let active = dir.join("journal.jsonl");
    let bytes = fs::read(&active).unwrap();

    let report = retain(&dir);

    assert_eq!(fs::read(&active).unwrap(), bytes);
    assert_eq!(report.kept_records, 2);
}

#[test]
fn a_lock_held_elsewhere_makes_retention_and_prune_busy_and_changes_nothing() {
    let dir = generation("retain-busy", 100, &[expired("gone", 1)]);
    plant(&dir, "journal.jsonl", &frame(&numbered("a", 2)));
    let held = hold_exclusively(&dir);
    let before = snapshot(&dir);

    let retained = journal(&dir).retain(&keep_everything(), T, BOOT);
    let pruned = journal(&dir).prune(&keep_everything(), T, BOOT);

    assert!(matches!(retained, Err(MaintenanceError::Busy)), "{retained:?}");
    assert!(matches!(pruned, Err(MaintenanceError::Busy)), "{pruned:?}");
    assert_eq!(snapshot(&dir), before);
    drop(held);
    retain(&dir);
}

#[test]
fn the_lock_is_free_again_when_retention_and_prune_return() {
    let dir = generation("retain-release", 100, &[expired("gone", 1)]);
    retain(&dir);
    assert!(lock_is_free(&dir));
    prune(&dir);
    assert!(lock_is_free(&dir));
}

#[test]
fn a_volume_that_is_not_local_apfs_is_refused_by_retention_and_prune() {
    for (name, flags) in [("nfs", 0), ("smbfs", 0), ("apfs", 0), ("hfs", 0x1000)] {
        let dir = generation("retain-volume", 100, &[expired("gone", 1)]);
        plant(&dir, "journal.jsonl", &frame(&numbered("a", 2)));
        let before = snapshot(&dir);
        let volume = FixedVolume::new(classify(name, flags));
        let journal = Journal::with_volume(&dir, &volume);

        let retained = journal.retain(&keep_everything(), T, BOOT);
        let pruned = journal.prune(&keep_everything(), T, BOOT);

        for result in [retained.map(drop), pruned.map(drop)] {
            match result {
                Err(MaintenanceError::UnsupportedFilesystem(facts)) => assert_eq!(facts.name, name),
                other => panic!("{name}: {other:?}"),
            }
        }
        assert_eq!(snapshot(&dir), before, "{name}");
    }
}

#[test]
fn a_symlinked_generation_is_refused_and_prune_rotates_nothing() {
    let dir = TempDir::private("retain-refuse-symlink");
    let elsewhere = TempDir::private("retain-refuse-symlink-target");
    plant(&elsewhere, "kept", &frame(&numbered("x", 1)));
    std::os::unix::fs::symlink(elsewhere.join("kept"), dir.join(generation_name(100))).unwrap();
    plant_generation(&dir, 200, &[expired("b", 2)]);
    plant(&dir, "journal.jsonl", &frame(&numbered("a", 3)));
    let before = without_lock(snapshot(&dir));

    let retained = refused_by(journal(&dir).retain(&keep_everything(), T, BOOT));
    let pruned = refused_by(journal(&dir).prune(&keep_everything(), T, BOOT));

    for (path, source) in [retained, pruned] {
        assert_eq!(path, dir.join(generation_name(100)));
        assert!(
            matches!(source, agentdust_core::safe_open::SafeOpenError::Symlink),
            "{source:?}"
        );
    }
    assert_eq!(without_lock(snapshot(&dir)), before);
    assert_eq!(
        fs::read(elsewhere.join("kept")).unwrap(),
        frame(&numbered("x", 1))
    );
}

#[test]
fn a_run_with_a_generation_that_cannot_be_read_deletes_and_rewrites_nothing() {
    let dir = TempDir::private("retain-unreadable");
    plant_generation(&dir, 100, &[expired("a", 1)]);
    plant_generation(&dir, 200, &[expired("b", 2)]);
    fs::set_permissions(dir.join(generation_name(100)), fs::Permissions::from_mode(0o000)).unwrap();
    let probe = File::open(dir.join(generation_name(100)));
    if probe.is_ok() {
        fs::set_permissions(dir.join(generation_name(100)), fs::Permissions::from_mode(0o600)).unwrap();
        return;
    }

    let result = journal(&dir).retain(&keep_everything(), T, BOOT);

    assert!(matches!(result, Err(MaintenanceError::Io(_))), "{result:?}");
    assert_eq!(
        names_in(&dir),
        [
            generation_name(100),
            generation_name(200),
            "journal.maint".to_owned()
        ]
    );
    assert_eq!(
        fs::read(dir.join(generation_name(200))).unwrap(),
        frame(&expired("b", 2))
    );
    fs::set_permissions(dir.join(generation_name(100)), fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn a_generation_with_a_torn_frame_and_nothing_to_drop_is_left_as_it_is() {
    let dir = TempDir::private("retain-torn-only");
    let bytes = join(&[&frame(&numbered("a", 1)), b"\x1e{\"v\":1,\"kind\""]);
    plant(&dir, &generation_name(100), &bytes);
    let path = dir.join(generation_name(100));
    let inode = inode_of(&path);

    let report = retain(&dir);

    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(inode_of(&path), inode);
    assert_eq!(
        (report.torn_frames, report.untouched, report.rewritten),
        (1, 1, 0)
    );
    assert!(corrupt_copies(&dir).is_empty());
}

#[test]
fn a_generation_of_nothing_but_garbage_is_neither_rewritten_nor_deleted() {
    let dir = TempDir::private("retain-garbage-only");
    let bytes = b"\xff\xfe this is not json\nnor is this\n".to_vec();
    plant(&dir, &generation_name(100), &bytes);

    let report = retain(&dir);

    assert_eq!(fs::read(dir.join(generation_name(100))).unwrap(), bytes);
    assert_eq!(
        (report.malformed_lines, report.untouched, report.deleted),
        (2, 1, 0)
    );
    assert!(corrupt_copies(&dir).is_empty());
}

#[test]
fn an_empty_generation_is_left_as_it_is() {
    let dir = TempDir::private("retain-empty-generation");
    plant(&dir, &generation_name(100), b"");
    let report = retain(&dir);
    assert_eq!((report.generations, report.untouched, report.deleted), (1, 1, 0));
    assert!(dir.join(generation_name(100)).exists());
}

#[test]
fn an_unknown_kind_line_is_kept_as_it_was_when_the_generation_is_rewritten() {
    let dir = TempDir::private("retain-unknown-kind");
    let mut mystery = serde_json::to_value(numbered("m", 2)).unwrap();
    mystery["kind"] = serde_json::json!("mystery");
    let mystery = serde_json::to_vec(&mystery).unwrap();
    plant(
        &dir,
        &generation_name(100),
        &join(&[&frame(&expired("gone", 1)), &[0x1e], &mystery, b"\n"]),
    );

    let report = retain(&dir);

    assert_eq!(
        fs::read(dir.join(generation_name(100))).unwrap(),
        join(&[&[0x1e], &mystery, b"\n"])
    );
    assert_eq!(
        (report.unknown_kind_lines, report.rewritten, report.deleted),
        (1, 1, 0)
    );
    assert!(corrupt_copies(&dir).is_empty());
}

#[test]
fn a_generation_with_only_an_unknown_kind_line_and_dropped_records_is_not_deleted() {
    let dir = TempDir::private("retain-unknown-kept");
    let mut mystery = serde_json::to_value(numbered("m", 2)).unwrap();
    mystery["kind"] = serde_json::json!("mystery");
    let mystery = serde_json::to_vec(&mystery).unwrap();
    plant(
        &dir,
        &generation_name(100),
        &join(&[&frame(&expired("gone", 1)), &[0x1e], &mystery, b"\n"]),
    );
    retain(&dir);
    assert!(dir.join(generation_name(100)).exists());
}

#[test]
fn records_of_a_session_that_has_not_ended_survive_the_default_policy() {
    let dir = TempDir::private("retain-defaults");
    let mut start = numbered("live", 1);
    start.kind = agentdust_core::journal::Kind::SessionStart;
    plant_generation(&dir, 100, &[start, numbered("live", 2)]);
    let report = journal(&dir)
        .retain(&agentdust_core::journal::retention::Policy::default(), T, BOOT)
        .unwrap();
    assert_eq!(report.kept_records, 2);
    assert_eq!(report.dropped_pinned, 0);
    assert!(report.degraded.is_empty());
    assert_eq!(report.untouched, 1);
}

#[test]
fn a_run_that_finds_a_record_of_another_boot_in_a_generation_of_this_boot_drops_it() {
    let dir = generation(
        "retain-boot",
        100,
        &[on_boot(numbered("old", 1), "older"), numbered("now", 2)],
    );
    let report = retain(&dir);
    assert_eq!((report.dropped_earlier_boot, report.kept_records), (1, 1));
    assert_eq!(sessions(&journal(&dir).read().unwrap().records), ["now"]);
}

#[test]
fn counts_of_damaged_lines_cover_every_file_that_was_read() {
    let dir = TempDir::private("retain-counts");
    plant(&dir, &generation_name(100), b"garbage\n");
    plant(&dir, "journal.jsonl", b"also garbage\n\x1e{\"v\":1");

    let report = retain(&dir);

    assert_eq!((report.malformed_lines, report.torn_frames), (2, 1));
    assert_eq!(report.generations, 1);
}

#[test]
fn the_free_functions_use_the_system_volume() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let dir = generation("retain-free", 100, &[expired("gone", 1), numbered("kept", 2)]);
    let report = agentdust_core::journal::retain(&dir, &keep_everything(), T, BOOT).unwrap();
    assert_eq!(report.rewritten, 1);
    let report = agentdust_core::journal::prune(&dir, &keep_everything(), T, BOOT).unwrap();
    assert_eq!(report.rotation, Rotation::Empty);
}

#[test]
fn a_fifo_named_like_a_generation_is_refused_without_blocking() {
    let dir = TempDir::private("retain-fifo");
    scratch::make_fifo(&dir.join(generation_name(100)));
    plant(&dir, "journal.jsonl", &frame(&numbered("a", 1)));
    let target = dir.to_path_buf();
    let result = returns_promptly(move || journal(&target).prune(&keep_everything(), T, BOOT));
    let (path, source) = refused_by(result);
    assert_eq!(path, dir.join(generation_name(100)));
    assert!(
        matches!(source, agentdust_core::safe_open::SafeOpenError::NotRegular),
        "{source:?}"
    );
    assert!(dir.join("journal.jsonl").exists());
}

#[test]
fn kept_lines_stay_in_their_original_file_order() {
    let dir = TempDir::private("retain-order");
    plant_generation(
        &dir,
        100,
        &[
            numbered("c", 3),
            expired("gone", 9),
            numbered("a", 1),
            numbered("b", 2),
        ],
    );
    retain(&dir);
    assert_eq!(
        fs::read(dir.join(generation_name(100))).unwrap(),
        frames(&[numbered("c", 3), numbered("a", 1), numbered("b", 2)])
    );
    assert_eq!(count_of(&journal(&dir).read().unwrap().records, "gone"), 0);
}
