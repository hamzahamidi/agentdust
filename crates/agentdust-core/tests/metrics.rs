mod scratch;

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use agentdust_core::class::Class;
use agentdust_core::metrics::{Mode, OUTCOME_FILE, OutcomeLog};
use scratch::TempDir;
use serde_json::Value;

const DAY_MS: u64 = 86_400_000;
const START: u64 = 1_700_000_000_000;

#[test]
fn an_empty_metrics_read_does_not_create_state() {
    let dir = TempDir::absent("metrics-empty");
    let summary = OutcomeLog::summary(&dir).unwrap();
    assert_eq!(summary.event_count, 0);
    assert!(summary.daily.is_empty());
    assert!(!dir.exists());
}

#[test]
fn stored_outcomes_have_only_the_fields_needed_for_aggregation() {
    let dir = TempDir::absent("metrics-private");
    OutcomeLog::new(&dir)
        .append(Mode::Automatic, Class::OwnedEnded, "terminated", START)
        .unwrap();

    let path = dir.join(OUTCOME_FILE);
    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(text.lines().count(), 1);
    let value: Value = serde_json::from_str(text.trim()).unwrap();
    let keys: BTreeSet<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, BTreeSet::from(["v", "wall_ms", "mode", "class", "result"]));
    assert_eq!(value["mode"], "automatic");
    assert_eq!(value["class"], "owned-ended");
    assert_eq!(value["result"], "terminated");
    let metadata = fs::metadata(path).unwrap();
    assert_eq!(metadata.permissions().mode() & 0o7777, 0o600);
    assert_eq!(metadata.nlink(), 1);
    assert_eq!(
        fs::metadata(dir.path()).unwrap().permissions().mode() & 0o7777,
        0o700
    );
}

#[test]
fn summary_counts_outcomes_and_builds_sorted_daily_chart_buckets() {
    let dir = TempDir::absent("metrics-summary");
    let log = OutcomeLog::new(&dir);
    log.append(Mode::Manual, Class::Suspect, "declined", START + DAY_MS)
        .unwrap();
    log.append(Mode::Automatic, Class::OwnedEnded, "terminated", START)
        .unwrap();
    log.append(Mode::Automatic, Class::OwnedEnded, "survivor", START + 1_000)
        .unwrap();

    let summary = OutcomeLog::summary(&dir).unwrap();
    assert_eq!(summary.event_count, 3);
    assert_eq!(summary.start_wall_ms, Some(START));
    assert_eq!(summary.end_wall_ms, Some(START + DAY_MS));
    assert_eq!(summary.by_outcome["terminated"], 1);
    assert_eq!(summary.by_outcome["survivor"], 1);
    assert_eq!(summary.by_outcome["declined"], 1);
    assert_eq!(summary.by_mode["automatic"]["terminated"], 1);
    assert_eq!(summary.by_class["owned-ended"], 2);
    assert_eq!(summary.daily.len(), 2);
    assert!(summary.daily[0].day_start_wall_ms < summary.daily[1].day_start_wall_ms);
    assert_eq!(summary.daily[0].by_mode["automatic"]["survivor"], 1);
    assert_eq!(summary.daily[1].by_mode["manual"]["declined"], 1);
}

#[test]
fn rotation_keeps_a_bounded_recent_history() {
    let dir = TempDir::absent("metrics-rotate");
    let log = OutcomeLog::with_limit(&dir, 220);
    for index in 0..20 {
        log.append(Mode::Automatic, Class::OwnedEnded, "terminated", START + index)
            .unwrap();
    }
    assert!(dir.join("outcomes.jsonl.1").exists());
    let summary = OutcomeLog::summary(&dir).unwrap();
    assert!(summary.event_count < 20);
    assert!(summary.event_count > 0);
    assert!(summary.start_wall_ms.unwrap() > START);
    assert_eq!(summary.end_wall_ms, Some(START + 19));
    assert!(fs::metadata(dir.join(OUTCOME_FILE)).unwrap().len() <= 220);
    assert!(fs::metadata(dir.join("outcomes.jsonl.1")).unwrap().len() <= 220);
    let retained_lines = fs::read_to_string(dir.join(OUTCOME_FILE))
        .unwrap()
        .lines()
        .count()
        + fs::read_to_string(dir.join("outcomes.jsonl.1"))
            .unwrap()
            .lines()
            .count();
    assert_eq!(summary.event_count as usize, retained_lines);
}

#[test]
fn summary_skips_malformed_and_newer_rows_across_both_generations() {
    let dir = TempDir::absent("metrics-malformed");
    let log = OutcomeLog::new(&dir);
    log.append(Mode::Manual, Class::Suspect, "declined", START)
        .unwrap();
    let current = dir.join(OUTCOME_FILE);
    let older = dir.join("outcomes.jsonl.1");
    fs::copy(&current, &older).unwrap();
    fs::set_permissions(&older, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&current, b"").unwrap();
    log.append(Mode::Automatic, Class::OwnedEnded, "terminated", START + DAY_MS)
        .unwrap();

    let mut file = OpenOptions::new().append(true).open(current).unwrap();
    file.write_all(b"not-json\n\xff\n").unwrap();
    file.write_all(
        br#"{"v":2,"wall_ms":1700000000000,"mode":"automatic","class":"owned-ended","result":"terminated"}"#,
    )
    .unwrap();
    file.write_all(b"\n{\"v\":1").unwrap();

    let summary = OutcomeLog::summary(&dir).unwrap();
    assert_eq!(summary.event_count, 2);
    assert_eq!(summary.skipped_lines, 4);
}

#[test]
fn unknown_result_codes_are_refused() {
    let dir = TempDir::absent("metrics-code");
    let err = OutcomeLog::new(&dir)
        .append(Mode::Manual, Class::OwnedEnded, "user supplied text", START)
        .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(!dir.exists());
}
