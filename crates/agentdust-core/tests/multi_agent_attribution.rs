mod classifier_support;
mod inventory_support;
mod session_support;

use agentdust_core::class::Class;
use agentdust_core::journal::Kind;
use classifier_support::run as classify;
use inventory_support::{Edit as ProcessEdit, raw};
use session_support::{Edit as RecordEdit, Table, rec, run as scopes};

#[test]
fn interleaved_sessions_and_subagents_keep_tags_in_their_own_owner_scopes() {
    let records = [
        rec(Kind::SessionStart).session("lead").by(10).tagged(1),
        rec(Kind::SessionStart).session("teammate-a").by(20).tagged(2),
        rec(Kind::SessionStart).session("teammate-b").by(30).tagged(3),
        rec(Kind::SubagentStart).session("lead").by(10).sub("agent-a"),
        rec(Kind::SubagentStart).session("lead").by(10).sub("agent-b"),
        rec(Kind::ShellStart).session("lead").by(10).sub("agent-a"),
        rec(Kind::ShellStart).session("lead").by(10).sub("agent-b"),
        rec(Kind::SubagentStop).session("lead").by(10).sub("agent-a"),
        rec(Kind::SessionEnd).session("teammate-a").by(20),
    ];
    let table = Table::new().alive(10).gone(20).alive(30);
    let session_scopes = scopes(&records, &table);
    let found = classify(
        vec![raw(300).tagged(1), raw(301).tagged(2), raw(302).tagged(3)],
        &session_scopes,
    );

    assert_eq!(
        found.iter().map(|item| item.class).collect::<Vec<_>>(),
        [Class::OwnedLive, Class::OwnedEnded, Class::OwnedLive,]
    );
    assert_eq!(
        found[0].attribution_owners.as_ref().unwrap()[0].session_id,
        "lead"
    );
    assert_eq!(
        found[1].attribution_owners.as_ref().unwrap()[0].session_id,
        "teammate-a"
    );
    assert_eq!(
        found[2].attribution_owners.as_ref().unwrap()[0].session_id,
        "teammate-b"
    );
    assert_eq!(session_scopes[0].subagent_ids.len(), 2);
}

#[test]
fn a_subagent_stop_and_parent_session_end_do_not_replace_fresh_parent_liveness() {
    let records = [
        rec(Kind::SessionStart).session("lead").by(10).tagged(1),
        rec(Kind::SubagentStart).session("lead").by(10).sub("agent-a"),
        rec(Kind::SubagentStop).session("lead").by(10).sub("agent-a"),
        rec(Kind::SessionEnd).session("lead").by(10),
    ];
    let session_scopes = scopes(&records, &Table::new().alive(10));
    let found = classify(vec![raw(300).tagged(1)], &session_scopes);

    assert_eq!(found[0].class, Class::OwnedLive);
    assert_eq!(found[0].attribution_owners.as_ref().unwrap().len(), 1);
}

#[test]
fn shared_tag_ownership_requires_every_exact_owner_to_be_proven_gone() {
    let records = [
        rec(Kind::SessionStart).session("owner-a").by(10).tagged(4),
        rec(Kind::SessionStart).session("owner-b").by(20).tagged(4),
    ];
    let both_gone = scopes(&records, &Table::new().gone(10).gone(20));
    let ended = classify(vec![raw(300).tagged(4)], &both_gone);
    assert_eq!(ended[0].class, Class::OwnedEnded);
    let owners = ended[0].attribution_owners.as_ref().unwrap();
    assert_eq!(owners.len(), 2);
    assert_eq!(owners[0].session_id, "owner-a");
    assert_eq!(owners[1].session_id, "owner-b");

    let one_resumed = scopes(&records, &Table::new().gone(10).alive(20));
    let live = classify(vec![raw(300).tagged(4)], &one_resumed);
    assert_eq!(live[0].class, Class::OwnedLive);
}

#[test]
fn a_subagent_process_is_an_additional_exact_owner_of_the_parent_tag() {
    let records = [
        rec(Kind::SessionStart).session("lead").by(10).tagged(7),
        rec(Kind::SubagentStart).session("lead").by(10).sub("agent-a"),
        rec(Kind::ShellStart).session("lead").by(20).sub("agent-a"),
        rec(Kind::SessionEnd).session("lead").by(10),
    ];

    for (probe, expected) in [
        (Table::new().gone(10).alive(20), Class::OwnedLive),
        (Table::new().alive(10).gone(20), Class::OwnedLive),
        (Table::new().gone(10).gone(20), Class::OwnedEnded),
    ] {
        let session_scopes = scopes(&records, &probe);
        let found = classify(vec![raw(300).tagged(7)], &session_scopes);
        assert_eq!(found[0].class, expected);
        let owners = found[0].attribution_owners.as_ref().unwrap();
        assert_eq!(owners.len(), 2);
        assert_eq!(
            owners.iter().map(|owner| owner.identity.pid).collect::<Vec<_>>(),
            [10, 20]
        );
    }
}

