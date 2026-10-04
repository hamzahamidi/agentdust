mod classifier_support;
mod inventory_support;

use agentdust_core::class::Class;
use agentdust_core::classifier::{Evidence, Provenance, classify};
use agentdust_core::inventory::Launchd;
use classifier_support::{
    alive, class_of, evidence_of, gone, launchd, old, policy, run, snapshot_with, the_tool,
};
use inventory_support::{Edit, raw};

fn run_with(
    processes: Vec<agentdust_core::inventory::RawProcess>,
    pids: &[i32],
) -> Vec<agentdust_core::classifier::Finding> {
    classify(
        &snapshot_with(processes, launchd(pids)),
        &Provenance::Available(&[]),
        &policy(),
    )
}

#[test]
fn a_helper_of_a_launchd_job_is_managed() {
    let found = run_with(vec![old(300), old(301).ppid(300)], &[300]);
    assert_eq!(class_of(&found, 300), Class::Managed);
    assert_eq!(evidence_of(&found, 300), [Evidence::ManagedLaunchd]);
    assert_eq!(class_of(&found, 301), Class::Managed);
    assert_eq!(evidence_of(&found, 301), [Evidence::ManagedLaunchdDescendant]);
}

#[test]
fn every_descendant_of_a_launchd_job_is_managed() {
    let processes = vec![
        old(300),
        raw(301).ppid(300),
        raw(302).ppid(301),
        raw(303).ppid(302),
    ];
    let found = run_with(processes, &[300]);
    for pid in [301, 302, 303] {
        assert_eq!(class_of(&found, pid), Class::Managed, "{pid}");
    }
}

#[test]
fn the_evidence_for_a_job_that_is_also_below_another_job_names_the_job_itself() {
    let found = run_with(vec![old(300), old(301).ppid(300)], &[300, 301]);
    assert_eq!(evidence_of(&found, 301), [Evidence::ManagedLaunchd]);
}

#[test]
fn a_process_whose_parent_is_not_a_job_is_not_managed_by_it() {
    let found = run_with(vec![old(300), raw(301).ppid(1), raw(302).ppid(301)], &[300]);
    assert_eq!(class_of(&found, 301), Class::Suspect);
    assert_eq!(class_of(&found, 302), Class::Unknown);
}

#[test]
fn managed_by_a_job_wins_over_ownership_and_suspect_traits() {
    let scopes = [gone(900, &[1]), alive(901, &[2])];
    let processes = vec![
        old(300),
        old(301).ppid(300).tagged(1),
        old(302).ppid(300).tagged(2),
        old(303).ppid(300),
    ];
    let found = classify(
        &snapshot_with(processes, launchd(&[300])),
        &Provenance::Available(&scopes),
        &policy(),
    );
    for pid in [301, 302, 303] {
        assert_eq!(class_of(&found, pid), Class::Managed, "{pid}");
    }
}

#[test]
fn without_the_launchd_list_the_rule_cannot_apply() {
    let processes = vec![old(300), old(301).ppid(300)];
    let found = classify(
        &snapshot_with(processes, Launchd::Unavailable),
        &Provenance::Available(&[]),
        &policy(),
    );
    assert_eq!(class_of(&found, 301), Class::Unknown);
}

#[test]
fn a_cycle_in_the_parent_links_ends_the_walk() {
    let found = run_with(
        vec![raw(300).ppid(301), raw(301).ppid(300), raw(302).ppid(300)],
        &[999],
    );
    assert_eq!(class_of(&found, 302), Class::Unknown);
}

#[test]
fn a_parent_missing_from_the_snapshot_ends_the_walk() {
    let found = run_with(vec![raw(300).ppid(4242), raw(301).ppid(300)], &[4242]);
    assert_eq!(class_of(&found, 300), Class::Managed);
    assert_eq!(class_of(&found, 301), Class::Managed);
    let found = run_with(vec![raw(300).ppid(4242), raw(301).ppid(300)], &[7]);
    assert_eq!(class_of(&found, 301), Class::Unknown);
}

#[test]
fn the_walk_is_bounded() {
    let mut processes = vec![raw(1000)];
    for pid in 1001..1100 {
        processes.push(raw(pid).ppid(pid - 1));
    }
    let found = run_with(processes, &[1000]);
    assert_eq!(class_of(&found, 1010), Class::Managed);
    assert_eq!(class_of(&found, 1099), Class::Unknown);
}

#[test]
fn the_tool_and_its_ancestors_stay_managed_for_their_own_reason() {
    let mut processes = the_tool();
    processes.push(old(300));
    let found = run(processes, &[]);
    assert_eq!(evidence_of(&found, 8000), [Evidence::ManagedAncestor]);
}
