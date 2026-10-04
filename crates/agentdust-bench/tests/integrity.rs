use std::collections::BTreeSet;

use agentdust_bench::ReadOutcome;
use agentdust_bench::integrity::{Integrity, check};
use agentdust_bench::payload::{marker, record_with};
use agentdust_core::journal::Record;

const SIZE: usize = 150;

fn at(writer: u32, seq: u64, mono_ts: u64) -> Record {
    record_with(writer, seq, SIZE, 1_800_000_000_000, mono_ts)
}

fn report(records: Vec<Record>, skipped_lines: usize) -> ReadOutcome {
    ReadOutcome {
        records,
        malformed_lines: skipped_lines,
        ..ReadOutcome::default()
    }
}

fn acked(pairs: &[(u32, u64)]) -> BTreeSet<(u32, u64)> {
    pairs.iter().copied().collect()
}

#[test]
fn a_clean_run_has_no_findings() {
    let records = vec![at(0, 0, 1), at(1, 0, 2), at(0, 1, 3)];
    let found = check(&report(records, 0), &acked(&[(0, 0), (0, 1), (1, 0)]), SIZE);
    assert_eq!(found, Integrity::default());
}

#[test]
fn an_acknowledged_record_that_is_missing_is_lost() {
    let found = check(&report(vec![at(0, 0, 1)], 0), &acked(&[(0, 0), (1, 1)]), SIZE);
    assert_eq!(found.lost, 1);
    assert_eq!(found.unacked, 0);
}

#[test]
fn a_repeated_record_counts_once_per_extra_copy() {
    let records = vec![at(0, 0, 1), at(0, 0, 1), at(0, 0, 1)];
    let found = check(&report(records, 0), &acked(&[(0, 0)]), SIZE);
    assert_eq!(found.duplicates, 2);
    assert_eq!(found.lost, 0);
}

#[test]
fn lines_that_did_not_parse_are_torn() {
    let found = check(&report(vec![at(0, 0, 1)], 3), &acked(&[(0, 0)]), SIZE);
    assert_eq!(found.torn, 3);
    assert_eq!(found.lost, 0);
}

#[test]
fn a_record_that_parses_with_foreign_content_is_interleaved_and_not_credited() {
    let mut mixed = at(0, 0, 1);
    mixed.session_id.push('!');
    let found = check(&report(vec![mixed], 0), &acked(&[(0, 0)]), SIZE);
    assert_eq!(found.interleaved, 1);
    assert_eq!(found.lost, 1);
}

#[test]
fn a_record_that_was_not_acknowledged_but_is_present_is_unacked() {
    let found = check(
        &report(vec![at(0, 0, 1), at(0, 1, 2)], 0),
        &acked(&[(0, 0)]),
        SIZE,
    );
    assert_eq!(found.unacked, 1);
    assert_eq!(found.lost, 0);
}

#[test]
fn a_writer_whose_sequence_goes_backwards_is_out_of_order() {
    let records = vec![at(0, 1, 1), at(1, 0, 2), at(0, 0, 3)];
    let found = check(&report(records, 0), &acked(&[(0, 0), (0, 1), (1, 0)]), SIZE);
    assert_eq!(found.out_of_order, 1);
}

#[test]
fn different_writers_may_interleave_in_any_order() {
    let records = vec![at(1, 0, 1), at(0, 0, 2), at(1, 1, 3), at(0, 1, 4)];
    let found = check(
        &report(records, 0),
        &acked(&[(0, 0), (0, 1), (1, 0), (1, 1)]),
        SIZE,
    );
    assert_eq!(found.out_of_order, 0);
}

#[test]
fn a_monotonic_stamp_that_goes_backwards_is_an_inversion() {
    let records = vec![at(0, 0, 10), at(1, 0, 5), at(0, 1, 20)];
    let found = check(&report(records, 0), &acked(&[(0, 0), (0, 1), (1, 0)]), SIZE);
    assert_eq!(found.mono_inversions, 1);
    assert_eq!(found.out_of_order, 0);
}

#[test]
fn a_torn_frame_is_torn_too() {
    let tail = ReadOutcome {
        records: vec![at(0, 0, 1)],
        torn_frames: 1,
        ..ReadOutcome::default()
    };
    let found = check(&tail, &acked(&[(0, 0)]), SIZE);
    assert_eq!(found.torn, 1);
    assert_eq!(found.lost, 0);
}

#[test]
fn lines_of_a_newer_version_or_an_unknown_kind_are_torn_too() {
    let odd = ReadOutcome {
        newer_version_lines: 2,
        unknown_kind_lines: 3,
        ..ReadOutcome::default()
    };
    assert_eq!(check(&odd, &acked(&[]), SIZE).torn, 5);
}

#[test]
fn skipped_lines_adds_every_kind_of_line_the_reader_did_not_return() {
    let all = ReadOutcome {
        records: vec![at(0, 0, 1)],
        malformed_lines: 2,
        torn_frames: 5,
        newer_version_lines: 3,
        unknown_kind_lines: 4,
        duplicates_removed: 0,
    };
    assert_eq!(all.skipped_lines(), 14);
    assert_eq!(ReadOutcome::default().skipped_lines(), 0);
}

#[test]
fn duplicates_the_reader_collapsed_still_count_and_are_not_torn() {
    let collapsed = ReadOutcome {
        records: vec![at(0, 0, 1)],
        duplicates_removed: 2,
        ..ReadOutcome::default()
    };
    let found = check(&collapsed, &acked(&[(0, 0)]), SIZE);
    assert_eq!(found.duplicates, 2);
    assert_eq!(found.torn, 0);
    assert_eq!(collapsed.skipped_lines(), 0);
}

#[test]
fn a_marker_that_survived_the_final_compaction_is_counted_apart_and_is_not_interleaved() {
    let records = vec![at(0, 0, 1), marker(3, 1_800_000_000_000, 2)];
    let found = check(&report(records, 0), &acked(&[(0, 0)]), SIZE);
    assert_eq!((found.markers, found.interleaved, found.lost), (1, 0, 0));
}
