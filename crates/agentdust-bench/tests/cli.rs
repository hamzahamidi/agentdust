mod common;

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use agentdust_bench::report::RunReport;
use common::{EXE, Scratch};

fn run_with(scratch: &Scratch, json: &Path, extra: &[&str]) -> Output {
    Command::new(EXE)
        .args(["run", "--repeats", "1"])
        .args(extra)
        .arg("--json")
        .arg(json)
        .arg("--root")
        .arg(scratch.path().join("work"))
        .output()
        .unwrap()
}

fn small_run(scratch: &Scratch, json: &Path) -> Output {
    run_with(scratch, json, &["--records", "60", "--reader-records", "100"])
}

fn load(json: &Path) -> RunReport {
    serde_json::from_slice(&fs::read(json).unwrap()).unwrap()
}

fn succeeded(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_small_run_covers_every_cell_and_prints_the_tables() {
    let scratch = Scratch::new("cli-run");
    fs::create_dir_all(scratch.path()).unwrap();
    let json = scratch.path().join("result.json");
    let output = small_run(&scratch, &json);
    succeeded(&output);
    let text = String::from_utf8(output.stdout).unwrap();
    for candidate in ["A", "C", "C2", "D"] {
        assert!(text.contains(&format!("| {candidate} | 3 | 150 |")), "{text}");
        assert!(text.contains(&format!("| {candidate} | 16 | 4000 |")), "{text}");
        assert!(text.contains(&format!("| {candidate} | 4000 | 100 |")), "{text}");
    }
    let report = load(&json);
    assert_eq!(report.cells.len(), 16);
    assert_eq!(report.reader_costs.len(), 8);
    assert!(report.cells.iter().all(|cell| cell.runs.len() == 1));
    assert!(!report.load_before.is_empty() && !report.load_after.is_empty());
    assert!(!report.filesystem.is_empty());
    for cell in &report.cells {
        let run = &cell.runs[0];
        let rotator = run
            .rotator
            .as_ref()
            .expect("every cell runs a rotator by default");
        assert!(rotator.final_cycle_ok, "{} {:?}", cell.candidate, rotator);
        assert_eq!(
            (
                run.integrity.markers,
                run.integrity.torn,
                run.integrity.interleaved
            ),
            (0, 0, 0)
        );
    }
}

#[test]
fn render_prints_the_same_tables_as_the_run_that_saved_them() {
    let scratch = Scratch::new("cli-render");
    fs::create_dir_all(scratch.path()).unwrap();
    let json = scratch.path().join("result.json");
    let run = run_with(
        &scratch,
        &json,
        &[
            "--records",
            "12",
            "--reader-records",
            "20",
            "--sizes",
            "150",
            "--writers",
            "3",
        ],
    );
    succeeded(&run);
    let render = Command::new(EXE).arg("render").arg(&json).output().unwrap();
    assert!(render.status.success());
    assert_eq!(render.stdout, run.stdout);
}

#[test]
fn the_saved_json_is_small_and_keeps_one_run_per_line() {
    let scratch = Scratch::new("cli-json");
    fs::create_dir_all(scratch.path()).unwrap();
    let json = scratch.path().join("result.json");
    succeeded(&small_run(&scratch, &json));
    let text = fs::read_to_string(&json).unwrap();
    assert!(text.lines().count() < 400, "{}", text.lines().count());
    assert_eq!(text.lines().filter(|line| line.contains("\"p50\"")).count(), 16);
}

#[test]
fn a_run_that_passes_its_time_budget_stops_and_saves_nothing() {
    let scratch = Scratch::new("cli-budget");
    fs::create_dir_all(scratch.path()).unwrap();
    let json = scratch.path().join("result.json");
    let output = Command::new(EXE)
        .args(["run", "--budget-secs", "0", "--json"])
        .arg(&json)
        .arg("--root")
        .arg(scratch.path().join("work"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("budget"));
    assert!(!json.exists());
}

#[test]
fn an_unknown_subcommand_prints_usage_and_exits_two() {
    for args in [
        &["nonsense"][..],
        &[][..],
        &["writer"][..],
        &["rotator"][..],
        &["run", "--repeats", "x"][..],
        &["fs"][..],
        &["fs", "a", "b"][..],
    ] {
        let output = Command::new(EXE).args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("usage"),
            "{args:?}"
        );
        assert!(output.stdout.is_empty(), "{args:?}");
    }
}

#[test]
fn a_run_can_be_narrowed_to_one_candidate_size_and_writer_count() {
    let scratch = Scratch::new("cli-narrow");
    fs::create_dir_all(scratch.path()).unwrap();
    let json = scratch.path().join("result.json");
    let output = Command::new(EXE)
        .args([
            "run",
            "--repeats",
            "2",
            "--records",
            "48",
            "--reader-records",
            "0",
            "--sizes",
            "16384",
            "--writers",
            "16",
            "--candidates",
            "C2",
            "--json",
        ])
        .arg(&json)
        .arg("--root")
        .arg(scratch.path().join("work"))
        .output()
        .unwrap();
    succeeded(&output);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("| C2 | 16 | 16384 |"), "{text}");
    let report = load(&json);
    assert_eq!(report.cells.len(), 1);
    let cell = &report.cells[0];
    assert_eq!(
        (cell.candidate.as_str(), cell.writers, cell.size),
        ("C2", 16, 16384)
    );
    assert_eq!(cell.runs.len(), 2);
    assert_eq!(cell.runs[0].attempted, 48);
    assert!(report.reader_costs.is_empty());
}

