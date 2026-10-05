#![cfg(target_os = "macos")]

use std::collections::{BTreeSet, HashMap};
use std::io;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use agentdust_core::class::Class;
use agentdust_core::classifier::{Evidence, Policy, Provenance, classify};
use agentdust_core::darwin::{self, DarwinSource};
use agentdust_core::identity::KernelIdentity;
use agentdust_core::inventory::{
    Clock, Cpu, IDLE_SAMPLE_GAP, LaunchdSource, ProcessSource, RawIdentity, RawProcess, SystemClock, Tag,
    take,
};
use agentdust_core::journal::{Agent, AgentIdentity, Kind, Record, SCHEMA_VERSION};
use agentdust_core::revalidate::Revalidation;
use agentdust_core::secret::Secret;
use agentdust_core::session::{ProviderLiveness, Scope, scopes};
use agentdust_core::tag::SessionTag;
use agentdust_testkit::fixture::{Fixture, Label, Plan, load_dir, materialise_plan};

const FIXTURE_BIN: &str = env!("CARGO_BIN_EXE_fixture-sleeper");
const MINUTE_US: u64 = 60_000_000;
const SCRIPTED_BOOT: &str = "scripted-boot";
const THRESHOLD_MINUTES: u64 = 30;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/m1")
}

fn corpus() -> Vec<Fixture> {
    match load_dir(&corpus_dir()) {
        Ok(loaded) => loaded.into_iter().map(|loaded| loaded.fixture).collect(),
        Err(problems) => {
            let lines: Vec<String> = problems.iter().map(ToString::to_string).collect();
            panic!("the corpus does not load:\n{}", lines.join("\n"));
        }
    }
}

fn my_uid() -> u32 {
    darwin::current_uid()
}

struct Recorded {
    process: RawProcess,
    in_launchd: bool,
}

