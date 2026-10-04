#![cfg(target_os = "macos")]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::time::Duration;

use agentdust_core::darwin::DarwinProvider;
use agentdust_core::identity::ProcessIdentity;
use agentdust_core::revalidate::{Revalidation, revalidate};
use agentdust_testkit::harness::{Error, Harness, ProcHandle, Signal, Tree};
use agentdust_testkit::spec::ProcSpec;
use agentdust_testkit::wait_until;

const FIXTURE: &str = env!("CARGO_BIN_EXE_fixture-sleeper");
const HANG_GUARD: Duration = Duration::from_secs(60);
const SETTLE: Duration = Duration::from_millis(400);
const LIFE: u64 = 120;
const EXIT_AFTER_MS: u64 = 5_000;

fn harness() -> Harness {
    Harness::new(FIXTURE).unwrap()
}

fn long() -> ProcSpec {
    ProcSpec::new().seconds(LIFE)
}

fn tree(harness: &mut Harness, spec: &ProcSpec) -> Tree {
    harness.spawn_tree(spec).unwrap()
}

fn gone(identity: &ProcessIdentity) -> bool {
    let provider = DarwinProvider::new().unwrap();
    revalidate(identity, &provider) != Revalidation::Match
}

fn survives(harness: &Harness, handle: ProcHandle) -> bool {
    !wait_until(|| harness.revalidate(handle) != Revalidation::Match, SETTLE)
}

fn dies(harness: &Harness, handle: ProcHandle) -> bool {
    wait_until(|| harness.revalidate(handle) == Revalidation::Gone, HANG_GUARD)
}

fn sent(harness: &Harness) -> Vec<(i32, Signal)> {
    harness.signal_log().iter().map(|e| (e.pid, e.signal)).collect()
}

#[test]
fn a_tree_registers_the_parent_and_each_child_with_their_identities() {
    let mut harness = harness();
    let tree = tree(&mut harness, &long().spawn(2));
    assert_eq!(tree.children.len(), 2);
    let parent = harness.pid(tree.parent);
    let first = harness.pid(tree.children[0]);
    let second = harness.pid(tree.children[1]);
    assert_ne!(first, second);
    for child in &tree.children {
        let pid = harness.pid(*child);
        assert_ne!(pid, parent);
        assert_eq!(harness.report(*child).pid, pid);
        assert_eq!(harness.report(*child).ppid, parent);
        assert_eq!(harness.ppid(*child).unwrap(), parent);
        assert_eq!(harness.identity(*child).kernel.pid, pid);
        assert_eq!(harness.revalidate(*child), Revalidation::Match);
        assert_eq!(
            harness.identity(*child).evidence.exe_path.canonicalize().unwrap(),
            Path::new(FIXTURE).canonicalize().unwrap()
        );
    }
    assert_eq!(harness.ppid(tree.parent).unwrap(), std::process::id() as i32);
    assert!(harness.signal_log().is_empty());
}

#[test]
fn a_tree_without_children_is_one_registered_process() {
    let mut harness = harness();
    let tree = tree(&mut harness, &long());
    assert!(tree.children.is_empty());
    assert_eq!(harness.revalidate(tree.parent), Revalidation::Match);
}

#[test]
fn the_echoed_variable_reaches_every_process_of_the_tree() {
    let mut harness = harness();
    let spec = long()
        .spawn(2)
        .env("AGENTDUST_TAG", "tree-3")
        .echo_env("AGENTDUST_TAG");
    let tree = tree(&mut harness, &spec);
    for handle in [tree.parent, tree.children[0], tree.children[1]] {
        assert_eq!(harness.report(handle).env.as_deref(), Some(&b"tree-3"[..]));
    }
}

#[test]
fn a_cooperative_child_dies_on_sigterm_and_nothing_else_does() {
    let mut harness = harness();
    let tree = tree(&mut harness, &long().spawn(2));
    harness.signal(tree.children[0], Signal::Term).unwrap();
    assert!(dies(&harness, tree.children[0]));
    assert!(survives(&harness, tree.children[1]));
    assert!(survives(&harness, tree.parent));
    assert_eq!(
        sent(&harness),
        vec![(harness.pid(tree.children[0]), Signal::Term)]
    );
}

