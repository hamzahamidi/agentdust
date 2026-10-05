mod classifier_support;
mod inventory_support;

use std::time::Duration;

use agentdust_core::class::{Actionability, Class, actionable_as};
use agentdust_core::classifier::{Evidence, Policy, Provenance, SUSPECT_MIN_AGE, SYSTEM_PREFIXES, classify};
use agentdust_core::inventory::{Launchd, Tag};
use agentdust_core::session::Liveness;
use classifier_support::{
    alive, class_of, degraded, evidence_of, find, gone, launchd, old, policy, run, scope, snapshot,
    snapshot_with, the_tool, unverified,
};
use inventory_support::{Edit, HOUR, MINUTE, NOW, SELF_PID, UID, raw};

#[test]
fn the_thresholds_are_the_provisional_values() {
    assert_eq!(SUSPECT_MIN_AGE, Duration::from_secs(30 * 60));
    assert_eq!(
        SYSTEM_PREFIXES,
        [
            "/System",
            "/usr",
            "/bin",
            "/sbin",
            "/Library/Apple",
            "/Applications"
        ]
    );
}

#[test]
fn evidence_names_are_dotted_and_unique() {
    let mut seen = std::collections::BTreeSet::new();
    for evidence in Evidence::ALL {
        let name = evidence.as_str();
        assert!(name.is_ascii() && name.contains('.'), "{name}");
        assert!(seen.insert(name), "{name}");
        assert_eq!(serde_json::to_string(&evidence).unwrap(), format!("\"{name}\""));
    }
    assert_eq!(Evidence::ManagedLaunchd.as_str(), "managed.launchd");
    assert_eq!(Evidence::SuspectIdle.as_str(), "suspect.idle");
}

#[test]
fn every_process_gets_one_finding_in_snapshot_order() {
    let found = run(vec![raw(301), raw(300), raw(302)], &[]);
    let pids: Vec<i32> = found.iter().map(|f| f.identity.kernel.pid).collect();
    assert_eq!(pids, [301, 300, 302]);
}

#[test]
fn a_finding_carries_the_raw_identity_and_the_age() {
    let found = run(vec![raw(300).started(NOW - 90 * MINUTE)], &[]);
    assert_eq!(found[0].identity, raw(300).started(NOW - 90 * MINUTE).identity);
    assert_eq!(found[0].age_us, 90 * MINUTE);
}

#[test]
fn an_age_before_the_start_is_zero() {
    let found = run(vec![raw(300).started(NOW + HOUR)], &[]);
    assert_eq!(found[0].age_us, 0);
}

#[test]
fn pid_one_is_managed() {
    let found = run(vec![raw(1).ppid(0).exe("/sbin/launchd"), raw(0).ppid(0)], &[]);
    assert_eq!(class_of(&found, 1), Class::Managed);
    assert!(evidence_of(&found, 1).contains(&Evidence::ManagedInit));
    assert_eq!(class_of(&found, 0), Class::Managed);
}

#[test]
fn another_users_process_is_managed_even_with_every_suspect_trait() {
    let found = run(vec![old(300).uid(0), old(301).uid(UID + 1), old(302)], &[]);
    assert_eq!(class_of(&found, 300), Class::Managed);
    assert_eq!(evidence_of(&found, 300), [Evidence::ManagedOtherUser]);
    assert_eq!(class_of(&found, 301), Class::Managed);
    assert_eq!(class_of(&found, 302), Class::Suspect);
}

#[test]
fn the_tool_itself_and_its_ancestors_are_managed() {
    let mut processes = the_tool();
    processes.push(old(300));
    processes.push(raw(301).ppid(SELF_PID));
    let found = run(processes, &[]);
    assert_eq!(evidence_of(&found, SELF_PID), [Evidence::ManagedSelf]);
    assert_eq!(evidence_of(&found, 8000), [Evidence::ManagedAncestor]);
    assert_eq!(class_of(&found, 7000), Class::Managed);
    assert_eq!(class_of(&found, 300), Class::Suspect);
    assert_eq!(class_of(&found, 301), Class::Unknown);
}

#[test]
fn an_idle_old_ancestor_under_launchd_is_never_a_suspect() {
    let mut processes = vec![raw(SELF_PID).ppid(8000), old(8000).ppid(7000), old(7000)];
    processes.push(old(300));
    let found = run(processes, &[]);
    assert_eq!(class_of(&found, 8000), Class::Managed);
    assert_eq!(class_of(&found, 7000), Class::Managed);
}

