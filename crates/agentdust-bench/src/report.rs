use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::harness::{CellOutcome, ReaderCost};
use crate::stats::Spread;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunReport {
    pub load_before: String,
    pub load_after: String,
    pub filesystem: String,
    pub repeats: u32,
    pub elapsed_secs: f64,
    pub cells: Vec<CellRuns>,
    pub reader_costs: Vec<ReaderCost>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CellRuns {
    pub candidate: String,
    pub writers: u32,
    pub size: usize,
    pub runs: Vec<CellOutcome>,
}

pub fn parse_load(uptime: &str) -> String {
    uptime
        .split_once("load average")
        .and_then(|(_, rest)| rest.split_once(':'))
        .map(|(_, load)| load.trim().to_owned())
        .filter(|load| !load.is_empty())
        .unwrap_or_else(|| "unavailable".to_owned())
}

pub fn load_average() -> String {
    Command::new("uptime")
        .output()
        .map(|output| parse_load(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_else(|_| "unavailable".to_owned())
}

pub fn to_compact_json(report: &RunReport) -> String {
    let value = serde_json::to_value(report).unwrap_or(Value::Null);
    let mut out = String::new();
    layout(&value, 0, &mut out);
    out.push('\n');
    out
}

fn nests_objects(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.values().any(is_object_list),
        _ => false,
    }
}

fn is_object_list(value: &Value) -> bool {
    matches!(value, Value::Array(items) if !items.is_empty() && items.iter().all(Value::is_object))
}

fn layout(value: &Value, indent: usize, out: &mut String) {
    let inner = " ".repeat(indent + 2);
    match value {
        Value::Object(map) if nests_objects(value) => {
            out.push_str("{\n");
            for (index, (key, item)) in map.iter().enumerate() {
                out.push_str(&inner);
                out.push_str(&Value::String(key.clone()).to_string());
                out.push_str(": ");
                layout(item, indent + 2, out);
                out.push_str(if index + 1 == map.len() { "\n" } else { ",\n" });
            }
            out.push_str(&" ".repeat(indent));
            out.push('}');
        }
        Value::Array(items) if is_object_list(value) => {
            out.push_str("[\n");
            for (index, item) in items.iter().enumerate() {
                out.push_str(&inner);
                layout(item, indent + 2, out);
                out.push_str(if index + 1 == items.len() { "\n" } else { ",\n" });
            }
            out.push_str(&" ".repeat(indent));
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

pub fn render_markdown(report: &RunReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "- Load averages before the run: {}\n",
        report.load_before
    ));
    out.push_str(&format!("- Load averages after the run: {}\n", report.load_after));
    out.push_str(&format!("- Data directory file system: {}\n", report.filesystem));
    out.push_str(&format!(
        "- Run time: {} s. Repeats per cell: {}.\n\n",
        plain(report.elapsed_secs),
        report.repeats
    ));
    out.push_str("### Append latency under contention and rotation\n\n");
    out.push_str(&table(
        &[
            "Candidate",
            "Writers",
            "Record bytes",
            "p50 ms",
            "p95 ms",
            "p99 ms",
            "max ms",
            "Dropped",
            "Lost",
            "Torn or interleaved",
            "Files",
        ],
        report.cells.iter().map(latency_row).collect(),
    ));
    out.push_str("\n### Consistency and throughput\n\n");
    out.push_str(&table(
        &[
            "Candidate",
            "Writers",
            "Record bytes",
            "Appends per second",
            "Out of order",
            "Duplicates",
            "Unacknowledged but present",
            "Timestamp inversions",
        ],
        report.cells.iter().map(consistency_row).collect(),
    ));
    out.push_str("\n### Rotator running during the appends\n\n");
    out.push_str(&table(
        &[
            "Candidate",
            "Writers",
            "Record bytes",
            "Rotations",
            "Rotations skipped",
            "Median rotation wait ms",
            "Max rotation wait ms",
            "Compactions",
            "Compactions skipped",
            "Re-appended records",
        ],
        report.cells.iter().filter_map(rotation_row).collect(),
    ));
    out.push_str("\n### Reader running during the appends\n\n");
    out.push_str(&table(
        &[
            "Candidate",
            "Writers",
            "Record bytes",
            "Reads",
            "Read ms",
            "Most torn tail lines seen",
        ],
        report.cells.iter().filter_map(live_reader_row).collect(),
    ));
    out.push_str("\n### Reader cost on a prepared store\n\n");
    out.push_str(&table(
        &[
            "Candidate",
            "Record bytes",
            "Records",
            "Files",
            "Bytes on disk",
            "Read ms",
        ],
        report.reader_costs.iter().map(reader_cost_row).collect(),
    ));
    out
}

fn table(headers: &[&str], rows: Vec<Vec<String>>) -> String {
    let mut out = format!("| {} |\n", headers.join(" | "));
    let rules: Vec<&str> = headers
        .iter()
        .enumerate()
        .map(|(i, _)| if i == 0 { "---" } else { "---:" })
        .collect();
    out.push_str(&format!("| {} |\n", rules.join(" | ")));
    for row in rows {
        out.push_str(&format!("| {} |\n", row.join(" | ")));
    }
    out
}

fn plain(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

fn ms(ns: f64) -> String {
    format!("{:.3}", ns / 1_000_000.0)
}

fn spread_ms(values: impl Iterator<Item = f64>) -> String {
    match Spread::of(&values.collect::<Vec<_>>()) {
        Some(s) => format!("{} ({} to {})", ms(s.median), ms(s.min), ms(s.max)),
        None => "n/a".to_owned(),
    }
}

fn median(values: impl Iterator<Item = f64>) -> Option<f64> {
    Spread::of(&values.collect::<Vec<_>>()).map(|s| s.median)
}

fn latency_row(cell: &CellRuns) -> Vec<String> {
    let total = |pick: fn(&CellOutcome) -> u64| cell.runs.iter().map(pick).sum::<u64>();
    vec![
        cell.candidate.clone(),
        cell.writers.to_string(),
        cell.size.to_string(),
        spread_ms(cell.runs.iter().map(|r| r.latency.p50 as f64)),
        spread_ms(cell.runs.iter().map(|r| r.latency.p95 as f64)),
        spread_ms(cell.runs.iter().map(|r| r.latency.p99 as f64)),
        spread_ms(cell.runs.iter().map(|r| r.latency.max as f64)),
        total(|r| r.dropped + r.stale + r.errors).to_string(),
        total(|r| r.integrity.lost).to_string(),
        total(|r| r.integrity.torn + r.integrity.interleaved).to_string(),
        median(cell.runs.iter().map(|r| r.files as f64)).map_or("n/a".to_owned(), plain),
    ]
}

fn consistency_row(cell: &CellRuns) -> Vec<String> {
    let total = |pick: fn(&CellOutcome) -> u64| cell.runs.iter().map(pick).sum::<u64>();
    let rate = median(
        cell.runs
            .iter()
            .map(|r| r.acked as f64 / (r.wall_ns.max(1) as f64 / 1e9)),
    );
    vec![
        cell.candidate.clone(),
        cell.writers.to_string(),
        cell.size.to_string(),
        rate.map_or("n/a".to_owned(), |rate| format!("{rate:.0}")),
        total(|r| r.integrity.out_of_order).to_string(),
        total(|r| r.integrity.duplicates).to_string(),
        total(|r| r.integrity.unacked).to_string(),
        total(|r| r.integrity.mono_inversions).to_string(),
    ]
}

fn rotation_row(cell: &CellRuns) -> Option<Vec<String>> {
    let rotators: Vec<_> = cell.runs.iter().filter_map(|r| r.rotator.as_ref()).collect();
    if rotators.is_empty() {
        return None;
    }
    let total =
        |pick: fn(&crate::worker::RotatorOutcome) -> u64| rotators.iter().map(|r| pick(r)).sum::<u64>();
    Some(vec![
        cell.candidate.clone(),
        cell.writers.to_string(),
        cell.size.to_string(),
        total(|r| r.rotations).to_string(),
        total(|r| r.rotate_busy).to_string(),
        median(rotators.iter().map(|r| r.wait_median_ns as f64)).map_or("n/a".to_owned(), ms),
        rotators
            .iter()
            .map(|r| r.wait_max_ns)
            .max()
            .map_or("n/a".to_owned(), |ns| ms(ns as f64)),
        total(|r| r.rewritten + r.deleted).to_string(),
        total(|r| r.retain_busy).to_string(),
        cell.runs.iter().map(|r| r.retried).sum::<u64>().to_string(),
    ])
}

fn live_reader_row(cell: &CellRuns) -> Option<Vec<String>> {
    let readers: Vec<_> = cell.runs.iter().filter_map(|r| r.reader.as_ref()).collect();
    if readers.is_empty() {
        return None;
    }
    Some(vec![
        cell.candidate.clone(),
        cell.writers.to_string(),
        cell.size.to_string(),
        median(readers.iter().map(|r| r.reads as f64)).map_or("n/a".to_owned(), plain),
        median(readers.iter().map(|r| r.median_ns as f64)).map_or("n/a".to_owned(), ms),
        readers
            .iter()
            .map(|r| r.max_skipped)
            .max()
            .unwrap_or(0)
            .to_string(),
    ])
}

fn reader_cost_row(cost: &ReaderCost) -> Vec<String> {
    let records = if cost.records_read == cost.records {
        cost.records.to_string()
    } else {
        format!("{} (read {})", cost.records, cost.records_read)
    };
    vec![
        cost.candidate.clone(),
        cost.size.to_string(),
        records,
        cost.files.to_string(),
        cost.bytes.to_string(),
        spread_ms(cost.read_ns.iter().map(|ns| *ns as f64)),
    ]
}