fn scripted(pid: i32, uid: u32, exe: &str, ppid: i32) -> RawProcess {
    RawProcess {
        identity: RawIdentity {
            kernel: KernelIdentity {
                boot_session_uuid: SCRIPTED_BOOT.to_owned(),
                pid,
                start_time_us: 1,
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

fn recorded(fixture: &str, role: &str) -> Recorded {
    let me = my_uid();
    let (process, in_launchd) = match (fixture, role) {
        ("system_process_pid_one", "launchd") => (scripted(1, 0, "/sbin/launchd", 0), false),
        ("other_user_process", "other_user_daemon") => (
            scripted(4_000_001, me + 1, "/opt/homebrew/bin/postgres", 1),
            false,
        ),
        ("launchd_job_lookalike", "launchd_job") => (scripted(4_000_002, me, "/Users/dev/bin/job", 1), true),
        ("launchd_job_lookalike", "job_helper") => (
            scripted(4_000_003, me, "/Users/dev/bin/job-helper", 4_000_002),
            false,
        ),
        ("homebrew_service_lookalike", "brew_service") => (
            scripted(4_000_004, me, "/opt/homebrew/opt/postgresql@16/bin/postgres", 1),
            true,
        ),
        ("app_helper_process", "app_main") => (
            scripted(4_000_005, me, "/Applications/Some.app/Contents/MacOS/Some", 1),
            false,
        ),
        ("app_helper_process", "app_helper") => (
            scripted(
                4_000_006,
                me,
                "/Applications/Some.app/Contents/Frameworks/Some Helper.app/Contents/MacOS/Some Helper",
                4_000_005,
            ),
            false,
        ),
        other => panic!("record-only role {other:?} has no recorded attributes"),
    };
    Recorded { process, in_launchd }
}

struct Extended<'a> {
    live: DarwinSource<'a>,
    extra: Vec<RawProcess>,
    launchd: Vec<i32>,
}

impl ProcessSource for Extended<'_> {
    fn scan(&self) -> io::Result<Vec<RawProcess>> {
        let mut processes = self.live.scan()?;
        processes.extend(self.extra.clone());
        Ok(processes)
    }

    fn cpu_time_ns(&self, identity: &KernelIdentity) -> io::Result<Option<u64>> {
        if identity.boot_session_uuid == SCRIPTED_BOOT {
            return Ok(Some(7));
        }
        self.live.cpu_time_ns(identity)
    }
}

impl LaunchdSource for Extended<'_> {
    fn pids(&self) -> io::Result<std::collections::BTreeSet<i32>> {
        Ok(self.launchd.iter().copied().collect())
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

#[derive(Debug, Clone)]
struct Outcome {
    fixture: String,
    role: String,
    label: Label,
    class: Class,
    evidence: Vec<Evidence>,
}

impl Outcome {
    fn describe(&self) -> String {
        format!(
            "{} / {} ({}): {} {:?}",
            self.fixture, self.role, self.label, self.class, self.evidence
        )
    }
}

fn tag_for(number: usize) -> SessionTag {
    SessionTag::from_bytes([number as u8; 16])
}

fn group_of(plan: &Plan, role: &str) -> usize {
    plan.groups
        .iter()
        .position(|group| group.parent.role == role || group.children.iter().any(|child| child.role == role))
        .unwrap_or_else(|| panic!("sampled role {role} is not started by the plan"))
}

fn tag_groups(fixture: &Fixture, plan: &Plan) -> HashMap<usize, usize> {
    let mut starts = 0;
    let mut by_group: HashMap<usize, usize> = HashMap::new();
    let mut sampled: HashMap<usize, BTreeSet<String>> = HashMap::new();
    for event in &fixture.journal {
        match event.kind {
            Kind::SessionStart => starts += 1,
            Kind::Sample => {
                for role in &event.roles {
                    let group = group_of(plan, role);
                    by_group.insert(group, starts);
                    sampled.entry(group).or_default().insert(role.clone());
                }
            }
            _ => {}
        }
    }
    for (group, roles) in &sampled {
        let children: BTreeSet<String> = plan.groups[*group]
            .children
            .iter()
            .map(|child| child.role.clone())
            .collect();
        assert_eq!(
            *roles, children,
            "{}: a sample names only some children of a group",
            fixture.name
        );
    }
    by_group
}

struct Prepared<'a> {
    fixture: &'a Fixture,
    materialised: agentdust_testkit::fixture::Materialised,
    scopes: Vec<Scope>,
    secret: Secret,
}

fn prepare(fixture: &Fixture) -> Prepared<'_> {
    let mut plan = fixture.plan().unwrap();
    let secret = Secret::from_bytes([3; 32]);
    for (group, number) in tag_groups(fixture, &plan) {
        let spec = plan.groups[group].parent.spec.clone();
        plan.groups[group].parent.spec = spec.env("AGENTDUST_SESSION", tag_for(number).as_str());
    }
    let agents: Vec<usize> = plan
        .groups
        .iter()
        .enumerate()
        .filter(|(_, group)| group.parent.label == Label::TrueManaged)
        .map(|(index, _)| index)
        .collect();
    let materialised =
        materialise_plan(&plan, FIXTURE_BIN).unwrap_or_else(|err| panic!("{}: {err}", fixture.name));
    let boot = darwin::boot_session_uuid().unwrap();
    let mut records = Vec::new();
    let mut starts = 0;
    let mut identity: Option<AgentIdentity> = None;
    for (index, event) in fixture.journal.iter().enumerate() {
        let mut key = None;
        if event.kind == Kind::SessionStart {
            starts += 1;
            identity = agents
                .get(starts - 1)
                .or(agents.last())
                .and_then(|group| materialised.handle(&plan.groups[*group].parent.role))
                .map(|handle| AgentIdentity::from_process(materialised.harness().identity(handle)).unwrap());
            key = Some(tag_for(starts).key(&secret));
        }
        records.push(Record {
            v: SCHEMA_VERSION,
            kind: event.kind,
            agent: Agent::Claude,
            session_id: event.session_id.clone(),
            subagent_id: event.subagent_id.clone(),
            agent_identity: identity.clone(),
            tool_use_id: event.tool_use_id.clone(),
            wall_ts: index as u64 + 1,
            mono_ts: index as u64 + 1,
            boot: boot.clone(),
            session_tag_key: key,
            cwd_key: None,
            exe_base: event.exe_base.clone(),
        });
    }
    let scopes = scopes(&records, &ProviderLiveness(materialised.harness().provider()));
    Prepared {
        fixture,
        materialised,
        scopes,
        secret,
    }
}

impl Prepared<'_> {
    fn classify_at(&self, age_minutes: u64) -> Vec<Outcome> {
        let record_only: Vec<(String, Recorded)> = self
            .fixture
            .processes
            .iter()
            .filter(|process| process.record_only)
            .map(|process| (process.role.clone(), recorded(&self.fixture.name, &process.role)))
            .collect();
        let source = Extended {
            live: DarwinSource::new(Some(&self.secret)).unwrap(),
            extra: record_only
                .iter()
                .map(|(_, recorded)| recorded.process.clone())
                .collect(),
            launchd: record_only
                .iter()
                .filter(|(_, recorded)| recorded.in_launchd)
                .map(|(_, recorded)| recorded.process.identity.kernel.pid)
                .collect(),
        };
        let clock = Later {
            offset_us: age_minutes * MINUTE_US,
        };
        let snapshot = take(&source, &source, &clock, IDLE_SAMPLE_GAP).unwrap();
        let policy = Policy::new(my_uid(), std::process::id() as i32);
        let found = classify(&snapshot, &Provenance::Available(&self.scopes), &policy);
        let mut outcomes = Vec::new();
        for process in self.fixture.observed() {
            let pid = if process.record_only {
                recorded(&self.fixture.name, &process.role)
                    .process
                    .identity
                    .kernel
                    .pid
            } else {
                let handle = self
                    .materialised
                    .handle(&process.role)
                    .unwrap_or_else(|| panic!("{}: {} was not started", self.fixture.name, process.role));
                if self.materialised.harness().revalidate(handle) == Revalidation::Gone {
                    continue;
                }
                self.materialised.harness().pid(handle)
            };
            let finding = found
                .iter()
                .find(|finding| finding.identity.kernel.pid == pid)
                .unwrap_or_else(|| panic!("{}: no finding for {}", self.fixture.name, process.role));
            outcomes.push(Outcome {
                fixture: self.fixture.name.clone(),
                role: process.role.clone(),
                label: process.expected.label,
                class: finding.class,
                evidence: finding.evidence.clone(),
            });
        }
        outcomes
    }
}