#[test]
fn a_cycle_in_the_parent_links_does_not_hang_the_ancestor_walk() {
    let processes = vec![
        raw(SELF_PID).ppid(8000),
        raw(8000).ppid(7000),
        raw(7000).ppid(8000),
    ];
    let found = run(processes, &[]);
    assert_eq!(class_of(&found, 7000), Class::Managed);
}

#[test]
fn live_agent_processes_are_managed_by_executable_script_or_identity() {
    let processes = vec![
        old(300).exe("/Users/dev/.local/share/claude/versions/2.1.289"),
        old(301).exe("/opt/tools/claude"),
        old(302).exe("/opt/homebrew/bin/node").script_agent(),
        raw(303).ppid(1),
    ];
    let scopes = [alive(303, &[1])];
    let found = run(processes, &scopes);
    for pid in [300, 301, 302, 303] {
        assert_eq!(class_of(&found, pid), Class::Managed, "{pid}");
        assert_eq!(evidence_of(&found, pid), [Evidence::ManagedAgent], "{pid}");
    }
}

#[test]
fn a_reused_agent_pid_with_another_start_time_is_not_the_agent() {
    let scopes = [gone(303, &[1])];
    let found = run(vec![old(303)], &scopes);
    assert_ne!(class_of(&found, 303), Class::Managed);
}

#[test]
fn a_node_process_with_another_script_is_not_an_agent() {
    let found = run(vec![old(300).exe("/opt/homebrew/bin/node")], &[]);
    assert_eq!(class_of(&found, 300), Class::Suspect);
}

#[test]
fn every_apple_system_prefix_is_managed() {
    for (index, prefix) in SYSTEM_PREFIXES.iter().enumerate() {
        let pid = 300 + index as i32;
        let found = run(vec![old(pid).exe(&format!("{prefix}/thing/bin/tool"))], &[]);
        assert_eq!(class_of(&found, pid), Class::Managed, "{prefix}");
        assert_eq!(
            evidence_of(&found, pid),
            [Evidence::ManagedSystemPath],
            "{prefix}"
        );
    }
}

#[test]
fn the_prefix_match_is_on_whole_path_components() {
    for path in [
        "/usrfoo/bin/tool",
        "/Systems/x",
        "/binx/tool",
        "/sbinary/x",
        "/Library/Application Support/x/tool",
        "/Library/AppleExtra/x",
        "/Applications2/x",
        "/Users/dev/usr/bin/tool",
        "/opt/homebrew/bin/node",
        "usr/bin/tool",
    ] {
        let found = run(vec![old(300).exe(path)], &[]);
        assert_eq!(class_of(&found, 300), Class::Suspect, "{path}");
    }
}

#[test]
fn usr_local_is_covered_by_usr() {
    let found = run(vec![old(300).exe("/usr/local/bin/node")], &[]);
    assert_eq!(class_of(&found, 300), Class::Managed);
}

#[test]
fn a_prefix_alone_is_a_system_path() {
    let found = run(vec![old(300).exe("/usr")], &[]);
    assert_eq!(class_of(&found, 300), Class::Managed);
}

#[test]
fn a_pid_in_the_launchctl_list_is_managed() {
    let found = classify(
        &snapshot_with(vec![old(300), old(301)], launchd(&[300])),
        &Provenance::Available(&[]),
        &policy(),
    );
    assert_eq!(class_of(&found, 300), Class::Managed);
    assert_eq!(evidence_of(&found, 300), [Evidence::ManagedLaunchd]);
    assert_eq!(class_of(&found, 301), Class::Suspect);
}

#[test]
fn every_managed_reason_is_listed_in_a_fixed_order() {
    let found = classify(
        &snapshot_with(
            vec![
                old(1).ppid(0).exe("/sbin/launchd"),
                old(300).uid(0).exe("/usr/bin/x"),
            ],
            launchd(&[1, 300]),
        ),
        &Provenance::Available(&[]),
        &policy(),
    );
    assert_eq!(
        evidence_of(&found, 1),
        [
            Evidence::ManagedInit,
            Evidence::ManagedSystemPath,
            Evidence::ManagedLaunchd
        ]
    );
    assert_eq!(
        evidence_of(&found, 300),
        [
            Evidence::ManagedOtherUser,
            Evidence::ManagedSystemPath,
            Evidence::ManagedLaunchd
        ]
    );
}

