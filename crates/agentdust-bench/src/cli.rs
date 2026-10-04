use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;
use std::time::Instant;

use crate::candidates::Candidate;
use crate::fsinfo;
use crate::harness::{Cell, measure_reader_cost, run_cell};
use crate::report::{CellRuns, RunReport, load_average, render_markdown, to_compact_json};
use crate::worker::{ReaderPlan, RotatorPlan, WriterPlan, run_reader, run_rotator, run_writer};

const USAGE: &str = "usage: journal-bench run [--repeats N] [--records N] [--reader-records N] [--budget-secs N] [--json PATH] [--root DIR]
                         [--sizes N,N] [--writers N,N] [--candidates A,C,C2,D] [--rotator on|off]
                         [--rotate-period-ms N] [--grace-ms N]
       journal-bench render PATH
       journal-bench fs PATH
       journal-bench writer --candidate A|C|C2|D --dir DIR --writer N --records N --size N [--pace-us N]
       journal-bench reader --candidate A|C|C2|D --dir DIR --size N --stop-file PATH [--interval-ms N]
       journal-bench rotator --candidate A|C|C2|D --dir DIR --stop-file PATH [--period-ms N] [--grace-ms N]
                         [--budget-ms N] [--final-budget-ms N]";

const DEFAULT_SIZES: [usize; 2] = [150, 4000];
const DEFAULT_WRITER_COUNTS: [u32; 2] = [3, 16];

enum CliError {
    Usage,
    Failed(String),
}

impl From<io::Error> for CliError {
    fn from(err: io::Error) -> Self {
        CliError::Failed(err.to_string())
    }
}

pub fn run(args: Vec<String>) -> ExitCode {
    match dispatch(args.get(1..).unwrap_or_default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::Usage) => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
        Err(CliError::Failed(message)) => {
            eprintln!("journal-bench: {message}");
            ExitCode::from(1)
        }
    }
}

fn dispatch(args: &[String]) -> Result<(), CliError> {
    let (command, rest) = args.split_first().ok_or(CliError::Usage)?;
    match command.as_str() {
        "run" => run_all(&Flags::parse(rest, &RUN_FLAGS)?),
        "render" => render(rest),
        "fs" => facts(rest),
        "writer" => writer(&Flags::parse(rest, &WRITER_FLAGS)?),
        "reader" => reader(&Flags::parse(rest, &READER_FLAGS)?),
        "rotator" => rotator(&Flags::parse(rest, &ROTATOR_FLAGS)?),
        _ => Err(CliError::Usage),
    }
}

const RUN_FLAGS: [&str; 12] = [
    "--repeats",
    "--records",
    "--reader-records",
    "--budget-secs",
    "--json",
    "--root",
    "--sizes",
    "--writers",
    "--candidates",
    "--rotator",
    "--rotate-period-ms",
    "--grace-ms",
];
const WRITER_FLAGS: [&str; 6] = [
    "--candidate",
    "--dir",
    "--writer",
    "--records",
    "--size",
    "--pace-us",
];
const READER_FLAGS: [&str; 5] = ["--candidate", "--dir", "--size", "--stop-file", "--interval-ms"];
const ROTATOR_FLAGS: [&str; 7] = [
    "--candidate",
    "--dir",
    "--stop-file",
    "--period-ms",
    "--grace-ms",
    "--budget-ms",
    "--final-budget-ms",
];

struct Flags(HashMap<String, String>);

impl Flags {
    fn parse(args: &[String], allowed: &[&str]) -> Result<Self, CliError> {
        let mut values = HashMap::new();
        for pair in args.chunks(2) {
            let [name, value] = pair else {
                return Err(CliError::Usage);
            };
            if !allowed.contains(&name.as_str()) || values.insert(name.clone(), value.clone()).is_some() {
                return Err(CliError::Usage);
            }
        }
        Ok(Self(values))
    }

    fn text(&self, name: &str) -> Result<&str, CliError> {
        self.0.get(name).map(String::as_str).ok_or(CliError::Usage)
    }

    fn number<T: FromStr>(&self, name: &str, default: Option<T>) -> Result<T, CliError> {
        match self.0.get(name) {
            Some(value) => value.parse().map_err(|_| CliError::Usage),
            None => default.ok_or(CliError::Usage),
        }
    }

    fn items<T>(
        &self,
        name: &str,
        default: Vec<T>,
        parse: impl Fn(&str) -> Option<T>,
    ) -> Result<Vec<T>, CliError> {
        match self.0.get(name) {
            None => Ok(default),
            Some(list) => list
                .split(',')
                .map(|item| parse(item).ok_or(CliError::Usage))
                .collect(),
        }
    }

    fn candidate(&self) -> Result<Candidate, CliError> {
        Candidate::from_label(self.text("--candidate")?).ok_or(CliError::Usage)
    }

    fn switch(&self, name: &str, default: bool) -> Result<bool, CliError> {
        match self.0.get(name).map(String::as_str) {
            None => Ok(default),
            Some("on") => Ok(true),
            Some("off") => Ok(false),
            Some(_) => Err(CliError::Usage),
        }
    }
}