#[test]
fn candidates_are_listed_in_the_order_given() {
    let scratch = Scratch::new("cli-order");
    fs::create_dir_all(scratch.path()).unwrap();
    let json = scratch.path().join("result.json");
    let output = run_with(
        &scratch,
        &json,
        &[
            "--records",
            "6",
            "--reader-records",
            "10",
            "--sizes",
            "150",
            "--writers",
            "3",
            "--candidates",
            "D,A,C2",
        ],
    );
    succeeded(&output);
    let report = load(&json);
    let order: Vec<_> = report.cells.iter().map(|c| c.candidate.as_str()).collect();
    assert_eq!(order, ["D", "A", "C2"]);
    let order: Vec<_> = report.reader_costs.iter().map(|c| c.candidate.as_str()).collect();
    assert_eq!(order, ["D", "A", "C2"]);
}

#[test]
fn the_rotator_can_be_switched_off_and_its_timing_set() {
    let scratch = Scratch::new("cli-rotator-off");
    fs::create_dir_all(scratch.path()).unwrap();
    let json = scratch.path().join("result.json");
    let off = run_with(
        &scratch,
        &json,
        &[
            "--records",
            "12",
            "--reader-records",
            "0",
            "--sizes",
            "150",
            "--writers",
            "3",
            "--candidates",
            "C",
            "--rotator",
            "off",
        ],
    );
    succeeded(&off);
    assert!(load(&json).cells[0].runs[0].rotator.is_none());
    let on = run_with(
        &scratch,
        &json,
        &[
            "--records",
            "12",
            "--reader-records",
            "0",
            "--sizes",
            "150",
            "--writers",
            "3",
            "--candidates",
            "C",
            "--rotator",
            "on",
            "--rotate-period-ms",
            "3",
            "--grace-ms",
            "0",
        ],
    );
    succeeded(&on);
    assert!(load(&json).cells[0].runs[0].rotator.is_some());
}

#[test]
fn a_list_flag_with_a_bad_item_is_a_usage_error() {
    for args in [
        &["run", "--sizes", "150,x"][..],
        &["run", "--sizes", ""][..],
        &["run", "--writers", "0"][..],
        &["run", "--writers", "3,"][..],
        &["run", "--candidates", "A,Z"][..],
        &["run", "--candidates", "B"][..],
        &["run", "--candidates", ""][..],
        &["run", "--rotator", "maybe"][..],
        &["run", "--sync", "maybe"][..],
        &[
            "rotator",
            "--candidate",
            "C",
            "--dir",
            "d",
            "--stop-file",
            "s",
            "--sync",
            "maybe",
        ][..],
    ] {
        let output = Command::new(EXE).args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
    }
}

#[cfg(target_os = "macos")]
#[test]
fn the_fs_subcommand_prints_the_file_system_facts_of_a_path() {
    let scratch = Scratch::new("cli-fs");
    let dir = scratch.create();
    let output = Command::new(EXE).arg("fs").arg(dir).output().unwrap();
    succeeded(&output);
    let text = String::from_utf8(output.stdout).unwrap();
    assert_eq!(text, format!("{}: apfs, local, supported\n", dir.display()));
    let missing = Command::new(EXE)
        .args(["fs", "/definitely/not/here"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(1));
    assert!(missing.stdout.is_empty());
}

#[test]
fn the_maintenance_syncs_are_on_by_default_and_can_be_switched_off() {
    let scratch = Scratch::new("cli-sync");
    fs::create_dir_all(scratch.path()).unwrap();
    let json = scratch.path().join("result.json");
    let narrow = [
        "--records",
        "12",
        "--reader-records",
        "0",
        "--sizes",
        "150",
        "--writers",
        "3",
        "--candidates",
        "C2",
    ];
    let default = run_with(&scratch, &json, &narrow);
    succeeded(&default);
    assert!(load(&json).maintenance_sync);
    assert!(String::from_utf8_lossy(&default.stdout).contains("- Maintenance sync: on\n"));
    let mut off_args = narrow.to_vec();
    off_args.extend(["--sync", "off", "--rotate-period-ms", "1"]);
    let off = run_with(&scratch, &json, &off_args);
    succeeded(&off);
    assert!(!load(&json).maintenance_sync);
    assert!(String::from_utf8_lossy(&off.stdout).contains("- Maintenance sync: off\n"));
}
