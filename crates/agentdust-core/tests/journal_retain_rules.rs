mod journal_support;
mod maintenance_support;
mod scratch;

use std::fs;

use agentdust_core::journal::retention::Degraded;
use agentdust_core::journal::{Agent, Kind, MaintenanceError, Record};
use journal_support::{count_of, frame, frames, generation_name, journal, names_in, plant};
use maintenance_support::{BOOT, T, expired, numbered, on_boot, plant_generation, policy};
use scratch::TempDir;

const LIMIT_MS: u64 = 1_000;

fn ended(session: &str, wall_ts: u64, mono_ts: u64) -> [Record; 2] {
    let mut start = numbered(session, mono_ts);
    start.kind = Kind::SessionStart;
    start.wall_ts = wall_ts;
    let mut end = numbered(session, mono_ts + 1);
    end.kind = Kind::SessionEnd;
    end.wall_ts = wall_ts;
    [start, end]
}

fn pinned(session: &str, wall_ts: u64, mono_ts: u64) -> Record {
    let mut record = numbered(session, mono_ts);
    record.wall_ts = wall_ts;
    record
}

fn retain_with(
    dir: &std::path::Path,
    max_bytes: u64,
) -> Result<agentdust_core::journal::RetainReport, MaintenanceError> {
    journal(dir).retain(&policy(LIMIT_MS, max_bytes), T, BOOT)
}

fn present(dir: &std::path::Path) -> Vec<String> {
    let read = journal(dir).read().unwrap();
    let mut sessions: Vec<String> = read.records.iter().map(|r| r.session_id.clone()).collect();
    sessions.sort();
    sessions
}

#[test]
fn an_ended_session_past_the_age_limit_goes_and_a_pinned_one_stays() {
    let dir = TempDir::private("rules-aged");
    let mut records = ended("done", T - 5_000, 1).to_vec();
    records.push(pinned("live", T - 5_000, 3));
    plant_generation(&dir, 100, &records);

    let report = retain_with(&dir, u64::MAX).unwrap();

    assert_eq!(report.dropped_aged, 2);
    assert_eq!(present(&dir), ["live"]);
}

fn resumed(session: &str, wall_ts: u64, mono_ts: u64) -> [Record; 2] {
    let mut start = pinned(session, wall_ts, mono_ts);
    start.kind = Kind::SessionStart;
    let activity = pinned(session, wall_ts + 1, mono_ts + 1);
    [start, activity]
}

#[test]
fn a_session_resumed_after_its_end_is_not_aged_out() {
    let dir = TempDir::private("rules-resumed-aged");
    plant_generation(&dir, 100, &ended("again", T - 9_000, 1));
    plant_generation(&dir, 200, &resumed("again", T - 8_000, 5));

    let report = retain_with(&dir, u64::MAX).unwrap();

    assert_eq!((report.dropped_aged, report.kept_records), (0, 4));
    assert!(report.degraded.is_empty());
    assert_eq!(present(&dir), ["again"; 4]);
}

#[test]
fn a_resumed_session_loses_evidence_only_as_pinned_evidence_and_reports_it() {
    let dir = TempDir::private("rules-resumed-cap");
    let mut all = ended("again", T - 500, 1).to_vec();
    all.extend(resumed("again", T - 400, 5));
    plant_generation(&dir, 100, &all[..2]);
    plant_generation(&dir, 200, &all[2..]);
    plant_generation(&dir, 300, &ended("done", T - 300, 10));
    let done: u64 = frames(&ended("done", T - 300, 10)).len() as u64;
    let total: u64 = frames(&all).len() as u64 + done;

    let ended_goes_first = retain_with(&dir, total - done).unwrap();
    assert_eq!(ended_goes_first.dropped_over_cap, 2);
    assert!(ended_goes_first.degraded.is_empty());
    assert_eq!(present(&dir), ["again"; 4]);

    let tighter = retain_with(&dir, frames(&all).len() as u64 - 1).unwrap();
    assert_eq!(tighter.dropped_over_cap, 0);
    assert_eq!(tighter.dropped_pinned, 1);
    assert_eq!(
        tighter.degraded,
        [Degraded {
            agent: Agent::Claude,
            session_id: "again".to_owned(),
            dropped_records: 1
        }]
    );
    assert_eq!(present(&dir), ["again"; 3]);
}

#[test]
fn a_young_ended_session_is_kept() {
    let dir = TempDir::private("rules-young");
    plant_generation(&dir, 100, &ended("done", T - 500, 1));
    let report = retain_with(&dir, u64::MAX).unwrap();
    assert_eq!(
        (report.dropped_aged, report.kept_records, report.untouched),
        (0, 2, 1)
    );
}

#[test]
fn a_session_that_spans_two_generations_is_dropped_from_both() {
    let dir = TempDir::private("rules-spans");
    let [start, end] = ended("done", T - 5_000, 1);
    plant_generation(&dir, 100, &[start, pinned("live", T, 5)]);
    plant_generation(&dir, 200, &[end]);

    let report = retain_with(&dir, u64::MAX).unwrap();

    assert_eq!((report.rewritten, report.deleted), (1, 1));
    assert_eq!(present(&dir), ["live"]);
    assert_eq!(names_in(&dir), [generation_name(100), "journal.maint".to_owned()]);
}

