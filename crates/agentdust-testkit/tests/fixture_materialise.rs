#![cfg(target_os = "macos")]

use std::path::PathBuf;
use std::time::Duration;

use agentdust_core::darwin::DarwinProvider;
use agentdust_core::revalidate::{Revalidation, revalidate};
use agentdust_testkit::fixture::{
    FIXTURE_SECONDS, Fixture, MaterialiseError, Materialised, ParentExpectation, PlanProblem, load, load_dir,
    materialise, materialise_plan,
};
use agentdust_testkit::harness::Signal;
use agentdust_testkit::wait_until;
use serde_json::json;

const FIXTURE_BIN: &str = env!("CARGO_BIN_EXE_fixture-sleeper");
const HANG_GUARD: Duration = Duration::from_secs(60);
const SETTLE: Duration = Duration::from_millis(400);

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/m1")
}

fn fixture(name: &str) -> Fixture {
    load(&corpus_dir().join(format!("{name}.json"))).unwrap_or_else(|problems| {
        panic!(
            "{}",
            problems
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        )
    })
}

fn start(name: &str) -> Materialised {
    materialise(&fixture(name), FIXTURE_BIN).unwrap_or_else(|err| panic!("{name}: {err}"))
}

fn test_pid() -> i32 {
    std::process::id() as i32
}

fn is_gone(materialised: &Materialised, role: &str) -> bool {
    let handle = materialised.handle(role).unwrap();
    materialised.harness().revalidate(handle) == Revalidation::Gone
}

fn is_alive(materialised: &Materialised, role: &str) -> bool {
    let handle = materialised.handle(role).unwrap();
    materialised.harness().revalidate(handle) == Revalidation::Match
}

fn pid(materialised: &Materialised, role: &str) -> i32 {
    materialised.harness().pid(materialised.handle(role).unwrap())
}

fn ppid(materialised: &Materialised, role: &str) -> i32 {
    materialised
        .harness()
        .ppid(materialised.handle(role).unwrap())
        .unwrap()
}

fn sid(materialised: &Materialised, role: &str) -> i32 {
    materialised
        .harness()
        .report(materialised.handle(role).unwrap())
        .sid
}

fn assert_structure(materialised: &Materialised, name: &str) {
    let violations = materialised.check_structure();
    assert!(violations.is_empty(), "{name}: {violations:?}");
}

#[test]
fn a_record_only_fixture_starts_nothing_and_signals_nothing() {
    let materialised = start("system_process_pid_one");
    assert!(materialised.roles().is_empty());
    assert!(materialised.handle("launchd").is_none());
    assert_eq!(materialised.plan().record_only, ["launchd"]);
    assert!(materialised.harness().signal_log().is_empty());
    assert_structure(&materialised, "system_process_pid_one");
}

#[test]
fn a_fixture_without_processes_starts_nothing() {
    let materialised = start("session_end_without_descendants");
    assert!(materialised.roles().is_empty());
    assert!(materialised.harness().signal_log().is_empty());
}

#[test]
fn a_lone_root_is_a_live_child_of_the_test() {
    let materialised = start("unrelated_process_no_evidence");
    assert_eq!(materialised.roles(), ["bystander"]);
    assert!(is_alive(&materialised, "bystander"));
    assert_eq!(ppid(&materialised, "bystander"), test_pid());
    assert_structure(&materialised, "unrelated_process_no_evidence");
}

#[test]
fn a_live_session_keeps_the_agent_and_its_child_alive_and_linked() {
    let materialised = start("live_session_background_server");
    assert!(is_alive(&materialised, "agent"));
    assert!(is_alive(&materialised, "dev_server"));
    assert_eq!(ppid(&materialised, "agent"), test_pid());
    assert_eq!(ppid(&materialised, "dev_server"), pid(&materialised, "agent"));
    assert!(materialised.harness().signal_log().is_empty());
    assert_structure(&materialised, "live_session_background_server");
}

