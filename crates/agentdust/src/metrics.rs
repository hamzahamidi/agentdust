use std::fmt::Write as _;
use std::process::ExitCode;

use agentdust_core::metrics::OutcomeLog;
use agentdust_core::paths;

pub fn run(json: bool) -> ExitCode {
    let dir = match paths::data_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("agentdust metrics: cannot locate the data directory: {err}");
            return ExitCode::FAILURE;
        }
    };
    let summary = match OutcomeLog::summary(&dir) {
        Ok(summary) => summary,
        Err(err) => {
            eprintln!("agentdust metrics: outcome history unavailable: {err}");
            return ExitCode::FAILURE;
        }
    };
    if json {
        match serde_json::to_string_pretty(&summary) {
            Ok(text) => println!("{text}"),
            Err(err) => {
                eprintln!("agentdust metrics: cannot format summary: {err}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        print_summary(&summary);
    }
    ExitCode::SUCCESS
}

fn print_summary(summary: &agentdust_core::metrics::Summary) {
    if summary.event_count == 0 {
        println!("No cleanup outcomes recorded yet.");
        return;
    }
    let mut text = format!("Cleanup outcomes: {}\n", summary.event_count);
    let _ = writeln!(text, "Proven stopped: {}", count(summary, "terminated"));
    let _ = writeln!(text, "Survived signal: {}", count(summary, "survivor"));
    let _ = writeln!(
        text,
        "Other outcomes: {}",
        summary.event_count - count(summary, "terminated") - count(summary, "survivor")
    );
    let _ = writeln!(
        text,
        "Skipped malformed or newer records: {}",
        summary.skipped_lines
    );
    text.push_str("\nBy mode:\n");
    for (mode, results) in &summary.by_mode {
        let _ = writeln!(text, "  {mode}: {}", results.values().sum::<u64>());
    }
    text.push_str("\nDaily chart data: run `agentdust metrics --json`.\n");
    print!("{text}");
}

fn count(summary: &agentdust_core::metrics::Summary, result: &str) -> u64 {
    summary.by_outcome.get(result).copied().unwrap_or_default()
}
