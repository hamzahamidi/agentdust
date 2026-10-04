use agentdust_bench::harness::{CellOutcome, ReaderCost};
use agentdust_bench::integrity::Integrity;
use agentdust_bench::report::{CellRuns, RunReport, parse_load, render_markdown, to_compact_json};
use agentdust_bench::stats::LatencySummary;
use agentdust_bench::worker::{ReaderOutcome, RotatorOutcome};

fn outcome(p50_ns: u64, dropped: u64) -> CellOutcome {
    CellOutcome {
        attempted: 2400,
        acked: 2400 - dropped,
        dropped,
        stale: 0,
        errors: 0,
        retried: 0,
        latency: LatencySummary {
            count: 2400,
            p50: p50_ns,
            p95: p50_ns * 2,
            p99: p50_ns * 3,
            max: p50_ns * 4,
        },
        integrity: Integrity::default(),
        files: 2,
        wall_ns: 500_000_000,
        reader: Some(ReaderOutcome {
            reads: 10,
            median_ns: 1_000_000,
            max_ns: 3_000_000,
            max_records: 2400,
            max_skipped: 0,
        }),
        rotator: Some(RotatorOutcome {
            cycles: 11,
            rotations: 10,
            rotate_busy: 1,
            retains: 10,
            retain_busy: 1,
            errors: 0,
            rewritten: 9,
            deleted: 1,
            final_cycle_ok: true,
            wait_median_ns: 250_000,
            wait_max_ns: 1_500_000,
        }),
    }
}

fn report() -> RunReport {
    RunReport {
        load_before: "5.59 5.98 6.83".to_owned(),
        load_after: "6.10 6.00 6.80".to_owned(),
        filesystem: "apfs, local, supported".to_owned(),
        maintenance_sync: true,
        repeats: 3,
        elapsed_secs: 41.5,
        cells: vec![CellRuns {
            candidate: "A".to_owned(),
            writers: 3,
            size: 150,
            runs: vec![outcome(100_000, 0), outcome(200_000, 1), outcome(300_000, 2)],
        }],
        reader_costs: vec![ReaderCost {
            candidate: "A".to_owned(),
            size: 150,
            records: 10_000,
            records_read: 10_000,
            files: 5,
            bytes: 1_500_000,
            read_ns: vec![14_000_000, 11_000_000, 12_000_000],
        }],
    }
}

#[test]
fn the_latency_table_shows_the_median_and_the_range_of_each_percentile() {
    let text = render_markdown(&report());
    assert!(
        text.contains(
            "| A | 3 | 150 | 0.200 (0.100 to 0.300) | 0.400 (0.200 to 0.600) | 0.600 (0.300 to 0.900) | 0.800 (0.400 to 1.200) | 3 | 0 | 0 | 2 |"
        ),
        "{text}"
    );
}

#[test]
fn dropped_counts_busy_drops_stale_failures_and_errors() {
    let mut data = report();
    data.cells[0].runs[0].stale = 2;
    data.cells[0].runs[1].errors = 4;
    let text = render_markdown(&data);
    assert!(
        text.contains("| 0.800 (0.400 to 1.200) | 9 | 0 | 0 | 2 |"),
        "{text}"
    );
}

#[test]
fn counts_are_totals_over_all_repeats_so_one_bad_run_cannot_hide() {
    let mut data = report();
    data.cells[0].runs[0].integrity.lost = 1;
    data.cells[0].runs[1].integrity.lost = 4;
    data.cells[0].runs[0].integrity.torn = 1;
    data.cells[0].runs[2].integrity.torn = 1;
    data.cells[0].runs[2].integrity.interleaved = 2;
    let text = render_markdown(&data);
    assert!(text.contains("| 3 | 5 | 4 | 2 |"), "{text}");
}

#[test]
fn the_rotation_table_totals_the_counts_and_shows_the_median_and_the_worst_wait() {
    let text = render_markdown(&report());
    assert!(
        text.contains("| A | 3 | 150 | 30 | 3 | 0.250 | 1.500 | 30 | 3 | 0 |"),
        "{text}"
    );
}

#[test]
fn the_rotation_table_counts_rewrites_and_deletes_as_compactions_and_reappends_apart() {
    let mut data = report();
    data.cells[0].runs[0].retried = 7;
    data.cells[0].runs[1].rotator.as_mut().unwrap().rewritten = 0;
    data.cells[0].runs[1].rotator.as_mut().unwrap().deleted = 0;
    data.cells[0].runs[2].rotator.as_mut().unwrap().wait_max_ns = 9_000_000;
    let text = render_markdown(&data);
    assert!(
        text.contains("| A | 3 | 150 | 30 | 3 | 0.250 | 9.000 | 20 | 3 | 7 |"),
        "{text}"
    );
}

