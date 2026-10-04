mod journal_support;
mod maintenance_support;
mod scratch;

use std::fs;
use std::os::unix::fs::symlink;

use agentdust_core::journal::{MaintenanceError, SCHEMA_VERSION};
use journal_support::{count_of, frame, frames, generation_name, join, journal, names_in, plant};
use maintenance_support::{
    BOOT, T, corrupt_copies, expired, inode_of, keep_everything, mode_of, numbered, plant_generation, policy,
    snapshot,
};
use scratch::TempDir;

const GARBAGE: &[u8] = b"\xff\xfe this is not json\n";

fn garbage_between(dropped: &str, kept: &str) -> Vec<u8> {
    join(&[&frame(&expired(dropped, 1)), GARBAGE, &frame(&numbered(kept, 2))])
}

fn future_line(padding: usize) -> Vec<u8> {
    let json = format!(
        "{{\"v\":{},\"kind\":\"later\",\"pad\":\"{}\"}}",
        SCHEMA_VERSION + 1,
        "x".repeat(padding)
    );
    join(&[&[0x1e], json.as_bytes(), b"\n"])
}

#[test]
fn a_line_that_does_not_parse_is_copied_aside_before_its_generation_is_rewritten() {
    let dir = TempDir::private("recover-copy");
    let original = garbage_between("gone", "kept");
    plant(&dir, &generation_name(100), &original);

    let report = journal(&dir).retain(&keep_everything(), T, BOOT).unwrap();

    let copy = dir.join(format!("journal.jsonl.corrupt-{T}"));
    assert_eq!(fs::read(&copy).unwrap(), original);
    assert_eq!(mode_of(&copy), 0o600);
    assert_eq!(report.corrupt_copies, [copy]);
    assert_eq!(report.malformed_lines, 1);
    assert_eq!(report.dropped_damaged_lines, 1);
    assert_eq!(
        fs::read(dir.join(generation_name(100))).unwrap(),
        frame(&numbered("kept", 2))
    );
    assert_eq!(journal(&dir).read().unwrap().malformed_lines, 0);
}

#[test]
fn a_generation_of_dropped_records_and_garbage_is_copied_aside_and_then_deleted() {
    let dir = TempDir::private("recover-delete");
    let original = join(&[&frame(&expired("gone", 1)), GARBAGE]);
    plant(&dir, &generation_name(100), &original);

    let report = journal(&dir).retain(&keep_everything(), T, BOOT).unwrap();

    assert!(!dir.join(generation_name(100)).exists());
    assert_eq!(report.deleted, 1);
    let copies = corrupt_copies(&dir);
    assert_eq!(copies.len(), 1);
    assert_eq!(fs::read(dir.join(&copies[0])).unwrap(), original);
}

#[test]
fn a_non_utf8_line_counts_as_malformed_and_is_copied_aside() {
    let dir = TempDir::private("recover-non-utf8");
    plant(
        &dir,
        &generation_name(100),
        &join(&[&frame(&expired("gone", 1)), b"\xc3\x28\n"]),
    );
    let report = journal(&dir).retain(&keep_everything(), T, BOOT).unwrap();
    assert_eq!(report.malformed_lines, 1);
    assert_eq!(corrupt_copies(&dir).len(), 1);
}

#[test]
fn a_torn_frame_is_dropped_with_a_rewrite_and_makes_no_copy() {
    let dir = TempDir::private("recover-torn");
    plant(
        &dir,
        &generation_name(100),
        &join(&[
            &frame(&expired("gone", 1)),
            &frame(&numbered("kept", 2)),
            b"\x1e{\"v\":1",
        ]),
    );

    let report = journal(&dir).retain(&keep_everything(), T, BOOT).unwrap();

    assert_eq!(
        fs::read(dir.join(generation_name(100))).unwrap(),
        frame(&numbered("kept", 2))
    );
    assert_eq!((report.torn_frames, report.dropped_damaged_lines), (1, 1));
    assert!(corrupt_copies(&dir).is_empty());
    assert!(report.corrupt_copies.is_empty());
}