#[test]
fn an_unidentified_subagent_event_makes_cleanup_attribution_ambiguous() {
    let records = [
        rec(Kind::SessionStart).session("lead").by(10).tagged(8),
        rec(Kind::ShellStart).session("lead").sub("agent-a"),
    ];
    let session_scopes = scopes(&records, &Table::new().gone(10));
    let found = classify(vec![raw(300).tagged(8)], &session_scopes);

    assert_eq!(found[0].class, Class::Unknown);
    assert!(found[0].attribution_owners.is_none());
}

#[test]
fn a_late_end_from_an_old_resume_does_not_close_the_new_owner() {
    let records = [
        rec(Kind::SessionStart).session("resume").by(10).tagged(5),
        rec(Kind::SessionStart).session("resume").by(20).tagged(6),
        rec(Kind::SessionEnd).session("resume").by(10),
    ];
    let session_scopes = scopes(&records, &Table::new().gone(10).alive(20));
    let found = classify(vec![raw(300).tagged(6)], &session_scopes);
    assert_eq!(found[0].class, Class::OwnedLive);
    assert_eq!(found[0].attribution_owners.as_ref().unwrap()[0].identity.pid, 20);
}

#[test]
fn a_late_subagent_stop_stays_with_its_exact_owner_after_resume() {
    let records = [
        rec(Kind::SessionStart).session("resume").by(10).tagged(9),
        rec(Kind::SubagentStart).session("resume").by(10).sub("agent-a"),
        rec(Kind::SessionStart).session("resume").by(20).tagged(10),
        rec(Kind::SubagentStop).session("resume").by(10).sub("agent-a"),
    ];
    let session_scopes = scopes(&records, &Table::new().gone(10).alive(20));
    let found = classify(vec![raw(300).tagged(9), raw(301).tagged(10)], &session_scopes);

    assert_eq!(found[0].class, Class::OwnedEnded);
    assert_eq!(found[1].class, Class::OwnedLive);
    assert_eq!(session_scopes[0].subagents["agent-a"].stop_events, 1);
    assert!(session_scopes[1].subagents.is_empty());
    assert!(!session_scopes[0].session_ended);
    assert_eq!(found[1].attribution_owners.as_ref().unwrap()[0].identity.pid, 20);
}

#[test]
fn a_subagent_start_without_an_id_makes_the_parent_tag_non_actionable() {
    let records = [
        rec(Kind::SessionStart).session("lead").by(10).tagged(11),
        rec(Kind::SubagentStart).session("lead").by(10),
        rec(Kind::SessionEnd).session("lead").by(10),
    ];
    let session_scopes = scopes(&records, &Table::new().gone(10));
    let found = classify(vec![raw(300).tagged(11)], &session_scopes);

    assert_eq!(found[0].class, Class::Unknown);
    assert!(found[0].attribution_owners.is_none());
}

#[test]
fn a_subagent_stop_without_an_id_makes_the_parent_tag_non_actionable() {
    let records = [
        rec(Kind::SessionStart).session("lead").by(10).tagged(12),
        rec(Kind::SubagentStop).session("lead").by(10),
        rec(Kind::SessionEnd).session("lead").by(10),
    ];
    let session_scopes = scopes(&records, &Table::new().gone(10));
    let found = classify(vec![raw(300).tagged(12)], &session_scopes);

    assert_eq!(found[0].class, Class::Unknown);
    assert!(found[0].attribution_owners.is_none());
}

#[test]
fn an_unresolved_subagent_digest_makes_the_whole_session_non_actionable() {
    let records = [
        rec(Kind::SessionStart).session("lead").by(10).tagged(13),
        rec(Kind::SubagentAttributionUnknown).session("lead").by(20),
        rec(Kind::SessionEnd).session("lead").by(10),
    ];
    let session_scopes = scopes(&records, &Table::new().gone(10).alive(20));
    let found = classify(vec![raw(300).tagged(13)], &session_scopes);

    assert_eq!(found[0].class, Class::Unknown);
    assert!(found[0].attribution_owners.is_none());
}

#[test]
fn a_child_event_before_its_parent_makes_later_parent_tags_non_actionable() {
    let records = [
        rec(Kind::SubagentStart).session("lead").by(20).sub("agent-a"),
        rec(Kind::SessionStart).session("lead").by(10).tagged(14),
        rec(Kind::SessionEnd).session("lead").by(10),
    ];
    let session_scopes = scopes(&records, &Table::new().gone(10).alive(20));
    let found = classify(vec![raw(300).tagged(14)], &session_scopes);

    assert_eq!(found[0].class, Class::Unknown);
    assert!(found[0].attribution_owners.is_none());
}
