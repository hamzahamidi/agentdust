#![cfg(target_os = "macos")]

use std::collections::BTreeSet;
use std::io;
use std::thread;
use std::time::Duration;

use agentdust_core::class::Class;
use agentdust_core::classifier::{Evidence, Finding, Policy, Provenance, classify};
use agentdust_core::darwin::DarwinSource;
use agentdust_core::identity::KernelIdentity;
use agentdust_core::inventory::{
    Clock, Cpu, IDLE_SAMPLE_GAP, LaunchdSource, ProcessSource, RawIdentity, RawProcess, SystemClock, Tag,
    take,
};
use agentdust_core::journal::{Agent, AgentIdentity, Kind, Record, SCHEMA_VERSION};
use agentdust_core::secret::Secret;
use agentdust_core::session::{ProviderLiveness, Scope, scopes};
use agentdust_core::tag::SessionTag;
use agentdust_testkit::harness::{Harness, ProcHandle, Signal};
use agentdust_testkit::spec::ProcSpec;

const FIXTURE: &str = env!("CARGO_BIN_EXE_fixture-sleeper");
const MINUTE_US: u64 = 60_000_000;

struct Extended<'a> {
    live: DarwinSource<'a>,
    extra: Vec<RawProcess>,
}

impl ProcessSource for Extended<'_> {
    fn scan(&self) -> io::Result<Vec<RawProcess>> {
        let mut processes = self.live.scan()?;
        processes.extend(self.extra.clone());
        Ok(processes)
    }

    fn cpu_time_ns(&self, identity: &KernelIdentity) -> io::Result<Option<u64>> {
        self.live.cpu_time_ns(identity)
    }
}

struct Listed(BTreeSet<i32>);

impl LaunchdSource for Listed {
    fn pids(&self) -> io::Result<BTreeSet<i32>> {
        Ok(self.0.clone())
    }
}

struct Later {
    offset_us: u64,
}

impl Clock for Later {
    fn now_us(&self) -> u64 {
        SystemClock.now_us() + self.offset_us
    }

    fn sleep(&self, _requested: Duration) {
        thread::sleep(Duration::from_millis(150));
    }
}

fn scripted(pid: i32, uid: u32, exe: &str, ppid: i32, start_time_us: u64) -> RawProcess {
    RawProcess {
        identity: RawIdentity {
            kernel: KernelIdentity {
                boot_session_uuid: "scripted-boot".to_owned(),
                pid,
                start_time_us,
                uid,
            },
            exe_path: Some(exe.into()),
            ppid,
            pgid: pid,
        },
        tag: Tag::Absent,
        agent_script: false,
        cpu: Cpu {
            first_ns: Some(7),
            later_ns: None,
        },
    }
}

fn my_uid() -> u32 {
    // SAFETY: geteuid takes no arguments and cannot fail.
    unsafe { libc::geteuid() }
}

fn record(harness: &Harness, agent: ProcHandle, session: &str, tag: &SessionTag, secret: &Secret) -> Record {
    let identity = harness.identity(agent);
    Record {
        v: SCHEMA_VERSION,
        kind: Kind::SessionStart,
        agent: Agent::Claude,
        session_id: session.to_owned(),
        subagent_id: None,
        agent_identity: Some(AgentIdentity::from_process(identity).unwrap()),
        tool_use_id: None,
        wall_ts: 1,
        mono_ts: 1,
        boot: identity.kernel.boot_session_uuid.clone(),
        session_tag_key: Some(tag.key(secret)),
        cwd_key: None,
        exe_base: None,
    }
}

struct Scenario {
    harness: Harness,
    secret: Secret,
    records: Vec<Record>,
    ended_children: Vec<ProcHandle>,
    live_parent: ProcHandle,
    live_child: ProcHandle,
    tunnel: ProcHandle,
    stubborn: ProcHandle,
    bystander: ProcHandle,
    service: ProcHandle,
    parents: Vec<i32>,
}

fn spec() -> ProcSpec {
    ProcSpec::new().seconds(120)
}