#[test]
fn managed_wins_over_every_ownership_signal() {
    let scopes = [gone(900, &[1]), alive(901, &[2])];
    let processes = vec![
        old(300).exe("/usr/bin/x").tagged(1),
        old(301).tagged(1),
        old(302).tagged(2),
        old(303).uid(0).tagged(1),
    ];
    let found = classify(
        &snapshot_with(processes, launchd(&[301, 302])),
        &Provenance::Available(&scopes),
        &policy(),
    );
    for pid in [300, 301, 302, 303] {
        assert_eq!(class_of(&found, pid), Class::Managed, "{pid}");
    }
}

#[test]
fn a_tag_of_a_session_whose_agent_is_alive_is_owned_live() {
    let scopes = [alive(900, &[1])];
    let found = run(vec![old(300).tagged(1)], &scopes);
    assert_eq!(class_of(&found, 300), Class::OwnedLive);
    assert_eq!(
        evidence_of(&found, 300),
        [Evidence::OwnedTag, Evidence::OwnedAgentAlive]
    );
}

#[test]
fn a_tag_of_a_session_whose_liveness_is_unreadable_is_owned_live_and_unverified() {
    let scopes = [unverified(900, &[1])];
    let found = run(vec![old(300).tagged(1)], &scopes);
    assert_eq!(class_of(&found, 300), Class::OwnedLive);
    assert_eq!(
        evidence_of(&found, 300),
        [Evidence::OwnedTag, Evidence::OwnedAgentUnverified]
    );
}

#[test]
fn a_session_end_record_without_the_agent_gone_keeps_the_process_owned_live() {
    let scopes = [scope(Some(900), Some(Liveness::Alive), true, &[1])];
    let found = run(vec![old(300).tagged(1)], &scopes);
    assert_eq!(class_of(&found, 300), Class::OwnedLive);
    let scopes = [scope(Some(900), Some(Liveness::Unknown), true, &[1])];
    let found = run(vec![old(300).tagged(1)], &scopes);
    assert_eq!(class_of(&found, 300), Class::OwnedLive);
}

#[test]
fn a_tag_of_a_session_whose_agent_is_gone_is_owned_ended() {
    let scopes = [gone(900, &[1])];
    let found = run(vec![raw(300).tagged(1)], &scopes);
    assert_eq!(class_of(&found, 300), Class::OwnedEnded);
    assert_eq!(
        evidence_of(&found, 300),
        [Evidence::OwnedTag, Evidence::OwnedAgentGone]
    );
    assert_eq!(actionable_as(Class::OwnedEnded), Actionability::BatchCode);
}

#[test]
fn owned_ended_needs_neither_age_nor_idleness_nor_an_orphaned_parent() {
    let scopes = [gone(900, &[1])];
    let young_busy = raw(300).tagged(1).started(NOW - MINUTE).busy().ppid(4242);
    let found = run(vec![young_busy], &scopes);
    assert_eq!(class_of(&found, 300), Class::OwnedEnded);
}

#[test]
fn a_tagged_orphan_that_is_old_and_idle_is_owned_ended_and_not_a_suspect() {
    let scopes = [gone(900, &[1])];
    let found = run(vec![old(300).tagged(1)], &scopes);
    assert_eq!(class_of(&found, 300), Class::OwnedEnded);
}

#[test]
fn several_sessions_with_one_tag_end_only_when_every_agent_is_gone() {
    let both_gone = [gone(900, &[1]), gone(901, &[1])];
    assert_eq!(
        class_of(&run(vec![raw(300).tagged(1)], &both_gone), 300),
        Class::OwnedEnded
    );
    let one_alive = [gone(900, &[1]), alive(901, &[1])];
    assert_eq!(
        class_of(&run(vec![raw(300).tagged(1)], &one_alive), 300),
        Class::OwnedLive
    );
    let one_unverified = [gone(900, &[1]), unverified(901, &[1])];
    assert_eq!(
        class_of(&run(vec![raw(300).tagged(1)], &one_unverified), 300),
        Class::OwnedLive
    );
}