#[test]
fn an_ended_session_is_gone_and_leaves_its_child_under_launchd_in_the_old_session() {
    let materialised = start("owned_ended_background_server");
    assert!(is_gone(&materialised, "agent"));
    assert!(is_alive(&materialised, "dev_server"));
    assert_eq!(ppid(&materialised, "dev_server"), 1);
    assert_ne!(sid(&materialised, "dev_server"), pid(&materialised, "dev_server"));
    assert_structure(&materialised, "owned_ended_background_server");
}

#[test]
fn a_detached_process_has_ppid_one_and_a_session_of_its_own() {
    let materialised = start("detached_tunnel");
    assert!(is_gone(&materialised, "agent"));
    assert!(is_alive(&materialised, "tunnel"));
    assert_eq!(ppid(&materialised, "tunnel"), 1);
    assert_eq!(sid(&materialised, "tunnel"), pid(&materialised, "tunnel"));
    assert_structure(&materialised, "detached_tunnel");
}

#[test]
fn a_detached_process_that_ignores_sigterm_keeps_running_after_sigterm() {
    let mut materialised = start("detached_ignores_sigterm");
    let handle = materialised.handle("listener").unwrap();
    let listener = pid(&materialised, "listener");
    materialised.harness_mut().signal(handle, Signal::Term).unwrap();
    assert!(!wait_until(|| !is_alive(&materialised, "listener"), SETTLE));
    let terms: Vec<i32> = materialised
        .harness()
        .signal_log()
        .iter()
        .filter(|event| event.signal == Signal::Term)
        .map(|event| event.pid)
        .collect();
    assert_eq!(terms, [listener]);
}

#[test]
fn a_cooperative_leftover_dies_on_sigterm_and_an_ignoring_one_survives_it() {
    let mut cooperative = start("owned_ended_background_server");
    let handle = cooperative.handle("dev_server").unwrap();
    cooperative.harness_mut().signal(handle, Signal::Term).unwrap();
    assert!(wait_until(|| is_gone(&cooperative, "dev_server"), HANG_GUARD));

    let mut stubborn = start("owned_ended_ignores_sigterm");
    let handle = stubborn.handle("stubborn_server").unwrap();
    stubborn.harness_mut().signal(handle, Signal::Term).unwrap();
    assert!(!wait_until(|| !is_alive(&stubborn, "stubborn_server"), SETTLE));
}

#[test]
fn a_batch_of_three_leftovers_are_three_distinct_orphans() {
    let materialised = start("owned_ended_batch_of_three");
    let roles = ["worker_a", "worker_b", "worker_c"];
    let mut pids: Vec<i32> = roles.iter().map(|role| pid(&materialised, role)).collect();
    for role in roles {
        assert!(is_alive(&materialised, role));
        assert_eq!(ppid(&materialised, role), 1);
    }
    pids.sort_unstable();
    pids.dedup();
    assert_eq!(pids.len(), 3);
    assert_structure(&materialised, "owned_ended_batch_of_three");
}

#[test]
fn a_resumed_session_orphans_both_trees_and_signals_only_its_two_agents() {
    let materialised = start("resumed_session_leftovers");
    for agent in ["agent_first", "agent_resumed"] {
        assert!(is_gone(&materialised, agent), "{agent}");
    }
    for leftover in ["first_leftover", "second_leftover"] {
        assert!(is_alive(&materialised, leftover), "{leftover}");
        assert_eq!(ppid(&materialised, leftover), 1);
    }
    let mut killed: Vec<i32> = materialised
        .harness()
        .signal_log()
        .iter()
        .inspect(|event| assert_eq!(event.signal, Signal::Kill))
        .map(|event| event.pid)
        .collect();
    let mut agents = vec![
        pid(&materialised, "agent_first"),
        pid(&materialised, "agent_resumed"),
    ];
    killed.sort_unstable();
    agents.sort_unstable();
    assert_eq!(killed, agents);
    assert_structure(&materialised, "resumed_session_leftovers");
}