#[test]
fn sigterm_ignoring_children_survive_sigterm_and_the_log_shows_one_sigterm_each() {
    let mut harness = harness();
    let tree = tree(&mut harness, &long().spawn(2).ignore_term());
    for child in &tree.children {
        harness.signal(*child, Signal::Term).unwrap();
    }
    for child in &tree.children {
        assert!(survives(&harness, *child));
    }
    assert!(survives(&harness, tree.parent));
    assert_eq!(
        sent(&harness),
        vec![
            (harness.pid(tree.children[0]), Signal::Term),
            (harness.pid(tree.children[1]), Signal::Term)
        ]
    );
}

#[test]
fn orphaning_kills_the_parent_and_reparents_every_child_to_launchd() {
    let mut harness = harness();
    let tree = tree(&mut harness, &long().spawn(2));
    harness.orphan(tree.parent).unwrap();
    assert_eq!(harness.revalidate(tree.parent), Revalidation::Gone);
    for child in &tree.children {
        assert_eq!(harness.ppid(*child).unwrap(), 1);
        assert_eq!(harness.revalidate(*child), Revalidation::Match);
    }
    assert_eq!(sent(&harness), vec![(harness.pid(tree.parent), Signal::Kill)]);
}

#[test]
fn a_setsid_child_survives_its_parent_being_killed_with_ppid_1_and_a_session_of_its_own() {
    let mut harness = harness();
    let plain = harness.spawn(&long()).unwrap();
    let tree = tree(&mut harness, &long().spawn(1).setsid());
    let child = tree.children[0];
    let parent_pid = harness.pid(tree.parent);
    assert_eq!(harness.ppid(child).unwrap(), parent_pid);
    harness.orphan(tree.parent).unwrap();
    assert_eq!(harness.ppid(child).unwrap(), 1);
    assert!(survives(&harness, child));
    let report = harness.report(child);
    assert_eq!(report.sid, report.pid);
    assert_ne!(report.sid, harness.report(plain).sid);
    assert_ne!(report.sid, harness.report(tree.parent).sid);
    assert_ne!(harness.report(plain).sid, harness.report(plain).pid);
}

#[test]
fn a_child_can_still_be_signalled_after_its_parent_was_orphaned() {
    let mut harness = harness();
    let tree = tree(&mut harness, &long().spawn(1));
    harness.orphan(tree.parent).unwrap();
    harness.signal(tree.children[0], Signal::Term).unwrap();
    assert!(dies(&harness, tree.children[0]));
    assert_eq!(
        sent(&harness),
        vec![
            (harness.pid(tree.parent), Signal::Kill),
            (harness.pid(tree.children[0]), Signal::Term)
        ]
    );
}

#[test]
fn an_orphaned_sigterm_ignoring_child_survives_sigterm_and_dies_on_sigkill() {
    let mut harness = harness();
    let tree = tree(&mut harness, &long().spawn(1).ignore_term().setsid());
    let child = tree.children[0];
    harness.orphan(tree.parent).unwrap();
    harness.signal(child, Signal::Term).unwrap();
    assert!(survives(&harness, child));
    harness.signal(child, Signal::Kill).unwrap();
    assert!(dies(&harness, child));
}

#[test]
fn orphan_refuses_a_process_that_is_not_a_direct_child() {
    let mut harness = harness();
    let tree = tree(&mut harness, &long().spawn(1));
    let child = tree.children[0];
    let pid = harness.pid(child);
    let result = harness.orphan(child);
    assert!(
        matches!(result, Err(Error::NotADirectChild(p)) if p == pid),
        "{result:?}"
    );
    assert!(survives(&harness, child));
    assert!(survives(&harness, tree.parent));
    assert!(harness.signal_log().is_empty());
}

#[test]
fn orphan_refuses_a_handle_from_another_harness() {
    let mut owner = harness();
    let mut other = harness();
    let tree = tree(&mut owner, &long().spawn(1));
    let bystander = other.spawn(&long()).unwrap();
    let result = other.orphan(tree.parent);
    assert!(matches!(result, Err(Error::ForeignHandle)), "{result:?}");
    assert!(survives(&owner, tree.parent));
    assert!(survives(&other, bystander));
    assert!(owner.signal_log().is_empty());
    assert!(other.signal_log().is_empty());
}