#[test]
fn a_clean_generation_makes_no_copy_even_when_it_is_rewritten() {
    let dir = TempDir::private("recover-clean");
    plant_generation(&dir, 100, &[expired("gone", 1), numbered("kept", 2)]);
    journal(&dir).retain(&keep_everything(), T, BOOT).unwrap();
    assert!(corrupt_copies(&dir).is_empty());
}

#[test]
fn a_garbage_line_with_nothing_to_drop_makes_no_copy_and_changes_nothing() {
    let dir = TempDir::private("recover-no-rewrite");
    let original = join(&[&frame(&numbered("a", 1)), GARBAGE]);
    plant(&dir, &generation_name(100), &original);
    let inode = inode_of(&dir.join(generation_name(100)));

    journal(&dir).retain(&keep_everything(), T, BOOT).unwrap();

    assert_eq!(fs::read(dir.join(generation_name(100))).unwrap(), original);
    assert_eq!(inode_of(&dir.join(generation_name(100))), inode);
    assert!(corrupt_copies(&dir).is_empty());
}

#[test]
fn two_damaged_generations_get_two_copies_with_the_next_free_names() {
    let dir = TempDir::private("recover-two");
    let first = garbage_between("gone-a", "a");
    let second = garbage_between("gone-b", "b");
    plant(&dir, &generation_name(100), &first);
    plant(&dir, &generation_name(200), &second);

    let report = journal(&dir).retain(&keep_everything(), T, BOOT).unwrap();

    assert_eq!(report.corrupt_copies.len(), 2);
    assert_eq!(
        fs::read(dir.join(format!("journal.jsonl.corrupt-{T}"))).unwrap(),
        first
    );
    assert_eq!(
        fs::read(dir.join(format!("journal.jsonl.corrupt-{}", T + 1))).unwrap(),
        second
    );
}

#[test]
fn a_copy_never_replaces_a_file_that_has_its_name() {
    let dir = TempDir::private("recover-name-taken");
    plant(&dir, &format!("journal.jsonl.corrupt-{T}"), b"an earlier copy");
    let original = garbage_between("gone", "kept");
    plant(&dir, &generation_name(100), &original);

    journal(&dir).retain(&keep_everything(), T, BOOT).unwrap();

    assert_eq!(
        fs::read(dir.join(format!("journal.jsonl.corrupt-{T}"))).unwrap(),
        b"an earlier copy"
    );
    assert_eq!(
        fs::read(dir.join(format!("journal.jsonl.corrupt-{}", T + 1))).unwrap(),
        original
    );
}

#[test]
fn a_failed_rewrite_leaves_the_generation_and_its_copy_in_place() {
    let dir = TempDir::private("recover-failed-rewrite");
    let original = garbage_between("gone", "kept");
    plant(&dir, &generation_name(100), &original);
    fs::create_dir(dir.join("journal.compact.tmp")).unwrap();

    let result = journal(&dir).retain(&keep_everything(), T, BOOT);

    assert!(matches!(result, Err(MaintenanceError::Io(_))), "{result:?}");
    assert_eq!(fs::read(dir.join(generation_name(100))).unwrap(), original);
    assert_eq!(
        fs::read(dir.join(format!("journal.jsonl.corrupt-{T}"))).unwrap(),
        original
    );
}

#[test]
fn a_compaction_file_left_by_a_crash_is_removed_and_never_read() {
    let dir = TempDir::private("recover-stale-tmp");
    plant(&dir, "journal.compact.tmp", b"half a replacement");
    plant_generation(&dir, 100, &[numbered("a", 1)]);
    assert_eq!(journal(&dir).read().unwrap().records.len(), 1);

    journal(&dir).retain(&keep_everything(), T, BOOT).unwrap();

    assert_eq!(names_in(&dir), [generation_name(100), "journal.maint".to_owned()]);
}