#[test]
fn a_degraded_session_never_ends_a_process_and_never_makes_it_a_suspect() {
    let only_degraded = [degraded(&[1])];
    let found = run(vec![old(300).tagged(1)], &only_degraded);
    assert_eq!(class_of(&found, 300), Class::Unknown);
    assert_eq!(evidence_of(&found, 300), [Evidence::TagDegraded]);
    let with_gone = [gone(900, &[1]), degraded(&[1])];
    let found = run(vec![old(300).tagged(1)], &with_gone);
    assert_eq!(class_of(&found, 300), Class::Unknown);
    assert_eq!(
        evidence_of(&found, 300),
        [
            Evidence::OwnedTag,
            Evidence::OwnedAgentGone,
            Evidence::TagDegraded
        ]
    );
    let with_alive = [alive(900, &[1]), degraded(&[1])];
    let found = run(vec![old(300).tagged(1)], &with_alive);
    assert_eq!(class_of(&found, 300), Class::OwnedLive);
}

#[test]
fn a_tag_that_no_session_explains_is_unknown_even_when_every_suspect_trait_holds() {
    let scopes = [gone(900, &[1])];
    let found = run(vec![old(300).tagged(9)], &scopes);
    assert_eq!(class_of(&found, 300), Class::Unknown);
    assert_eq!(evidence_of(&found, 300), [Evidence::TagUnmatched]);
}

#[test]
fn a_tag_that_could_not_be_checked_is_unknown_even_when_every_suspect_trait_holds() {
    let scopes = [gone(900, &[1])];
    for tag in [Tag::Unkeyed, Tag::Unreadable] {
        let found = run(vec![old(300).tag(tag.clone())], &scopes);
        assert_eq!(class_of(&found, 300), Class::Unknown, "{tag:?}");
        assert_eq!(evidence_of(&found, 300), [Evidence::TagUnverifiable], "{tag:?}");
    }
}

#[test]
fn without_provenance_a_tag_is_unverifiable_and_owned_classes_do_not_exist() {
    let processes = vec![old(300).tagged(1), old(301).tagged(2), raw(302).tagged(1)];
    let found = classify(&snapshot(processes), &Provenance::Unavailable, &policy());
    for pid in [300, 301, 302] {
        assert_eq!(class_of(&found, pid), Class::Unknown, "{pid}");
        assert_eq!(evidence_of(&found, pid), [Evidence::TagUnverifiable], "{pid}");
    }
}

#[test]
fn an_owned_process_without_a_readable_path_cannot_be_actionable() {
    let scopes = [gone(900, &[1])];
    let found = run(vec![raw(300).tagged(1).no_path()], &scopes);
    assert_eq!(class_of(&found, 300), Class::Unknown);
    assert_eq!(
        evidence_of(&found, 300),
        [
            Evidence::OwnedTag,
            Evidence::OwnedAgentGone,
            Evidence::PathUnreadable
        ]
    );
}

#[test]
fn an_old_idle_untagged_orphan_is_a_suspect() {
    let found = run(vec![old(300)], &[]);
    assert_eq!(class_of(&found, 300), Class::Suspect);
    assert_eq!(
        evidence_of(&found, 300),
        [
            Evidence::SuspectParentLaunchd,
            Evidence::SuspectSameUser,
            Evidence::SuspectAge,
            Evidence::SuspectIdle
        ]
    );
    assert_eq!(actionable_as(Class::Suspect), Actionability::PerItemCode);
}

#[test]
fn the_age_threshold_is_inclusive() {
    let at = run(vec![old(300).started(NOW - 30 * MINUTE)], &[]);
    assert_eq!(class_of(&at, 300), Class::Suspect);
    let under = run(vec![old(300).started(NOW - 30 * MINUTE + 1)], &[]);
    assert_eq!(class_of(&under, 300), Class::Unknown);
    assert!(evidence_of(&under, 300).is_empty());
}

#[test]
fn the_age_threshold_comes_from_the_policy() {
    let policy = Policy {
        min_age: Duration::from_secs(60),
        ..Policy::new(UID, SELF_PID)
    };
    let young = old(300).started(NOW - 2 * MINUTE);
    let found = classify(&snapshot(vec![young]), &Provenance::Available(&[]), &policy);
    assert_eq!(class_of(&found, 300), Class::Suspect);
}

