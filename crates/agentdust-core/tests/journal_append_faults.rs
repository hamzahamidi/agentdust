mod journal_support;
mod scratch;

use agentdust_core::journal::{JournalError, ReadReport};
use journal_support::{Scripted, Step, frame, journal, record, sessions};
use scratch::TempDir;

struct Outcome {
    report: ReadReport,
    result: Result<u32, JournalError>,
    calls: Vec<usize>,
}

fn frame_len() -> usize {
    frame(&record("middle", "b", 2, 2)).len()
}

fn survive(name: &str, steps: Vec<Step>) -> Outcome {
    let dir = TempDir::absent(name);
    journal(&dir).append(&record("before", "b", 1, 1)).unwrap();
    let mut writer = Scripted::new(steps);
    let result = journal(&dir)
        .append_with(&record("middle", "b", 2, 2), &mut writer)
        .map(|appended| appended.attempts);
    journal(&dir).append(&record("after", "b", 3, 3)).unwrap();
    Outcome {
        report: journal(&dir).read().unwrap(),
        result,
        calls: writer.calls,
    }
}

fn short(outcome: &Outcome) -> (usize, usize) {
    match &outcome.result {
        Err(JournalError::ShortWrite { written, expected }) => (*written, *expected),
        other => panic!("{other:?}"),
    }
}

fn os_error(outcome: &Outcome, errno: i32) {
    match &outcome.result {
        Err(JournalError::Io(err)) => assert_eq!(err.raw_os_error(), Some(errno)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn eintr_before_any_transfer_is_restarted_and_the_record_is_stored_once() {
    let outcome = survive("fault-eintr", vec![Step::Interrupt, Step::Interrupt]);
    assert_eq!(outcome.result.unwrap(), 1);
    assert_eq!(outcome.calls, [frame_len(); 3]);
    assert_eq!(sessions(&outcome.report.records), ["before", "middle", "after"]);
    assert_eq!(outcome.report.skipped_lines(), 0);
    assert_eq!(outcome.report.duplicates_removed, 0);
}

#[test]
fn a_zero_byte_write_is_a_short_write_that_stores_nothing_and_is_not_retried() {
    let outcome = survive("fault-zero", vec![Step::Take(0)]);
    assert_eq!(short(&outcome), (0, frame_len()));
    assert_eq!(outcome.calls.len(), 1);
    assert_eq!(sessions(&outcome.report.records), ["before", "after"]);
    assert_eq!(outcome.report.skipped_lines(), 0);
}

#[test]
fn a_one_byte_write_leaves_a_lone_separator_that_costs_no_other_record() {
    let outcome = survive("fault-one-byte", vec![Step::Take(1)]);
    assert_eq!(short(&outcome), (1, frame_len()));
    assert_eq!(outcome.calls.len(), 1);
    assert_eq!(sessions(&outcome.report.records), ["before", "after"]);
    assert_eq!(outcome.report.skipped_lines(), 0);
}

#[test]
fn a_write_of_all_but_one_byte_leaves_one_torn_frame_and_never_a_record() {
    let outcome = survive("fault-n-minus-one", vec![Step::Take(frame_len() - 1)]);
    assert_eq!(short(&outcome), (frame_len() - 1, frame_len()));
    assert_eq!(outcome.calls.len(), 1);
    assert_eq!(sessions(&outcome.report.records), ["before", "after"]);
    assert_eq!(outcome.report.torn_frames, 1);
    assert_eq!(outcome.report.malformed_lines, 0);
    assert!(!outcome.report.truncated_last_line);
}

#[test]
fn a_write_of_half_the_frame_leaves_one_torn_frame_and_the_next_record_is_read() {
    let outcome = survive("fault-half", vec![Step::Take(frame_len() / 2)]);
    assert_eq!(short(&outcome), (frame_len() / 2, frame_len()));
    assert_eq!(sessions(&outcome.report.records), ["before", "after"]);
    assert_eq!(outcome.report.torn_frames, 1);
}

#[test]
fn enospc_stores_nothing_and_reports_the_error_without_a_retry() {
    let outcome = survive("fault-enospc", vec![Step::Fail(libc::ENOSPC)]);
    os_error(&outcome, libc::ENOSPC);
    assert_eq!(outcome.calls.len(), 1);
    assert_eq!(sessions(&outcome.report.records), ["before", "after"]);
    assert_eq!(outcome.report.skipped_lines(), 0);
}

#[test]
fn eio_stores_nothing_and_reports_the_error_without_a_retry() {
    let outcome = survive("fault-eio", vec![Step::Fail(libc::EIO)]);
    os_error(&outcome, libc::EIO);
    assert_eq!(outcome.calls.len(), 1);
    assert_eq!(sessions(&outcome.report.records), ["before", "after"]);
    assert_eq!(outcome.report.skipped_lines(), 0);
}

#[test]
fn eio_after_bytes_reached_the_file_leaves_one_torn_frame_and_the_next_record_is_read() {
    let outcome = survive(
        "fault-eio-landed",
        vec![Step::LandThenFail(frame_len() - 1, libc::EIO)],
    );
    os_error(&outcome, libc::EIO);
    assert_eq!(sessions(&outcome.report.records), ["before", "after"]);
    assert_eq!(outcome.report.torn_frames, 1);
}

#[test]
fn enospc_after_the_whole_frame_but_the_newline_landed_still_costs_no_other_record() {
    let outcome = survive(
        "fault-enospc-landed",
        vec![Step::LandThenFail(frame_len() - 1, libc::ENOSPC)],
    );
    os_error(&outcome, libc::ENOSPC);
    assert_eq!(sessions(&outcome.report.records), ["before", "after"]);
    assert_eq!(outcome.report.torn_frames, 1);
}

#[test]
fn a_cut_at_any_length_costs_only_the_cut_record() {
    for kept in 0..frame_len() {
        let outcome = survive(&format!("fault-sweep-{kept}"), vec![Step::Take(kept)]);
        assert_eq!(short(&outcome), (kept, frame_len()), "kept {kept}");
        assert_eq!(
            sessions(&outcome.report.records),
            ["before", "after"],
            "kept {kept}"
        );
        assert_eq!(outcome.report.malformed_lines, 0, "kept {kept}");
        assert_eq!(outcome.report.torn_frames, usize::from(kept > 1), "kept {kept}");
        assert!(!outcome.report.truncated_last_line, "kept {kept}");
    }
}