#[test]
fn a_short_lived_leftover_exits_by_itself_and_reads_gone() {
    let materialised = start("owned_ended_exits_during_approval");
    assert_structure(&materialised, "owned_ended_exits_during_approval");
    assert!(wait_until(|| is_gone(&materialised, "short_lived"), HANG_GUARD));
    assert_structure(&materialised, "owned_ended_exits_during_approval");
}

#[test]
fn the_leftover_of_an_abruptly_ended_agent_is_alive_under_launchd() {
    let materialised = start("abrupt_termination_no_session_end");
    assert_eq!(materialised.roles(), ["agent", "sampled_child"]);
    assert!(is_gone(&materialised, "agent"));
    assert!(is_alive(&materialised, "sampled_child"));
    assert_eq!(ppid(&materialised, "sampled_child"), 1);
    assert_structure(&materialised, "abrupt_termination_no_session_end");
}

#[test]
fn a_live_launcher_chain_keeps_both_processes_alive_and_linked() {
    let materialised = start("unrelated_process_with_live_launcher");
    assert!(is_alive(&materialised, "launcher"));
    assert!(is_alive(&materialised, "server"));
    assert_eq!(ppid(&materialised, "launcher"), test_pid());
    assert_eq!(ppid(&materialised, "server"), pid(&materialised, "launcher"));
    assert!(materialised.harness().signal_log().is_empty());
    assert_structure(&materialised, "unrelated_process_with_live_launcher");
}

#[test]
fn every_fixture_in_the_corpus_has_the_structure_it_claims() {
    let loaded = load_dir(&corpus_dir()).expect("the corpus loads");
    assert!(loaded.len() >= 14);
    for item in loaded {
        let name = item.fixture.name.clone();
        let materialised =
            materialise(&item.fixture, FIXTURE_BIN).unwrap_or_else(|err| panic!("{name}: {err}"));
        assert_structure(&materialised, &name);
        let started: Vec<&str> = materialised.roles();
        let live = item
            .fixture
            .processes
            .iter()
            .filter(|process| !process.record_only)
            .count();
        assert_eq!(started.len(), live, "{name}");
        materialised
            .shutdown()
            .unwrap_or_else(|err| panic!("{name}: {err}"));
    }
}

#[test]
fn a_structure_that_differs_from_the_plan_is_reported_for_the_role() {
    let planned = fixture("owned_ended_background_server").plan().unwrap();

    let mut own_session = planned.clone();
    own_session.groups[0].children[0].structure.own_session = true;
    let materialised = materialise_plan(&own_session, FIXTURE_BIN).unwrap();
    let violations = materialised.check_structure();
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert_eq!(violations[0].role, "dev_server");
    assert!(violations[0].detail.contains("session"), "{violations:?}");

    let mut parent = planned.clone();
    parent.groups[0].children[0].structure.parent = ParentExpectation::Harness;
    let materialised = materialise_plan(&parent, FIXTURE_BIN).unwrap();
    let violations = materialised.check_structure();
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert_eq!(violations[0].role, "dev_server");
    assert!(violations[0].detail.contains("parent"), "{violations:?}");

    let mut launchd = fixture("live_session_background_server").plan().unwrap();
    launchd.groups[0].children[0].structure.parent = ParentExpectation::Launchd;
    let materialised = materialise_plan(&launchd, FIXTURE_BIN).unwrap();
    let violations = materialised.check_structure();
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].detail.contains("parent"), "{violations:?}");
}