#[test]
fn orphan_refuses_a_parent_that_has_already_exited() {
    let mut harness = harness();
    let tree = tree(&mut harness, &long().spawn(1).exit_after_ms(EXIT_AFTER_MS));
    assert!(dies(&harness, tree.parent));
    let pid = harness.pid(tree.parent);
    let result = harness.orphan(tree.parent);
    assert!(
        matches!(result, Err(Error::NotRunning(p)) if p == pid),
        "{result:?}"
    );
    assert!(harness.signal_log().is_empty());
}

#[test]
fn a_parent_can_be_orphaned_once() {
    let mut harness = harness();
    let tree = tree(&mut harness, &long().spawn(1));
    harness.orphan(tree.parent).unwrap();
    let pid = harness.pid(tree.parent);
    let result = harness.orphan(tree.parent);
    assert!(
        matches!(result, Err(Error::NotRunning(p)) if p == pid),
        "{result:?}"
    );
    assert_eq!(sent(&harness), vec![(pid, Signal::Kill)]);
    assert!(survives(&harness, tree.children[0]));
}

#[test]
fn a_child_that_exits_during_the_approval_window_reads_gone_and_cannot_be_signalled() {
    let mut harness = harness();
    let tree = tree(&mut harness, &long().spawn(1).exit_after_ms(EXIT_AFTER_MS));
    let child = tree.children[0];
    let approved = harness.identity(child).clone();
    assert_eq!(revalidate(&approved, harness.provider()), Revalidation::Match);
    assert!(dies(&harness, child));
    assert_eq!(revalidate(&approved, harness.provider()), Revalidation::Gone);
    let result = harness.signal(child, Signal::Term);
    assert!(matches!(result, Err(Error::NotRunning(_))), "{result:?}");
    assert!(harness.signal_log().is_empty());
}

#[test]
fn no_process_of_a_tree_survives_the_drop() {
    let mut harness = harness();
    let tree = tree(&mut harness, &long().spawn(2).ignore_term());
    let identities: Vec<ProcessIdentity> = [tree.parent, tree.children[0], tree.children[1]]
        .iter()
        .map(|handle| harness.identity(*handle).clone())
        .collect();
    drop(harness);
    for identity in &identities {
        assert!(gone(identity), "{identity:?}");
    }
}

#[test]
fn no_orphaned_child_that_ignores_sigterm_and_sits_in_its_own_session_survives_the_drop() {
    let mut harness = harness();
    let tree = tree(&mut harness, &long().spawn(2).ignore_term().setsid());
    let identities: Vec<ProcessIdentity> = [tree.parent, tree.children[0], tree.children[1]]
        .iter()
        .map(|handle| harness.identity(*handle).clone())
        .collect();
    harness.orphan(tree.parent).unwrap();
    drop(harness);
    for identity in &identities {
        assert!(gone(identity), "{identity:?}");
    }
}

#[test]
fn no_process_of_a_tree_survives_a_panic() {
    let mut identities = Vec::new();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut harness = harness();
        let tree = tree(&mut harness, &long().spawn(2).ignore_term().setsid());
        harness.orphan(tree.parent).unwrap();
        identities.extend(
            [tree.parent, tree.children[0], tree.children[1]]
                .iter()
                .map(|handle| harness.identity(*handle).clone()),
        );
        panic!("a test failed while its fixtures were running");
    }));
    assert!(result.is_err());
    assert_eq!(identities.len(), 3);
    for identity in &identities {
        assert!(gone(identity), "{identity:?}");
    }
}

#[test]
fn shutdown_reports_no_survivor_for_a_tree() {
    let mut harness = harness();
    let tree = tree(&mut harness, &long().spawn(2).ignore_term());
    let identities: Vec<ProcessIdentity> = tree
        .children
        .iter()
        .map(|handle| harness.identity(*handle).clone())
        .collect();
    harness.shutdown().unwrap();
    for identity in &identities {
        assert!(gone(identity));
    }
}
