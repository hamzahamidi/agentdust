mod classifier_support;
mod inventory_support;
mod journal_support;
mod scratch;
mod session_support;

use std::path::PathBuf;

use agentdust_core::classifier::Policy;
use agentdust_core::cwd::RelationContext;
use agentdust_core::doctor::{Components, Unavailable, run};
use agentdust_core::identity::KernelIdentity;
use agentdust_core::inventory::LiveDetails;
use agentdust_core::journal::Kind;
use agentdust_core::journal::volume::FixedVolume;
use agentdust_core::session::{Liveness, LivenessProbe};
use inventory_support::{
    Edit, Log, NOW, SELF_PID, ScriptedClock, ScriptedLaunchd, ScriptedSource, UID, log, raw,
};
use journal_support::journal;
use scratch::TempDir;
use session_support::{Edit as _, rec};

struct LoggedProbe(Log);

impl LivenessProbe for LoggedProbe {
    fn probe(&self, identity: &KernelIdentity) -> Liveness {
        self.0.borrow_mut().push(format!("probe {}", identity.pid));
        Liveness::Gone
    }
}

struct NoLive;

impl LiveDetails for NoLive {
    fn command(&self, _: &KernelIdentity) -> Option<Vec<Vec<u8>>> {
        None
    }

    fn cwd(&self, _: &KernelIdentity) -> Option<PathBuf> {
        None
    }
}

fn nowhere() -> RelationContext {
    RelationContext {
        reference: None,
        home: None,
        temp_roots: Vec::new(),
    }
}

#[test]
fn the_run_scans_waits_then_reads_the_journal_and_probes_the_agents() {
    let dir = TempDir::private("doctor-run-order");
    journal(&dir)
        .append(&rec(Kind::SessionStart).by(10).tagged(1))
        .unwrap();
    let log = log();
    let source = ScriptedSource::new(
        &log,
        vec![raw(SELF_PID).ppid(8000), raw(8000).ppid(1), raw(300).tagged(1)],
    );
    let launchd = ScriptedLaunchd::new(&log, &[]);
    let clock = ScriptedClock::new(&log, NOW);
    let probe = LoggedProbe(log.clone());
    let volume = FixedVolume::apfs_local();
    let diagnosis = run(&Components {
        data_dir: &dir,
        volume: &volume,
        secret_available: true,
        processes: &source,
        launchd: &launchd,
        clock: &clock,
        liveness: &probe,
        live: &NoLive,
        relation: &nowhere(),
        policy: Policy::new(UID, SELF_PID),
    })
    .unwrap();
    let events = log.borrow().clone();
    let first_probe = events.iter().position(|e| e.starts_with("probe")).unwrap();
    let last_clock = events.iter().rposition(|e| e == "now").unwrap();
    assert_eq!(events[0], "scan");
    assert_eq!(events[1], "sleep 2000ms");
    assert!(last_clock < first_probe, "{events:?}");
    assert_eq!(diagnosis.counts.owned_ended, 1);
}

#[test]
fn a_failed_scan_is_an_error_and_nothing_is_read_from_the_journal() {
    let dir = TempDir::private("doctor-run-scan");
    journal(&dir)
        .append(&rec(Kind::SessionStart).by(10).tagged(1))
        .unwrap();
    let log = log();
    let source = ScriptedSource::new(&log, vec![raw(300)]).failing();
    let launchd = ScriptedLaunchd::new(&log, &[]);
    let clock = ScriptedClock::new(&log, NOW);
    let probe = LoggedProbe(log.clone());
    let volume = FixedVolume::apfs_local();
    let result = run(&Components {
        data_dir: &dir,
        volume: &volume,
        secret_available: true,
        processes: &source,
        launchd: &launchd,
        clock: &clock,
        liveness: &probe,
        live: &NoLive,
        relation: &nowhere(),
        policy: Policy::new(UID, SELF_PID),
    });
    assert!(result.is_err());
    assert_eq!(*log.borrow(), ["scan"]);
}

#[test]
fn a_lost_secret_leaves_tagged_processes_out_of_the_owned_classes() {
    let dir = TempDir::private("doctor-run-secret");
    journal(&dir)
        .append(&rec(Kind::SessionStart).by(10).tagged(1))
        .unwrap();
    let log = log();
    let source = ScriptedSource::new(
        &log,
        vec![raw(SELF_PID).ppid(8000), raw(8000).ppid(1), raw(300).tagged(1)],
    );
    let launchd = ScriptedLaunchd::new(&log, &[]);
    let clock = ScriptedClock::new(&log, NOW);
    let probe = LoggedProbe(log.clone());
    let volume = FixedVolume::apfs_local();
    let diagnosis = run(&Components {
        data_dir: &dir,
        volume: &volume,
        secret_available: false,
        processes: &source,
        launchd: &launchd,
        clock: &clock,
        liveness: &probe,
        live: &NoLive,
        relation: &nowhere(),
        policy: Policy::new(UID, SELF_PID),
    })
    .unwrap();
    assert_eq!(diagnosis.owned, Some(Unavailable::SecretUnavailable));
    assert_eq!(diagnosis.counts.owned_ended, 0);
    assert!(diagnosis.findings.is_empty());
}