fn scenario() -> Scenario {
    let mut harness = Harness::new(FIXTURE).unwrap();
    let secret = Secret::from_bytes([1; 32]);
    let ended_tag = SessionTag::from_bytes([0xa1; 16]);
    let live_tag = SessionTag::from_bytes([0xb2; 16]);
    let ended = harness
        .spawn_tree(&spec().spawn(2).env("AGENTDUST_SESSION", ended_tag.as_str()))
        .unwrap();
    let live = harness
        .spawn_tree(&spec().spawn(1).env("AGENTDUST_SESSION", live_tag.as_str()))
        .unwrap();
    let tunnel = harness.spawn_tree(&spec().spawn(1).setsid()).unwrap();
    let stubborn = harness
        .spawn_tree(&spec().spawn(1).setsid().ignore_term())
        .unwrap();
    let bystander = harness.spawn(&spec()).unwrap();
    let service = harness.spawn(&spec()).unwrap();
    let records = vec![
        record(&harness, ended.parent, "ended", &ended_tag, &secret),
        record(&harness, live.parent, "live", &live_tag, &secret),
    ];
    let parents = vec![
        harness.pid(ended.parent),
        harness.pid(tunnel.parent),
        harness.pid(stubborn.parent),
    ];
    for tree in [&ended, &tunnel, &stubborn] {
        harness.orphan(tree.parent).unwrap();
    }
    Scenario {
        harness,
        secret,
        records,
        ended_children: ended.children,
        live_parent: live.parent,
        live_child: live.children[0],
        tunnel: tunnel.children[0],
        stubborn: stubborn.children[0],
        bystander,
        service,
        parents,
    }
}

fn run(scenario: &Scenario, secret: Option<&Secret>, provenance: Option<&[Scope]>) -> Vec<Finding> {
    let source = Extended {
        live: DarwinSource::new(secret).unwrap(),
        extra: vec![
            scripted(1, 0, "/sbin/launchd", 0, 1),
            scripted(2_000_001, my_uid() + 1, "/opt/homebrew/bin/node", 1, 1),
            scripted(2_000_002, my_uid(), "/usr/sbin/cfprefsd", 1, 1),
            scripted(
                2_000_003,
                my_uid(),
                "/Applications/Some.app/Contents/MacOS/Some",
                1,
                1,
            ),
        ],
    };
    let launchd = Listed(BTreeSet::from([scenario.harness.pid(scenario.service)]));
    let clock = Later {
        offset_us: 31 * MINUTE_US,
    };
    let snapshot = take(&source, &launchd, &clock, IDLE_SAMPLE_GAP).unwrap();
    let policy = Policy::new(my_uid(), std::process::id() as i32);
    match provenance {
        Some(scopes) => classify(&snapshot, &Provenance::Available(scopes), &policy),
        None => classify(&snapshot, &Provenance::Unavailable, &policy),
    }
}

fn class_of(findings: &[Finding], pid: i32) -> Class {
    findings
        .iter()
        .find(|finding| finding.identity.kernel.pid == pid)
        .unwrap_or_else(|| panic!("no finding for pid {pid}"))
        .class
}

fn evidence_of(findings: &[Finding], pid: i32) -> Vec<Evidence> {
    findings
        .iter()
        .find(|finding| finding.identity.kernel.pid == pid)
        .unwrap()
        .evidence
        .clone()
}

fn session_scopes(scenario: &Scenario) -> Vec<Scope> {
    scopes(&scenario.records, &ProviderLiveness(scenario.harness.provider()))
}

#[test]
fn live_processes_are_classified_by_the_rules() {
    let scenario = scenario();
    let scopes = session_scopes(&scenario);
    let found = run(&scenario, Some(&scenario.secret), Some(&scopes));
    let pid = |handle: ProcHandle| scenario.harness.pid(handle);

    for child in &scenario.ended_children {
        assert_eq!(class_of(&found, pid(*child)), Class::OwnedEnded);
        assert_eq!(
            evidence_of(&found, pid(*child)),
            [Evidence::OwnedTag, Evidence::OwnedAgentGone]
        );
    }
    assert_eq!(class_of(&found, pid(scenario.live_parent)), Class::Managed);
    assert_eq!(
        evidence_of(&found, pid(scenario.live_parent)),
        [Evidence::ManagedAgent]
    );
    assert_eq!(class_of(&found, pid(scenario.live_child)), Class::OwnedLive);
    for detached in [scenario.tunnel, scenario.stubborn] {
        assert_eq!(class_of(&found, pid(detached)), Class::Suspect);
        assert_eq!(
            evidence_of(&found, pid(detached)),
            [
                Evidence::SuspectParentLaunchd,
                Evidence::SuspectSameUser,
                Evidence::SuspectAge,
                Evidence::SuspectIdle
            ]
        );
    }
    assert_eq!(class_of(&found, pid(scenario.bystander)), Class::Unknown);
    assert_eq!(class_of(&found, pid(scenario.service)), Class::Managed);
    assert_eq!(
        evidence_of(&found, pid(scenario.service)),
        [Evidence::ManagedLaunchd]
    );
    for scripted in [1, 2_000_001, 2_000_002, 2_000_003] {
        assert_eq!(class_of(&found, scripted), Class::Managed, "{scripted}");
    }
    assert_eq!(class_of(&found, std::process::id() as i32), Class::Managed);
}

