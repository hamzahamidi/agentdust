mod common;

use std::fs;
use std::process::Command;

use agentdust_bench::Candidate;
use agentdust_bench::durability::{DurabilityReport, DurabilityRow, Mode, measure, render_durability};
use common::{EXE, Scratch};

fn row(mode: &str, p50_ns: u64) -> DurabilityRow {
    DurabilityRow {
        mode: mode.to_owned(),
        samples: 200,
        size: 150,
        p50: p50_ns,
        p95: p50_ns * 2,
        p99: p50_ns * 3,
        max: p50_ns * 4,
    }
}

fn report() -> DurabilityReport {
    DurabilityReport {
        load_before: "5.59 5.98 6.83".to_owned(),
        load_after: "6.10 6.00 6.80".to_owned(),
        filesystem: "apfs, local, supported".to_owned(),
        elapsed_secs: 3.4,
        rows: vec![
            row("none", 40_000),
            row("fsync", 1_500_000),
            row("full", 6_000_000),
        ],
    }
}

#[test]
fn the_three_modes_are_none_fsync_and_full_in_that_order() {
    let labels: Vec<_> = Mode::ALL.iter().map(|mode| mode.label()).collect();
    assert_eq!(labels, ["none", "fsync", "full"]);
    for mode in Mode::ALL {
        assert_eq!(Mode::from_label(mode.label()), Some(mode));
    }
    assert_eq!(Mode::from_label("data"), None);
}

#[test]
fn a_measurement_appends_every_sample_and_reports_ordered_percentiles() {
    for mode in Mode::ALL {
        let scratch = Scratch::new("durability");
        let row = measure(scratch.path(), mode, 20, 150).unwrap();
        assert_eq!(row.mode, mode.label());
        assert_eq!((row.samples, row.size), (20, 150));
        assert!(row.p50 > 0 && row.p50 <= row.p95 && row.p95 <= row.p99 && row.p99 <= row.max);
        let stored = Candidate::Append.open(scratch.path()).read_all().unwrap();
        assert_eq!(stored.records.len(), 20, "{}", mode.label());
        assert_eq!(stored.skipped_lines(), 0, "{}", mode.label());
    }
}

#[test]
fn a_measurement_of_no_samples_is_an_error() {
    let scratch = Scratch::new("durability-none");
    assert!(measure(scratch.path(), Mode::None, 0, 150).is_err());
}

#[test]
fn the_table_lists_one_row_per_mode_with_the_percentiles_in_milliseconds() {
    let text = render_durability(&report());
    assert!(
        text.starts_with(
            "- Load averages before the run: 5.59 5.98 6.83\n- Load averages after the run: 6.10 6.00 6.80\n- Data directory file system: apfs, local, supported\n- Run time: 3.4 s.\n\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("| Mode | Samples | Record bytes | p50 ms | p95 ms | p99 ms | max ms |\n| --- | ---: | ---: | ---: | ---: | ---: | ---: |\n"),
        "{text}"
    );
    assert!(
        text.contains("| none | 200 | 150 | 0.040 | 0.080 | 0.120 | 0.160 |\n"),
        "{text}"
    );
    assert!(
        text.contains("| fsync | 200 | 150 | 1.500 | 3.000 | 4.500 | 6.000 |\n"),
        "{text}"
    );
    assert!(
        text.contains("| full | 200 | 150 | 6.000 | 12.000 | 18.000 | 24.000 |\n"),
        "{text}"
    );
    assert!(!text.contains('\u{2014}') && !text.contains('\u{2013}'));
}

#[test]
fn a_report_survives_a_json_round_trip() {
    let json = serde_json::to_string(&report()).unwrap();
    let back: DurabilityReport = serde_json::from_str(&json).unwrap();
    assert_eq!(render_durability(&back), render_durability(&report()));
}

#[test]
fn the_subcommand_prints_the_table_and_saves_the_json() {
    let scratch = Scratch::new("durability-cli");
    fs::create_dir_all(scratch.path()).unwrap();
    let json = scratch.path().join("durability.json");
    let output = Command::new(EXE)
        .args(["durability", "--samples", "5", "--size", "150", "--json"])
        .arg(&json)
        .arg("--root")
        .arg(scratch.path().join("work"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    for mode in ["none", "fsync", "full"] {
        assert!(text.contains(&format!("| {mode} | 5 | 150 |")), "{text}");
    }
    let saved: DurabilityReport = serde_json::from_slice(&fs::read(&json).unwrap()).unwrap();
    assert_eq!(saved.rows.len(), 3);
    assert!(!saved.filesystem.is_empty());
    assert!(!scratch.path().join("work").exists());
}

#[test]
fn the_subcommand_refuses_a_bad_flag_value_with_usage() {
    for args in [
        &["durability", "--samples", "x"][..],
        &["durability", "--samples", "0"][..],
        &["durability", "--size", "-1"][..],
        &["durability", "--mode", "none"][..],
        &["durability", "--samples"][..],
    ] {
        let output = Command::new(EXE).args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
    }
}