#[test]
fn a_late_event_in_the_active_file_keeps_an_ended_session_in_the_generations() {
    let dir = TempDir::private("rules-late");
    plant_generation(&dir, 100, &ended("done", T - 5_000, 1));
    plant(&dir, "journal.jsonl", &frame(&pinned("done", T - 10, 9)));

    let report = retain_with(&dir, u64::MAX).unwrap();

    assert_eq!(report.dropped_aged, 0);
    assert_eq!(count_of(&journal(&dir).read().unwrap().records, "done"), 3);
}

#[test]
fn records_in_the_active_file_are_never_dropped_and_count_toward_the_limit() {
    let dir = TempDir::private("rules-active");
    plant_generation(&dir, 100, &ended("done", T - 100, 1));
    let active = [
        on_boot(numbered("active-old", 7), "older"),
        pinned("active", T, 8),
    ];
    plant(&dir, "journal.jsonl", &frames(&active));
    let held_bytes = frames(&active).len() as u64;

    let report = retain_with(&dir, held_bytes).unwrap();

    assert_eq!((report.dropped_earlier_boot, report.dropped_over_cap), (0, 2));
    assert_eq!(present(&dir), ["active", "active-old"]);
}

#[test]
fn the_oldest_ended_session_goes_first_when_the_journal_is_over_the_limit() {
    let dir = TempDir::private("rules-cap");
    let one = frames(&ended("e1", T, 11)).len() as u64;
    let mut records = Vec::new();
    for (n, session) in ["e1", "e2", "e3"].into_iter().enumerate() {
        records.extend(ended(session, T - 300 + 100 * n as u64, 10 * n as u64 + 11));
    }
    plant_generation(&dir, 100, &records);

    let report = retain_with(&dir, 2 * one + 1).unwrap();

    assert_eq!(report.dropped_over_cap, 2);
    assert_eq!(present(&dir), ["e2", "e2", "e3", "e3"]);
}

#[test]
fn pinned_evidence_dropped_for_the_limit_is_reported_as_degraded() {
    let dir = TempDir::private("rules-degraded");
    let records: Vec<_> = (0..5).map(|n| pinned("p", T - 100 + n, 10 + n)).collect();
    let one = frame(&records[0]).len() as u64;
    plant_generation(&dir, 100, &records);

    let report = retain_with(&dir, 3 * one).unwrap();

    assert_eq!(report.dropped_pinned, 2);
    assert_eq!(
        report.degraded,
        [Degraded {
            agent: Agent::Claude,
            session_id: "p".to_owned(),
            dropped_records: 2
        }]
    );
    assert_eq!(present(&dir).len(), 3);
}

#[test]
fn a_session_that_stays_within_the_limit_is_never_reported_as_degraded() {
    let dir = TempDir::private("rules-not-degraded");
    let records: Vec<_> = (0..5).map(|n| pinned("p", T - 100 + n, 10 + n)).collect();
    plant_generation(&dir, 100, &records);
    let report = retain_with(&dir, u64::MAX).unwrap();
    assert!(report.degraded.is_empty());
    assert_eq!(report.dropped_pinned, 0);
}

#[test]
fn the_size_of_a_record_is_its_whole_frame() {
    let dir = TempDir::private("rules-frame-size");
    let records: Vec<_> = (0..3).map(|n| pinned("p", T - 100 + n, 10 + n)).collect();
    plant_generation(&dir, 100, &records);
    let exact: u64 = records.iter().map(|r| frame(r).len() as u64).sum();

    let fits = retain_with(&dir, exact).unwrap();
    assert_eq!(fits.dropped_pinned, 0);

    let short = retain_with(&dir, exact - 1).unwrap();
    assert_eq!(short.dropped_pinned, 1);
}

#[test]
fn the_default_limits_keep_a_recent_ended_session() {
    let dir = TempDir::private("rules-defaults");
    plant_generation(&dir, 100, &ended("done", T - 60_000, 1));
    let report = journal(&dir)
        .retain(&agentdust_core::journal::retention::Policy::default(), T, BOOT)
        .unwrap();
    assert_eq!(report.kept_records, 2);
}

#[test]
fn records_of_an_earlier_boot_go_whatever_their_session_state() {
    let dir = TempDir::private("rules-boot");
    plant_generation(
        &dir,
        100,
        &[
            expired("a", 1),
            on_boot(ended("b", T, 2)[1].clone(), "older"),
            pinned("c", T, 5),
        ],
    );
    let report = retain_with(&dir, u64::MAX).unwrap();
    assert_eq!(report.dropped_earlier_boot, 2);
    assert_eq!(present(&dir), ["c"]);
    assert_eq!(
        fs::read(dir.join(generation_name(100))).unwrap(),
        frame(&pinned("c", T, 5))
    );
}
