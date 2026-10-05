use std::fmt::Write as _;
use std::io;
use std::process::ExitCode;

use agentdust_agents::claude_setup::{self, Overall, SetupEnv};
use agentdust_core::config::{ApplySwitch, Disabled, apply_switch};
use agentdust_core::journal;
use agentdust_core::paths;
use agentdust_core::safe_open::{self, SafeOpenError};

use crate::setup::Context;

pub fn run() -> ExitCode {
    let data_dir = match paths::data_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("agentdust status: cannot locate the data directory: {err}");
            return ExitCode::FAILURE;
        }
    };
    let mut out = format!("agentdust {}\n", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(out, "data directory: {}", data_dir.display());
    let state = match safe_open::check_dir(&data_dir) {
        Ok(()) => "ok".to_owned(),
        Err(SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => {
            "missing (created by setup or by the first hook run)".to_owned()
        }
        Err(err) => format!("refused: {err}"),
    };
    let _ = writeln!(out, "data directory state: {state}");
    match journal::status(&data_dir) {
        Ok(facts) => {
            let _ = writeln!(out, "filesystem: {}", facts.describe());
        }
        Err(err) => {
            let _ = writeln!(out, "filesystem: unknown ({err})");
        }
    }
    match journal::read(&data_dir) {
        Ok(report) => {
            let yes_no = |flag: bool| if flag { "yes" } else { "no" };
            let _ = writeln!(out, "journal records: {}", report.records.len());
            let _ = writeln!(out, "journal skipped lines: {}", report.skipped_lines());
            if report.skipped_lines() > 0 {
                let _ = writeln!(
                    out,
                    "journal skipped detail: malformed {}, torn frames {}, newer version {}, unknown kind {}",
                    report.malformed_lines,
                    report.torn_frames,
                    report.newer_version_lines,
                    report.unknown_kind_lines
                );
            }
            let _ = writeln!(
                out,
                "journal truncated last line: {}",
                yes_no(report.truncated_last_line)
            );
            let _ = writeln!(
                out,
                "journal unsupported version: {}",
                yes_no(report.unsupported_version)
            );
            let _ = writeln!(out, "journal duplicates removed: {}", report.duplicates_removed);
            let _ = writeln!(out, "journal unsafe files: {}", report.unsafe_files);
        }
        Err(err) => {
            let _ = writeln!(out, "journal: unavailable ({err})");
        }
    }
    out.push_str(&setup_line(&data_dir));
    let apply = match apply_switch(&data_dir) {
        ApplySwitch::Enabled => "enabled".to_owned(),
        ApplySwitch::Disabled(Disabled::SwitchedOff) => "disabled (apply = false in config.toml)".to_owned(),
        ApplySwitch::Disabled(reason) => match reason {
            Disabled::Unsafe(detail) => format!("disabled (config.toml cannot be used: {detail})"),
            Disabled::Unreadable => "disabled (config.toml cannot be used: unreadable)".to_owned(),
            Disabled::Invalid => "disabled (config.toml cannot be used: invalid)".to_owned(),
            Disabled::SwitchedOff => unreachable!(),
        },
    };
    let _ = writeln!(out, "apply: {apply}");
    print!("{out}");
    ExitCode::SUCCESS
}

fn setup_line(data_dir: &std::path::Path) -> String {
    let context = match Context::detect() {
        Ok(context) => context,
        Err(err) => return format!("setup: unknown ({err})\n"),
    };
    let env = SetupEnv {
        data_dir: data_dir.to_owned(),
        runner: None,
        claude_cli: None,
        ..context.env()
    };
    let report = claude_setup::check(&env, false);
    let overall = report.overall();
    let word = match overall {
        Overall::Installed => "installed",
        Overall::NotInstalled => "not installed",
        Overall::Partial => "partly installed",
        Overall::Modified => "modified",
        Overall::Unknown => "unknown",
    };
    let mut line = format!("setup: {word}\n");
    if !matches!(overall, Overall::Installed | Overall::NotInstalled) {
        line.push_str("  run agentdust setup --check for details\n");
    }
    line
}