#[test]
fn a_process_that_used_cpu_during_the_sample_is_not_a_suspect() {
    let found = run(vec![old(300).busy()], &[]);
    assert_eq!(class_of(&found, 300), Class::Unknown);
}

#[test]
fn a_process_without_two_cpu_samples_is_not_a_suspect() {
    for (first, later) in [(None, None), (Some(1), None), (None, Some(1))] {
        let found = run(vec![old(300).cpu(first, later)], &[]);
        assert_eq!(class_of(&found, 300), Class::Unknown, "{first:?} {later:?}");
    }
}

#[test]
fn a_process_with_a_live_parent_is_not_a_suspect() {
    let found = run(vec![raw(200).ppid(1), old(300).ppid(200)], &[]);
    assert_eq!(class_of(&found, 300), Class::Unknown);
}

#[test]
fn a_parent_missing_from_the_snapshot_is_not_evidence_that_the_chain_is_gone() {
    let found = run(vec![old(300).ppid(4242)], &[]);
    assert_eq!(class_of(&found, 300), Class::Unknown);
}

#[test]
fn a_would_be_suspect_without_a_readable_path_is_unknown() {
    let found = run(vec![old(300).no_path()], &[]);
    assert_eq!(class_of(&found, 300), Class::Unknown);
    assert_eq!(
        evidence_of(&found, 300),
        [
            Evidence::SuspectParentLaunchd,
            Evidence::SuspectSameUser,
            Evidence::SuspectAge,
            Evidence::SuspectIdle,
            Evidence::PathUnreadable
        ]
    );
}

#[test]
fn without_provenance_an_untagged_orphan_can_still_be_a_suspect() {
    let found = classify(&snapshot(vec![old(300)]), &Provenance::Unavailable, &policy());
    assert_eq!(class_of(&found, 300), Class::Suspect);
}

#[test]
fn without_the_launchd_list_nothing_can_be_actionable() {
    let scopes = [gone(900, &[1]), alive(901, &[2])];
    let processes = vec![
        old(300),
        old(301).tagged(1),
        old(302).tagged(2),
        old(303).exe("/usr/bin/x"),
    ];
    let found = classify(
        &snapshot_with(processes, Launchd::Unavailable),
        &Provenance::Available(&scopes),
        &policy(),
    );
    assert_eq!(class_of(&found, 300), Class::Unknown);
    assert_eq!(
        evidence_of(&found, 300),
        [
            Evidence::SuspectParentLaunchd,
            Evidence::SuspectSameUser,
            Evidence::SuspectAge,
            Evidence::SuspectIdle,
            Evidence::LaunchdUnavailable
        ]
    );
    assert_eq!(class_of(&found, 301), Class::Unknown);
    assert_eq!(
        evidence_of(&found, 301),
        [
            Evidence::OwnedTag,
            Evidence::OwnedAgentGone,
            Evidence::LaunchdUnavailable
        ]
    );
    assert_eq!(class_of(&found, 302), Class::OwnedLive);
    assert_eq!(class_of(&found, 303), Class::Managed);
}

#[test]
fn likely_owned_is_never_produced() {
    let scopes = [gone(900, &[1]), alive(901, &[2]), degraded(&[3])];
    let processes = vec![
        old(300),
        old(301).tagged(1),
        old(302).tagged(2),
        old(303).tagged(3),
        old(304).tagged(9),
        raw(305),
    ];
    let found = run(processes, &scopes);
    assert!(found.iter().all(|finding| finding.class != Class::LikelyOwned));
}

#[test]
fn the_snapshot_and_scopes_are_not_changed_by_classification() {
    let processes = vec![old(300), old(301).tagged(1)];
    let scopes = [gone(900, &[1])];
    let shot = snapshot(processes.clone());
    let before = shot.clone();
    classify(&shot, &Provenance::Available(&scopes), &policy());
    assert_eq!(shot, before);
}

#[test]
fn a_finding_names_its_identity_for_revalidation() {
    let found = run(vec![old(300)], &[]);
    let finding = find(&found, 300);
    assert_eq!(finding.identity.kernel, old(300).identity.kernel);
    assert_eq!(
        finding.identity.exe_path.as_deref(),
        Some(std::path::Path::new("/opt/homebrew/bin/node"))
    );
}