#[test]
fn a_cell_without_a_rotator_has_no_rotation_row() {
    let mut data = report();
    for run in &mut data.cells[0].runs {
        run.rotator = None;
    }
    let text = render_markdown(&data);
    assert!(
        text.contains("### Rotator running during the appends\n\n| Candidate"),
        "{text}"
    );
    assert!(!text.contains("| A | 3 | 150 | 0 | 0 |"), "{text}");
}

#[test]
fn the_reader_cost_table_shows_files_bytes_and_the_read_time() {
    let text = render_markdown(&report());
    assert!(
        text.contains("| A | 150 | 10000 | 5 | 1500000 | 12.000 (11.000 to 14.000) |"),
        "{text}"
    );
}

#[test]
fn the_live_reader_table_shows_reads_and_the_largest_torn_tail() {
    let mut data = report();
    data.cells[0].runs[2].reader.as_mut().unwrap().max_skipped = 1;
    let text = render_markdown(&data);
    assert!(text.contains("| A | 3 | 150 | 10 | 1.000 | 1 |"), "{text}");
}

#[test]
fn the_header_lists_the_load_averages_file_system_sync_mode_and_run_time_one_per_line() {
    let text = render_markdown(&report());
    assert!(text.starts_with(
        "- Load averages before the run: 5.59 5.98 6.83\n- Load averages after the run: 6.10 6.00 6.80\n- Data directory file system: apfs, local, supported\n- Maintenance sync: on\n- Run time: 41.5 s. Repeats per cell: 3.\n\n"
    ));
}

#[test]
fn the_text_has_no_em_or_en_dash() {
    let text = render_markdown(&report());
    assert!(!text.contains('\u{2014}'));
    assert!(!text.contains('\u{2013}'));
}

#[test]
fn a_report_survives_a_json_round_trip() {
    let json = serde_json::to_string(&report()).unwrap();
    let back: RunReport = serde_json::from_str(&json).unwrap();
    assert_eq!(render_markdown(&back), render_markdown(&report()));
}

#[test]
fn the_saved_json_puts_every_run_on_one_line_and_reads_back_the_same() {
    let mut data = report();
    let template = data.cells[0].clone();
    for (writers, candidate) in [(16, "C"), (3, "C2"), (16, "D")] {
        data.cells.push(CellRuns {
            candidate: candidate.to_owned(),
            writers,
            ..template.clone()
        });
    }
    let json = to_compact_json(&data);
    let lines = json.lines().count();
    let runs: usize = data.cells.iter().map(|cell| cell.runs.len()).sum();
    let run_lines: Vec<_> = json.lines().filter(|line| line.contains("\"p50\"")).collect();
    assert_eq!(run_lines.len(), runs);
    assert!(
        run_lines
            .iter()
            .all(|line| line.contains("\"integrity\"") && line.contains("\"rotator\""))
    );
    assert!(
        lines <= runs + 7 * data.cells.len() + data.reader_costs.len() + 12,
        "{lines}"
    );
    let back: RunReport = serde_json::from_str(&json).unwrap();
    assert_eq!(render_markdown(&back), render_markdown(&data));
    assert!(json.ends_with("}\n"));
}

#[test]
fn twenty_cells_of_five_runs_stay_far_below_800_lines_of_json() {
    let mut data = report();
    let template = data.cells[0].clone();
    let runs = vec![outcome(100_000, 0); 5];
    data.cells = (0..20)
        .map(|n| CellRuns {
            candidate: format!("C{n}"),
            runs: runs.clone(),
            ..template.clone()
        })
        .collect();
    data.reader_costs = vec![data.reader_costs[0].clone(); 8];
    assert!(to_compact_json(&data).lines().count() < 400);
}

#[test]
fn the_load_is_read_from_macos_and_linux_uptime_output() {
    assert_eq!(
        parse_load(" 4:59  up 21 days,  2:06, 1 user, load averages: 5.59 5.98 6.83\n"),
        "5.59 5.98 6.83"
    );
    assert_eq!(
        parse_load(" 10:00:01 up 3 days,  1:02,  2 users,  load average: 0.52, 0.58, 0.59"),
        "0.52, 0.58, 0.59"
    );
    assert_eq!(parse_load("no such command"), "unavailable");
}

#[test]
fn a_run_without_maintenance_sync_says_so_in_the_header() {
    let mut data = report();
    data.maintenance_sync = false;
    let text = render_markdown(&data);
    assert!(text.contains("- Maintenance sync: off\n"), "{text}");
}
