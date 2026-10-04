mod classifier_support;
mod inventory_support;

use agentdust_core::class::Class;
use agentdust_core::classifier::{Finding, Provenance, classify};
use agentdust_core::inventory::{RawProcess, Tag};
use agentdust_core::session::{Liveness, Scope};
use classifier_support::specs::{arb_spec, class_at, pid_of, run, scopes};
use classifier_support::{alive, gone, launchd, policy, snapshot_with};
use inventory_support::{Edit, MINUTE, NOW, raw};
use proptest::collection::vec;
use proptest::prelude::*;

const ENDED: i32 = 300;
const LIVE: i32 = 301;
const ORPHAN: i32 = 302;
const YOUNG: i32 = 303;
const SYSTEM: i32 = 304;
const AGENT: i32 = 305;

fn processes() -> Vec<RawProcess> {
    vec![
        raw(ENDED).tagged(1).ppid(1).started(NOW - 90 * MINUTE),
        raw(LIVE).tagged(2).ppid(1).started(NOW - 90 * MINUTE),
        raw(ORPHAN).ppid(1).started(NOW - 90 * MINUTE),
        raw(YOUNG).ppid(1).started(NOW - MINUTE),
        raw(SYSTEM)
            .ppid(1)
            .started(NOW - 90 * MINUTE)
            .exe("/usr/sbin/cfprefsd"),
        raw(AGENT)
            .ppid(1)
            .started(NOW - 90 * MINUTE)
            .exe("/opt/tools/claude"),
    ]
}

fn full_scopes() -> Vec<Scope> {
    vec![gone(900, &[1]), alive(901, &[2])]
}

fn classes(processes: Vec<RawProcess>, provenance: &Provenance) -> Vec<Class> {
    let found: Vec<Finding> = classify(&snapshot_with(processes, launchd(&[])), provenance, &policy());
    [ENDED, LIVE, ORPHAN, YOUNG, SYSTEM, AGENT]
        .iter()
        .map(|pid| class_at(&found, *pid))
        .collect()
}

fn full() -> Vec<Class> {
    classes(processes(), &Provenance::Available(&full_scopes()))
}

fn lowered() -> Vec<Class> {
    use Class::{Managed, Suspect, Unknown};
    vec![Unknown, Unknown, Suspect, Unknown, Managed, Managed]
}

fn no_class_became_actionable(loss: &[Class]) {
    let before = full();
    for (index, class) in loss.iter().enumerate() {
        assert_ne!(*class, Class::OwnedEnded, "process {index}");
        if matches!(class, Class::Suspect | Class::OwnedEnded) {
            assert_eq!(*class, before[index], "process {index}");
        }
    }
}

#[test]
fn with_full_provenance_the_fixtures_have_the_expected_classes() {
    use Class::{Managed, OwnedEnded, OwnedLive, Suspect, Unknown};
    assert_eq!(
        full(),
        [OwnedEnded, OwnedLive, Suspect, Unknown, Managed, Managed]
    );
}

#[test]
fn a_lost_journal_downgrades_owned_classes_and_nothing_else() {
    let found = classes(processes(), &Provenance::Available(&[]));
    assert_eq!(found, lowered());
    no_class_became_actionable(&found);
}

#[test]
fn an_unsupported_journal_or_volume_downgrades_owned_classes_and_nothing_else() {
    let found = classes(processes(), &Provenance::Unavailable);
    assert_eq!(found, lowered());
    no_class_became_actionable(&found);
}

#[test]
fn a_hook_that_found_no_agent_leaves_sessions_degraded_and_downgrades_owned_classes() {
    let degraded: Vec<Scope> = full_scopes()
        .into_iter()
        .map(|mut scope| {
            scope.identity = None;
            scope.liveness = None;
            scope
        })
        .collect();
    let found = classes(processes(), &Provenance::Available(&degraded));
    assert_eq!(found, lowered());
    no_class_became_actionable(&found);
}

#[test]
fn a_lost_session_start_record_downgrades_owned_classes() {
    let keyless: Vec<Scope> = full_scopes()
        .into_iter()
        .map(|mut scope| {
            scope.tag_keys.clear();
            scope
        })
        .collect();
    let found = classes(processes(), &Provenance::Available(&keyless));
    assert_eq!(found, lowered());
    no_class_became_actionable(&found);
}

#[test]
fn a_lost_install_secret_leaves_tags_unkeyed_and_downgrades_owned_classes() {
    let unkeyed: Vec<RawProcess> = processes()
        .into_iter()
        .map(|mut process| {
            if matches!(process.tag, Tag::Keyed(_)) {
                process.tag = Tag::Unkeyed;
            }
            process
        })
        .collect();
    let found = classes(unkeyed, &Provenance::Available(&full_scopes()));
    assert_eq!(found, lowered());
    no_class_became_actionable(&found);
}

#[test]
fn unreadable_liveness_keeps_owned_processes_out_of_every_actionable_class() {
    let unknown: Vec<Scope> = full_scopes()
        .into_iter()
        .map(|mut scope| {
            scope.liveness = Some(Liveness::Unknown);
            scope
        })
        .collect();
    let found = classes(processes(), &Provenance::Available(&unknown));
    use Class::{Managed, OwnedLive, Suspect, Unknown};
    assert_eq!(found, [OwnedLive, OwnedLive, Suspect, Unknown, Managed, Managed]);
    no_class_became_actionable(&found);
}

#[test]
fn a_tag_with_no_other_evidence_never_becomes_a_suspect_under_any_loss() {
    let tagged_orphans: Vec<RawProcess> = vec![
        raw(ENDED).tagged(1).ppid(1).started(NOW - 600 * MINUTE),
        raw(LIVE).tagged(2).ppid(1).started(NOW - 600 * MINUTE),
    ];
    for provenance in [Provenance::Unavailable, Provenance::Available(&[])] {
        let found = classify(
            &snapshot_with(tagged_orphans.clone(), launchd(&[])),
            &provenance,
            &policy(),
        );
        assert_eq!(class_at(&found, ENDED), Class::Unknown);
        assert_eq!(class_at(&found, LIVE), Class::Unknown);
    }
}

fn lose(scopes: &[Scope], which: u8) -> Vec<Scope> {
    scopes
        .iter()
        .cloned()
        .map(|mut scope| {
            match which {
                0 => {
                    scope.identity = None;
                    scope.liveness = None;
                }
                1 => scope.tag_keys.clear(),
                _ => scope.liveness = Some(Liveness::Unknown),
            }
            scope
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn any_loss_of_provenance_only_lowers_confidence(
        specs in vec(arb_spec(), 1..24),
        launchd_known in any::<bool>(),
        which in 0..5u8,
    ) {
        let scopes = scopes();
        let before = run(&specs, &Provenance::Available(&scopes), launchd_known);
        let degraded;
        let after = match which {
            3 => run(&specs, &Provenance::Unavailable, launchd_known),
            4 => run(&specs, &Provenance::Available(&[]), launchd_known),
            n => {
                degraded = lose(&scopes, n);
                run(&specs, &Provenance::Available(&degraded), launchd_known)
            }
        };
        for (index, spec) in specs.iter().enumerate() {
            let pid = pid_of(index);
            let (was, now) = (class_at(&before, pid), class_at(&after, pid));
            if matches!(now, Class::OwnedEnded | Class::Suspect) {
                prop_assert_eq!(now, was, "{:?}", spec);
            }
            prop_assert_ne!(now, Class::OwnedEnded, "{:?}", spec);
        }
    }
}
