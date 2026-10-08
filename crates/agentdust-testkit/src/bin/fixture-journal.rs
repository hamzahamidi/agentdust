use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use agentdust_core::clock;
use agentdust_core::journal::retention::Policy;
use agentdust_core::journal::{
    self, Agent, FrameWriter, Journal, Kind, MaintenanceError, Record, SCHEMA_VERSION,
};

const BOOT: &str = "stress-boot";
const MARKER_BOOT: &str = "marker-boot";
const HANG_GUARD: Duration = Duration::from_secs(60);
const HELD: &str = "held";
const RELEASE: &str = "release";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["write", dir, writer, count, pace_us] => {
            write(Path::new(dir), number(writer), number(count), number(pace_us));
        }
        ["straggle", dir, sync, rounds] => straggle(Path::new(dir), Path::new(sync), number(rounds)),
        ["rotate", dir, sync, stop, period_ms] => {
            rotate(
                Path::new(dir),
                Path::new(sync),
                Path::new(stop),
                number(period_ms),
            );
        }
        _ => return ExitCode::from(2),
    }
    ExitCode::SUCCESS
}

fn number(text: &str) -> u64 {
    text.parse().unwrap_or(0)
}

fn record(id: &str, boot: &str) -> Record {
    Record {
        v: SCHEMA_VERSION,
        kind: Kind::ShellStart,
        agent: Agent::Claude,
        session_id: id.to_owned(),
        subagent_id: None,
        tool_use_id: None,
        wall_ts: clock::wall_ms(),
        mono_ts: clock::monotonic_ns(),
        boot: boot.to_owned(),
        cwd_key: None,
        agent_identity: None,
        session_tag_key: None,
        exe_base: None,
    }
}

fn csv(ids: &[String]) -> String {
    ids.join(",")
}

fn write(dir: &Path, writer: u64, count: u64, pace_us: u64) {
    let (mut attempted, mut retried) = (0, 0);
    let mut failed = Vec::new();
    for seq in 0..count {
        let id = format!("w{writer}-{seq}");
        match journal::append(dir, &record(&id, BOOT)) {
            Ok(appended) => retried += u64::from(appended.attempts > 1),
            Err(_) => failed.push(id),
        }
        attempted += 1;
        if pace_us > 0 {
            thread::sleep(Duration::from_micros(pace_us));
        }
    }
    println!("attempted={attempted} retried={retried} failed={}", csv(&failed));
}

struct Holding {
    sync: PathBuf,
    waiting: bool,
    round: u64,
}

impl FrameWriter for Holding {
    fn write(&mut self, mut file: &File, frame: &[u8]) -> io::Result<usize> {
        if self.waiting {
            self.waiting = false;
            fs::write(self.sync.join(format!("{HELD}-{}", self.round)), b"")?;
            let started = Instant::now();
            while !self.sync.join(format!("{RELEASE}-{}", self.round)).exists() {
                if started.elapsed() >= HANG_GUARD {
                    std::process::exit(3);
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
        file.write(frame)
    }
}

fn straggle(dir: &Path, sync: &Path, rounds: u64) {
    let mut failed = Vec::new();
    let mut fewest_attempts = u32::MAX;
    for round in 0..rounds {
        let id = format!("s-{round}");
        let mut writer = Holding {
            sync: sync.to_path_buf(),
            waiting: true,
            round,
        };
        match Journal::new(dir).append_with(&record(&id, BOOT), &mut writer) {
            Ok(appended) => fewest_attempts = fewest_attempts.min(appended.attempts),
            Err(_) => failed.push(id),
        }
        let _ = fs::remove_file(sync.join(format!("{HELD}-{round}")));
        let _ = fs::remove_file(sync.join(format!("{RELEASE}-{round}")));
    }
    println!(
        "attempted={rounds} fewest_attempts={} failed={}",
        if fewest_attempts == u32::MAX {
            0
        } else {
            fewest_attempts
        },
        csv(&failed)
    );
}

fn rotate(dir: &Path, sync: &Path, stop: &Path, period_ms: u64) {
    let policy = Policy {
        max_bytes: u64::MAX,
        ..Policy::default()
    };
    let started = Instant::now();
    let mut totals = Totals::default();
    let mut released = BTreeSet::new();
    loop {
        let stopping = stop.exists();
        let holding = fs::read_dir(sync)
            .unwrap()
            .filter_map(|entry| {
                entry
                    .ok()?
                    .file_name()
                    .to_str()?
                    .strip_prefix(&format!("{HELD}-"))?
                    .parse::<u64>()
                    .ok()
            })
            .find(|round| !released.contains(round));
        totals.cycles += 1;
        let marker = record(&format!("marker-{}", totals.cycles), MARKER_BOOT);
        if journal::append(dir, &marker).is_ok() {
            totals.markers += 1;
        }
        match journal::prune(dir, &policy, clock::wall_ms(), BOOT) {
            Ok(report) => {
                totals.rotations += u64::from(matches!(report.rotation, journal::Rotation::Rotated { .. }));
                let changed = report.retained.rewritten + report.retained.deleted;
                totals.changes += changed as u64;
                totals.dropped += report.retained.dropped_earlier_boot as u64;
                totals.other_drops += (report.retained.dropped_aged
                    + report.retained.dropped_over_cap
                    + report.retained.dropped_pinned) as u64;
                if stopping {
                    totals.final_rotated = matches!(report.rotation, journal::Rotation::Rotated { .. });
                    totals.final_changed = changed as u64;
                }
            }
            Err(MaintenanceError::Busy) => totals.busy += 1,
            Err(_) => totals.errors += 1,
        }
        if let Some(round) = holding {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(sync.join(format!("{RELEASE}-{round}")))
                .unwrap();
            released.insert(round);
            totals.releases += 1;
        }
        if stopping {
            break;
        }
        if started.elapsed() >= HANG_GUARD {
            std::process::exit(3);
        }
        thread::sleep(Duration::from_millis(period_ms));
    }
    println!(
        "cycles={} rotations={} changes={} busy={} errors={} markers={} dropped={} other_drops={} releases={} final_rotated={} final_changed={}",
        totals.cycles,
        totals.rotations,
        totals.changes,
        totals.busy,
        totals.errors,
        totals.markers,
        totals.dropped,
        totals.other_drops,
        totals.releases,
        u8::from(totals.final_rotated),
        totals.final_changed
    );
}

#[derive(Default)]
struct Totals {
    cycles: u64,
    rotations: u64,
    changes: u64,
    busy: u64,
    errors: u64,
    markers: u64,
    dropped: u64,
    other_drops: u64,
    releases: u64,
    final_rotated: bool,
    final_changed: u64,
}