fn corpus_outcomes() -> &'static Vec<Outcome> {
    static OUTCOMES: OnceLock<Vec<Outcome>> = OnceLock::new();
    OUTCOMES.get_or_init(|| {
        corpus()
            .iter()
            .flat_map(|fixture| prepare(fixture).classify_at(THRESHOLD_MINUTES + 1))
            .collect()
    })
}

fn actionable(class: Class) -> bool {
    matches!(class, Class::OwnedEnded | Class::Suspect)
}

#[test]
fn every_observed_process_is_classified_as_labelled() {
    let wrong: Vec<String> = corpus_outcomes()
        .iter()
        .filter(|outcome| outcome.class != outcome.label.class())
        .map(Outcome::describe)
        .collect();
    assert!(
        wrong.is_empty(),
        "classified against the label:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn no_process_outside_true_owned_ended_is_classified_owned_ended() {
    let wrong: Vec<String> = corpus_outcomes()
        .iter()
        .filter(|outcome| outcome.label != Label::TrueOwnedEnded && outcome.class == Class::OwnedEnded)
        .map(Outcome::describe)
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn nothing_that_must_never_be_signalled_is_actionable() {
    let wrong: Vec<String> = corpus_outcomes()
        .iter()
        .filter(|outcome| outcome.label.must_never_signal() && actionable(outcome.class))
        .map(Outcome::describe)
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn every_true_owned_ended_and_true_detached_process_is_found() {
    for (label, class) in [
        (Label::TrueOwnedEnded, Class::OwnedEnded),
        (Label::TrueDetached, Class::Suspect),
    ] {
        let seen: Vec<&Outcome> = corpus_outcomes().iter().filter(|o| o.label == label).collect();
        assert!(!seen.is_empty());
        let missed: Vec<String> = seen
            .iter()
            .filter(|o| o.class != class)
            .map(|o| o.describe())
            .collect();
        assert!(missed.is_empty(), "{}", missed.join("\n"));
    }
}

#[test]
fn the_corpus_is_large_enough_for_the_claims_not_to_be_vacuous() {
    let mut counts: HashMap<Label, usize> = HashMap::new();
    for outcome in corpus_outcomes() {
        *counts.entry(outcome.label).or_default() += 1;
    }
    let at_least = |label: Label, minimum: usize| {
        let found = counts.get(&label).copied().unwrap_or(0);
        assert!(
            found >= minimum,
            "{label}: {found} observed, expected at least {minimum}"
        );
    };
    at_least(Label::TrueOwnedEnded, 9);
    at_least(Label::TrueLiveOwned, 4);
    at_least(Label::TrueDetached, 2);
    at_least(Label::TrueManaged, 12);
    at_least(Label::TrueUnknown, 4);
}

#[test]
fn a_launchd_helper_and_an_app_helper_are_managed_by_their_own_evidence() {
    let find = |fixture: &str, role: &str| {
        corpus_outcomes()
            .iter()
            .find(|o| o.fixture == fixture && o.role == role)
            .unwrap_or_else(|| panic!("{fixture} {role}"))
    };
    assert_eq!(
        find("launchd_job_lookalike", "launchd_job").evidence,
        [Evidence::ManagedLaunchd]
    );
    assert_eq!(
        find("launchd_job_lookalike", "job_helper").evidence,
        [Evidence::ManagedLaunchdChild]
    );
    assert_eq!(
        find("homebrew_service_lookalike", "brew_service").evidence,
        [Evidence::ManagedLaunchd]
    );
    assert_eq!(
        find("app_helper_process", "app_helper").evidence,
        [Evidence::ManagedSystemPath]
    );
    assert_eq!(
        find("other_user_process", "other_user_daemon").evidence,
        [Evidence::ManagedOtherUser]
    );
}

#[test]
fn the_suspect_class_follows_the_age_threshold_and_never_reaches_a_protected_process() {
    let names = [
        "detached_tunnel",
        "detached_ignores_sigterm",
        "unrelated_process_with_live_launcher",
        "unrelated_process_no_evidence",
        "live_session_background_server",
        "launchd_job_lookalike",
        "homebrew_service_lookalike",
    ];
    let all = corpus();
    for name in names {
        let fixture = all.iter().find(|fixture| fixture.name == name).unwrap();
        let prepared = prepare(fixture);
        for minutes in [0, 10, 20, THRESHOLD_MINUTES - 1, THRESHOLD_MINUTES + 1, 90, 600] {
            for outcome in prepared.classify_at(minutes) {
                let should_be_suspect = outcome.label == Label::TrueDetached && minutes >= THRESHOLD_MINUTES;
                assert_eq!(
                    outcome.class == Class::Suspect,
                    should_be_suspect,
                    "age {minutes} min: {}",
                    outcome.describe()
                );
            }
        }
    }
}