#[test]
fn a_symlink_in_the_place_of_the_compaction_file_is_removed_and_not_followed() {
    let dir = TempDir::private("recover-tmp-symlink");
    let elsewhere = TempDir::private("recover-tmp-target");
    plant(&elsewhere, "precious", b"do not touch");
    symlink(elsewhere.join("precious"), dir.join("journal.compact.tmp")).unwrap();
    plant_generation(&dir, 100, &[expired("gone", 1), numbered("kept", 2)]);

    journal(&dir).retain(&keep_everything(), T, BOOT).unwrap();

    assert_eq!(fs::read(elsewhere.join("precious")).unwrap(), b"do not touch");
    assert_eq!(names_in(&dir), [generation_name(100), "journal.maint".to_owned()]);
}

#[test]
fn a_crash_after_the_copy_and_before_the_rewrite_leaves_a_second_copy_on_the_next_run() {
    let dir = TempDir::private("recover-crash-after-copy");
    let original = garbage_between("gone", "kept");
    plant(&dir, &generation_name(100), &original);
    plant(&dir, &format!("journal.jsonl.corrupt-{T}"), &original);

    journal(&dir).retain(&keep_everything(), T, BOOT).unwrap();

    assert_eq!(corrupt_copies(&dir).len(), 2);
    assert_eq!(
        fs::read(dir.join(generation_name(100))).unwrap(),
        frame(&numbered("kept", 2))
    );
}

#[test]
fn copies_and_the_compaction_file_are_not_journal_files_to_a_reader() {
    let dir = TempDir::private("recover-reader-ignores");
    plant_generation(&dir, 100, &[numbered("a", 1)]);
    plant(&dir, "journal.jsonl.corrupt-5", &frame(&numbered("copy", 2)));
    plant(&dir, "journal.compact.tmp", &frame(&numbered("tmp", 3)));
    let report = journal(&dir).read().unwrap();
    assert_eq!(count_of(&report.records, "a"), 1);
    assert_eq!(
        count_of(&report.records, "copy") + count_of(&report.records, "tmp"),
        0
    );
}

#[test]
fn a_generation_with_a_newer_version_line_is_never_rewritten_even_when_it_holds_dropped_records() {
    let dir = TempDir::private("s18-short");
    let held = join(&[
        &frame(&expired("gone", 1)),
        &future_line(10),
        GARBAGE,
        &frame(&numbered("kept", 2)),
    ]);
    plant(&dir, &generation_name(100), &held);
    plant_generation(&dir, 200, &[expired("b", 3), numbered("c", 4)]);
    plant_generation(&dir, 300, &[expired("d", 5), numbered("e", 6)]);
    let inode = inode_of(&dir.join(generation_name(100)));

    let report = journal(&dir).retain(&keep_everything(), T, BOOT).unwrap();

    assert_eq!(fs::read(dir.join(generation_name(100))).unwrap(), held);
    assert_eq!(inode_of(&dir.join(generation_name(100))), inode);
    assert_eq!(report.held_newer_version, 1);
    assert_eq!(report.newer_version_lines, 1);
    assert_eq!((report.generations, report.rewritten, report.deleted), (3, 2, 0));
    assert_eq!(report.dropped_earlier_boot, 2);
    assert!(corrupt_copies(&dir).is_empty());
    let read = journal(&dir).read().unwrap();
    assert!(read.unsupported_version);
    assert_eq!(count_of(&read.records, "gone"), 1);
}

#[test]
fn a_newer_version_line_over_the_size_cap_still_holds_its_generation_untouched() {
    for padding in [70_000usize, 300_000] {
        let dir = TempDir::private("s18-long");
        let held = join(&[
            &frame(&expired("gone", 1)),
            &future_line(padding),
            &frame(&numbered("kept", 2)),
        ]);
        plant(&dir, &generation_name(100), &held);
        let inode = inode_of(&dir.join(generation_name(100)));

        let report = journal(&dir).retain(&keep_everything(), T, BOOT).unwrap();

        assert_eq!(
            fs::read(dir.join(generation_name(100))).unwrap(),
            held,
            "{padding}"
        );
        assert_eq!(inode_of(&dir.join(generation_name(100))), inode, "{padding}");
        assert_eq!(report.newer_version_lines, 1, "{padding}");
        assert_eq!(report.malformed_lines, 0, "{padding}");
        assert_eq!(report.held_newer_version, 1, "{padding}");
        assert_eq!(report.dropped_earlier_boot, 0, "{padding}");
        assert_eq!(journal(&dir).read().unwrap().newer_version_lines, 1, "{padding}");
    }
}