fn run_all(flags: &Flags) -> Result<(), CliError> {
    let repeats: u32 = flags.number("--repeats", Some(5))?;
    let records: u64 = flags.number("--records", Some(2400))?;
    let reader_records: u64 = flags.number("--reader-records", Some(10_000))?;
    let budget_secs: u64 = flags.number("--budget-secs", Some(60))?;
    let rotate_period_ms: u64 = flags.number("--rotate-period-ms", Some(10))?;
    let grace_ms: u64 = flags.number("--grace-ms", Some(0))?;
    let rotator = flags.switch("--rotator", true)?;
    let sizes = flags.items("--sizes", DEFAULT_SIZES.to_vec(), |item| item.parse().ok())?;
    let writer_counts = flags.items("--writers", DEFAULT_WRITER_COUNTS.to_vec(), |item| {
        item.parse().ok().filter(|count| *count >= 1)
    })?;
    let candidates = flags.items("--candidates", Candidate::ALL.to_vec(), Candidate::from_label)?;
    let root = match flags.0.get("--root") {
        Some(root) => PathBuf::from(root),
        None => std::env::temp_dir().join(format!("agentdust-bench-{}", std::process::id())),
    };
    let exe = std::env::current_exe()?;
    let started = Instant::now();
    let within_budget = || {
        if started.elapsed().as_secs() >= budget_secs {
            Err(CliError::Failed(format!(
                "the time budget of {budget_secs} s is used up"
            )))
        } else {
            Ok(())
        }
    };
    let load_before = load_average();
    fs::create_dir_all(&root)?;
    let filesystem = fsinfo::probe(&root)
        .map(|facts| facts.describe())
        .unwrap_or_else(|_| "unavailable".to_owned());
    let mut cells = Vec::new();
    for &size in &sizes {
        for &writers in &writer_counts {
            for &candidate in &candidates {
                cells.push(CellRuns {
                    candidate: candidate.label().to_owned(),
                    writers,
                    size,
                    runs: Vec::new(),
                });
            }
        }
    }
    for _ in 0..repeats {
        for runs in &mut cells {
            within_budget()?;
            let candidate = Candidate::from_label(&runs.candidate).ok_or(CliError::Usage)?;
            let cell = Cell {
                candidate,
                writers: runs.writers,
                records_per_writer: (records / u64::from(runs.writers)).max(1),
                size: runs.size,
                reader: true,
                rotator,
                pace_us: 0,
                rotate_period_ms,
                grace_ms,
            };
            runs.runs.push(run_cell(&exe, &root, &cell)?);
        }
    }
    let mut reader_costs = Vec::new();
    if reader_records > 0 {
        for &size in &sizes {
            for &candidate in &candidates {
                within_budget()?;
                reader_costs.push(measure_reader_cost(
                    candidate,
                    &root,
                    reader_records,
                    size,
                    repeats,
                )?);
            }
        }
    }
    let _ = fs::remove_dir(&root);
    let report = RunReport {
        load_before,
        load_after: load_average(),
        filesystem,
        repeats,
        elapsed_secs: (started.elapsed().as_secs_f64() * 10.0).round() / 10.0,
        cells,
        reader_costs,
    };
    print_text(&render_markdown(&report))?;
    if let Some(path) = flags.0.get("--json") {
        fs::write(path, to_compact_json(&report))?;
    }
    Ok(())
}

fn render(args: &[String]) -> Result<(), CliError> {
    let [path] = args else {
        return Err(CliError::Usage);
    };
    let report: RunReport = serde_json::from_slice(&fs::read(path)?).map_err(io::Error::other)?;
    print_text(&render_markdown(&report))
}

fn facts(args: &[String]) -> Result<(), CliError> {
    let [path] = args else {
        return Err(CliError::Usage);
    };
    let found = fsinfo::probe(std::path::Path::new(path))?;
    print_text(&format!("{path}: {}\n", found.describe()))
}

fn print_text(text: &str) -> Result<(), CliError> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(text.as_bytes())?;
    stdout.flush()?;
    Ok(())
}

fn wait_for_go() -> io::Result<bool> {
    println!("ready");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim() == "go")
}

fn print_json(value: &impl serde::Serialize) -> Result<(), CliError> {
    print_text(&format!(
        "{}\n",
        serde_json::to_string(value).map_err(io::Error::other)?
    ))
}

fn writer(flags: &Flags) -> Result<(), CliError> {
    let plan = WriterPlan {
        candidate: flags.candidate()?,
        dir: PathBuf::from(flags.text("--dir")?),
        writer: flags.number("--writer", None)?,
        records: flags.number("--records", None)?,
        size: flags.number("--size", None)?,
        pace_us: flags.number("--pace-us", Some(0))?,
    };
    if !wait_for_go()? {
        return Ok(());
    }
    print_json(&run_writer(&plan))
}

fn reader(flags: &Flags) -> Result<(), CliError> {
    let plan = ReaderPlan {
        candidate: flags.candidate()?,
        dir: PathBuf::from(flags.text("--dir")?),
        size: flags.number("--size", None)?,
        interval_ms: flags.number("--interval-ms", Some(20))?,
        stop_file: PathBuf::from(flags.text("--stop-file")?),
    };
    if !wait_for_go()? {
        return Ok(());
    }
    print_json(&run_reader(&plan))
}

fn rotator(flags: &Flags) -> Result<(), CliError> {
    let plan = RotatorPlan {
        candidate: flags.candidate()?,
        dir: PathBuf::from(flags.text("--dir")?),
        period_ms: flags.number("--period-ms", Some(10))?,
        grace_ms: flags.number("--grace-ms", Some(0))?,
        budget_ms: flags.number("--budget-ms", Some(50))?,
        final_budget_ms: flags.number("--final-budget-ms", Some(10_000))?,
        stop_file: PathBuf::from(flags.text("--stop-file")?),
    };
    if !wait_for_go()? {
        return Ok(());
    }
    print_json(&run_rotator(&plan))
}