#[test]
fn a_process_that_is_not_running_is_reported_unless_its_lifetime_is_over() {
    let mut materialised = start("live_session_background_server");
    let handle = materialised.handle("dev_server").unwrap();
    materialised.harness_mut().signal(handle, Signal::Kill).unwrap();
    assert!(wait_until(|| is_gone(&materialised, "dev_server"), HANG_GUARD));
    let lifetime = Duration::from_secs(FIXTURE_SECONDS);

    let violations = materialised.check_structure_at(Duration::ZERO);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert_eq!(violations[0].role, "dev_server");
    assert!(violations[0].detail.starts_with("not running"), "{violations:?}");

    let just_before = materialised.check_structure_at(lifetime - Duration::from_millis(1));
    assert_eq!(just_before, violations);

    assert!(materialised.check_structure_at(lifetime).is_empty());
    assert!(materialised.check_structure_at(lifetime * 2).is_empty());
}

#[test]
fn the_lifetime_excuse_only_covers_a_process_that_is_gone() {
    let materialised = start("live_session_background_server");
    let lifetime = Duration::from_secs(FIXTURE_SECONDS);
    assert!(materialised.check_structure_at(lifetime * 2).is_empty());

    let mut wrong_parent = fixture("live_session_background_server").plan().unwrap();
    wrong_parent.groups[0].children[0].structure.parent = ParentExpectation::Launchd;
    let materialised = materialise_plan(&wrong_parent, FIXTURE_BIN).unwrap();
    let violations = materialised.check_structure_at(lifetime * 2);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].detail.contains("parent"), "{violations:?}");
}

#[test]
fn no_process_of_the_corpus_can_pass_a_wait_by_running_out_of_time() {
    let loaded = load_dir(&corpus_dir()).expect("the corpus loads");
    let mut members = 0;
    for item in loaded {
        let plan = item.fixture.plan().unwrap();
        for group in &plan.groups {
            for member in std::iter::once(&group.parent).chain(&group.children) {
                assert!(
                    member.spec.exit_after_ms.is_some() || member.spec.lifetime() > HANG_GUARD,
                    "{}/{}",
                    item.fixture.name,
                    member.role
                );
                members += 1;
            }
        }
    }
    assert!(members >= 20, "{members} members");
}

#[test]
fn a_fixture_that_cannot_be_planned_starts_nothing_and_says_why() {
    let value = json!({
        "schema_version": 1,
        "name": "deep",
        "description": "A grandchild that the harness cannot start.",
        "agent": "claude",
        "processes": [
            { "role": "agent", "flags": [], "expected":
                { "label": "true_managed", "class": "managed", "must_never_signal": true } },
            { "role": "shell", "parent_role": "agent", "flags": [], "expected":
                { "label": "true_unknown", "class": "unknown", "must_never_signal": true } },
            { "role": "server", "parent_role": "shell", "flags": [], "expected":
                { "label": "true_unknown", "class": "unknown", "must_never_signal": true } }
        ],
        "journal": [],
        "session_ended": false
    });
    let fixture: Fixture = serde_json::from_value(value).unwrap();
    match materialise(&fixture, FIXTURE_BIN) {
        Err(MaterialiseError::Plan(problems)) => assert_eq!(
            problems,
            [PlanProblem::TooDeep {
                role: "server".to_owned()
            }]
        ),
        Err(other) => panic!("expected a plan error, found {other}"),
        Ok(_) => panic!("the fixture should not start"),
    }
}

#[test]
fn a_fixture_binary_that_does_not_exist_is_an_error_not_a_panic() {
    let result = materialise(
        &fixture("live_session_background_server"),
        "/nonexistent/fixture-sleeper",
    );
    assert!(matches!(result, Err(MaterialiseError::Harness(_))));
}

#[test]
fn dropping_the_materialised_fixture_ends_every_process_even_when_detached_and_ignoring() {
    let provider = DarwinProvider::new().unwrap();
    let mut identities = Vec::new();
    {
        let materialised = start("detached_ignores_sigterm");
        let handle = materialised.handle("listener").unwrap();
        identities.push(materialised.harness().identity(handle).clone());
        assert_eq!(revalidate(&identities[0], &provider), Revalidation::Match);
    }
    assert_ne!(revalidate(&identities[0], &provider), Revalidation::Match);
}