#[test]
fn a_generation_with_only_a_newer_version_line_is_never_deleted() {
    let dir = TempDir::private("s18-only");
    let held = future_line(100);
    plant(&dir, &generation_name(100), &held);
    let report = journal(&dir).prune(&keep_everything(), T, BOOT).unwrap().retained;
    assert_eq!(fs::read(dir.join(generation_name(100))).unwrap(), held);
    assert_eq!((report.held_newer_version, report.deleted), (1, 0));
}

#[test]
fn an_over_cap_line_of_the_current_version_is_malformed_and_is_dropped_with_a_copy() {
    let dir = TempDir::private("s18-over-cap-current");
    let json = format!(
        "{{\"v\":{SCHEMA_VERSION},\"kind\":\"shell_start\",\"pad\":\"{}\"}}",
        "x".repeat(70_000)
    );
    let over = join(&[&[0x1e], json.as_bytes(), b"\n"]);
    let original = join(&[&frame(&expired("gone", 1)), &over, &frame(&numbered("kept", 2))]);
    plant(&dir, &generation_name(100), &original);

    let report = journal(&dir).retain(&keep_everything(), T, BOOT).unwrap();

    assert_eq!((report.malformed_lines, report.newer_version_lines), (1, 0));
    assert_eq!(
        fs::read(dir.join(generation_name(100))).unwrap(),
        frame(&numbered("kept", 2))
    );
    assert_eq!(corrupt_copies(&dir).len(), 1);
}

#[test]
fn the_records_of_a_held_generation_count_toward_the_limit_and_are_never_dropped_for_it() {
    let dir = TempDir::private("s18-counts");
    let pinned: Vec<_> = (0..6).map(|n| numbered("held", 10 + n)).collect();
    let held = join(&[&frames(&pinned), &future_line(10)]);
    plant(&dir, &generation_name(100), &held);
    let mut start = numbered("ended", 1);
    start.kind = agentdust_core::journal::Kind::SessionStart;
    let mut end = numbered("ended", 2);
    end.kind = agentdust_core::journal::Kind::SessionEnd;
    plant_generation(&dir, 200, &[start, end]);
    let limit = frames(&pinned).len() as u64;

    let report = journal(&dir)
        .retain(&policy(14 * 24 * 3_600_000, limit), T, BOOT)
        .unwrap();

    assert_eq!(fs::read(dir.join(generation_name(100))).unwrap(), held);
    assert!(!dir.join(generation_name(200)).exists());
    assert_eq!((report.dropped_over_cap, report.dropped_pinned), (2, 0));
    assert_eq!(count_of(&journal(&dir).read().unwrap().records, "held"), 6);
}

#[test]
fn nothing_in_the_data_directory_changes_when_every_generation_is_held() {
    let dir = TempDir::private("s18-all-held");
    plant(
        &dir,
        &generation_name(100),
        &join(&[&frame(&expired("a", 1)), &future_line(5)]),
    );
    plant(
        &dir,
        &generation_name(200),
        &join(&[&frame(&expired("b", 2)), &future_line(5)]),
    );
    let before = snapshot(&dir);

    let report = journal(&dir).retain(&keep_everything(), T, BOOT).unwrap();

    assert_eq!(snapshot(&dir), {
        let mut expected = before;
        expected.insert("journal.maint".to_owned(), Vec::new());
        expected
    });
    assert_eq!(
        (report.held_newer_version, report.rewritten, report.deleted),
        (2, 0, 0)
    );
}