#[test]
fn classification_signals_nothing_and_the_children_stay_alive() {
    let scenario = scenario();
    let scopes = session_scopes(&scenario);
    run(&scenario, Some(&scenario.secret), Some(&scopes));
    let log = scenario.harness.signal_log();
    assert_eq!(log.len(), scenario.parents.len());
    assert!(log.iter().all(|event| event.signal == Signal::Kill));
    assert!(log.iter().all(|event| scenario.parents.contains(&event.pid)));
    for child in
        scenario
            .ended_children
            .iter()
            .chain([&scenario.live_child, &scenario.tunnel, &scenario.stubborn])
    {
        assert_eq!(
            scenario.harness.revalidate(*child),
            agentdust_core::revalidate::Revalidation::Match
        );
    }
}

#[test]
fn a_lost_secret_leaves_the_tags_unverifiable_and_owned_classes_unavailable() {
    let scenario = scenario();
    let scopes = session_scopes(&scenario);
    let found = run(&scenario, None, Some(&scopes));
    let pid = |handle: ProcHandle| scenario.harness.pid(handle);
    for child in &scenario.ended_children {
        assert_eq!(class_of(&found, pid(*child)), Class::Unknown);
        assert_eq!(evidence_of(&found, pid(*child)), [Evidence::TagUnverifiable]);
    }
    assert_eq!(class_of(&found, pid(scenario.live_child)), Class::Unknown);
    assert_eq!(class_of(&found, pid(scenario.tunnel)), Class::Suspect);
}

#[test]
fn a_lost_journal_leaves_the_tags_unmatched_and_never_makes_a_tagged_process_a_suspect() {
    let scenario = scenario();
    let found = run(&scenario, Some(&scenario.secret), Some(&[]));
    let pid = |handle: ProcHandle| scenario.harness.pid(handle);
    for child in &scenario.ended_children {
        assert_eq!(class_of(&found, pid(*child)), Class::Unknown);
        assert_eq!(evidence_of(&found, pid(*child)), [Evidence::TagUnmatched]);
    }
    assert_eq!(class_of(&found, pid(scenario.live_child)), Class::Unknown);
}

#[test]
fn an_unsupported_journal_leaves_owned_classes_unavailable() {
    let scenario = scenario();
    let found = run(&scenario, Some(&scenario.secret), None);
    let pid = |handle: ProcHandle| scenario.harness.pid(handle);
    for child in &scenario.ended_children {
        assert_eq!(class_of(&found, pid(*child)), Class::Unknown);
    }
    assert_eq!(class_of(&found, pid(scenario.tunnel)), Class::Suspect);
}

#[test]
fn a_young_detached_process_is_not_a_suspect() {
    let scenario = scenario();
    let scopes = session_scopes(&scenario);
    let source = DarwinSource::new(Some(&scenario.secret)).unwrap();
    let launchd = Listed(BTreeSet::new());
    let snapshot = take(&source, &launchd, &SystemClock, Duration::from_millis(150)).unwrap();
    let policy = Policy::new(my_uid(), std::process::id() as i32);
    let found = classify(&snapshot, &Provenance::Available(&scopes), &policy);
    assert_eq!(
        class_of(&found, scenario.harness.pid(scenario.tunnel)),
        Class::Unknown
    );
    assert_eq!(
        class_of(&found, scenario.harness.pid(scenario.stubborn)),
        Class::Unknown
    );
}
