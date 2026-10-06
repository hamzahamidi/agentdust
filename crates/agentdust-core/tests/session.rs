mod session_support;

use std::cell::Cell;
use std::collections::BTreeSet;

use agentdust_core::identity::KernelIdentity;
use agentdust_core::journal::{Agent, Kind};
use agentdust_core::session::{Liveness, LivenessProbe, State, scopes};
use session_support::{Edit, Table, kernel, kernel_on, key, rec, run};

fn states(records: &[agentdust_core::journal::Record], table: &Table) -> Vec<State> {
    run(records, table).iter().map(|scope| scope.state).collect()
}

#[test]
fn no_records_give_no_scopes() {
    assert!(run(&[], &Table::new()).is_empty());
}

#[test]
fn the_first_record_of_a_session_is_unknown_when_nothing_proves_the_agent_alive() {
    for kind in [Kind::ShellStart, Kind::ShellEnd, Kind::Sample, Kind::ServerStart] {
        let found = run(&[rec(kind)], &Table::new());
        assert_eq!(found.len(), 1, "{kind:?}");
        assert_eq!(found[0].state, State::Unknown, "{kind:?}");
        assert!(found[0].degraded(), "{kind:?}");
    }
}

#[test]
fn a_session_start_makes_the_scope_active_even_without_an_identity() {
    let found = run(&[rec(Kind::SessionStart)], &Table::new());
    assert_eq!(found[0].state, State::Active);
    assert!(found[0].degraded());
    assert_eq!(found[0].identity, None);
    assert!(!found[0].agent_gone());
}

#[test]
fn a_session_start_with_a_live_agent_is_active_and_anchored() {
    let found = run(&[rec(Kind::SessionStart).by(10)], &Table::new().alive(10));
    assert_eq!(found[0].state, State::Active);
    assert!(!found[0].degraded());
    assert_eq!(found[0].identity, Some(kernel(10)));
    assert_eq!(found[0].liveness, Some(Liveness::Alive));
    assert_eq!(
        found[0].exe_base.as_ref().map(|name| name.as_str()),
        Some("claude")
    );
}

#[test]
fn a_session_start_whose_agent_is_gone_is_ended_by_the_agent() {
    let found = run(&[rec(Kind::SessionStart).by(10)], &Table::new().gone(10));
    assert_eq!(found[0].state, State::Ended);
    assert!(found[0].agent_gone());
    assert!(!found[0].session_ended);
}

#[test]
fn a_session_start_whose_liveness_cannot_be_read_stays_active_and_never_ends() {
    let found = run(&[rec(Kind::SessionStart).by(10)], &Table::new());
    assert_eq!(found[0].state, State::Active);
    assert_eq!(found[0].liveness, Some(Liveness::Unknown));
    assert!(!found[0].agent_gone());
}

#[test]
fn any_event_with_a_live_identity_makes_an_unknown_scope_active() {
    let found = run(&[rec(Kind::ShellStart).by(10)], &Table::new().alive(10));
    assert_eq!(found[0].state, State::Active);
}

#[test]
fn an_event_with_an_identity_that_cannot_be_probed_leaves_the_scope_unknown() {
    let found = run(&[rec(Kind::ShellStart).by(10)], &Table::new());
    assert_eq!(found[0].state, State::Unknown);
}

#[test]
fn an_event_whose_agent_is_gone_ends_the_scope_without_a_session_start() {
    let found = run(&[rec(Kind::ShellEnd).by(10)], &Table::new().gone(10));
    assert_eq!(found[0].state, State::Ended);
    assert!(found[0].agent_gone());
}

#[test]
fn a_session_end_ends_the_scope_but_the_agent_is_not_gone() {
    let records = [rec(Kind::SessionStart), rec(Kind::SessionEnd)];
    let found = run(&records, &Table::new());
    assert_eq!(found[0].state, State::Ended);
    assert!(found[0].session_ended);
    assert!(!found[0].agent_gone());
}

#[test]
fn a_session_end_with_a_live_agent_is_ended_and_the_agent_is_not_gone() {
    let records = [rec(Kind::SessionStart).by(10), rec(Kind::SessionEnd).by(10)];
    let found = run(&records, &Table::new().alive(10));
    assert_eq!(found[0].state, State::Ended);
    assert!(found[0].session_ended);
    assert!(!found[0].agent_gone());
    assert_eq!(found[0].liveness, Some(Liveness::Alive));
}

#[test]
fn a_session_end_and_a_gone_agent_are_both_recorded() {
    let records = [rec(Kind::SessionStart).by(10), rec(Kind::SessionEnd).by(10)];
    let found = run(&records, &Table::new().gone(10));
    assert_eq!(found[0].state, State::Ended);
    assert!(found[0].session_ended);
    assert!(found[0].agent_gone());
}

#[test]
fn a_session_end_without_any_start_is_an_ended_scope() {
    let found = run(&[rec(Kind::SessionEnd)], &Table::new());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].state, State::Ended);
}

#[test]
fn a_duplicate_session_start_with_the_same_identity_opens_no_second_scope() {
    let records = [
        rec(Kind::SessionStart).by(10),
        rec(Kind::ShellStart).by(10),
        rec(Kind::SessionStart).by(10),
    ];
    assert_eq!(run(&records, &Table::new().alive(10)).len(), 1);
}

#[test]
fn a_duplicate_session_start_changes_nothing_but_adds_its_tag_key() {
    let once = [rec(Kind::SessionStart).by(10).tagged(1)];
    let twice = [
        rec(Kind::SessionStart).by(10).tagged(1),
        rec(Kind::SessionStart).by(10).tagged(1),
    ];
    let table = Table::new().alive(10);
    assert_eq!(run(&once, &table), run(&twice, &table));
    let again = [
        rec(Kind::SessionStart).by(10).tagged(1),
        rec(Kind::SessionStart).by(10).tagged(2),
    ];
    let found = run(&again, &table);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].tag_keys, BTreeSet::from([key(1), key(2)]));
}

#[test]
fn a_different_identity_for_the_same_session_opens_a_resumed_scope() {
    let records = [
        rec(Kind::SessionStart).by(10).tagged(1),
        rec(Kind::SessionStart).by(20).tagged(2),
    ];
    let found = run(&records, &Table::new().gone(10).alive(20));
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].identity, Some(kernel(10)));
    assert_eq!(found[0].state, State::Ended);
    assert_eq!(found[0].tag_keys, BTreeSet::from([key(1)]));
    assert_eq!(found[1].identity, Some(kernel(20)));
    assert_eq!(found[1].state, State::Active);
    assert_eq!(found[1].tag_keys, BTreeSet::from([key(2)]));
}

#[test]
fn the_same_pid_on_another_boot_is_another_identity() {
    let records = [
        rec(Kind::SessionStart).by(10).on_boot("b1"),
        rec(Kind::SessionStart).by(10).on_boot("b2"),
    ];
    let found = run(&records, &Table::new());
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].identity, Some(kernel_on("b1", 10)));
    assert_eq!(found[1].identity, Some(kernel_on("b2", 10)));
}

#[test]
fn a_late_event_of_an_old_agent_does_not_reopen_its_scope_or_open_another() {
    let records = [
        rec(Kind::SessionStart).by(10),
        rec(Kind::SessionStart).by(20),
        rec(Kind::ShellEnd).by(10),
    ];
    let found = run(&records, &Table::new().gone(10).alive(20));
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].state, State::Ended);
    assert_eq!(found[1].state, State::Active);
}

#[test]
fn late_events_never_move_an_ended_scope_back_to_active() {
    let records = [
        rec(Kind::SessionStart).by(10),
        rec(Kind::SessionEnd).by(10),
        rec(Kind::ShellStart).by(10),
        rec(Kind::ShellEnd),
        rec(Kind::SessionStart).by(10),
    ];
    let found = run(&records, &Table::new().alive(10));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].state, State::Ended);
}

#[test]
fn a_late_event_after_a_session_end_without_an_identity_stays_ended() {
    let records = [
        rec(Kind::SessionStart),
        rec(Kind::SessionEnd),
        rec(Kind::ShellStart),
    ];
    assert_eq!(states(&records, &Table::new()), [State::Ended]);
}

#[test]
fn an_identity_free_end_does_not_close_one_of_several_resumed_scopes() {
    let records = [
        rec(Kind::SessionStart).by(10),
        rec(Kind::SessionStart).by(20),
        rec(Kind::SessionEnd),
    ];
    let found = run(&records, &Table::new().alive(10).alive(20));
    assert_eq!(found[0].state, State::Active);
    assert_eq!(found[1].state, State::Active);
}

#[test]
fn a_session_start_without_an_identity_never_merges_its_tag_into_an_anchored_scope() {
    let records = [
        rec(Kind::SessionStart).by(10).tagged(1),
        rec(Kind::SessionStart).tagged(2),
    ];
    let found = run(&records, &Table::new().gone(10));
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].tag_keys, BTreeSet::from([key(1)]));
    assert_eq!(found[0].state, State::Ended);
    assert_eq!(found[1].tag_keys, BTreeSet::from([key(2)]));
    assert!(found[1].degraded());
    assert_eq!(found[1].state, State::Active);
    assert!(!found[1].agent_gone());
}

#[test]
fn session_starts_without_an_identity_share_one_degraded_scope() {
    let records = [
        rec(Kind::SessionStart).tagged(1),
        rec(Kind::SessionStart).tagged(2),
    ];
    let found = run(&records, &Table::new());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].tag_keys, BTreeSet::from([key(1), key(2)]));
}

#[test]
fn an_identity_never_moves_into_a_scope_that_had_none() {
    let records = [rec(Kind::ShellStart), rec(Kind::SessionStart).by(10).tagged(3)];
    let found = run(&records, &Table::new().gone(10));
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].identity, None);
    assert_eq!(found[0].state, State::Unknown);
    assert_eq!(found[1].identity, Some(kernel(10)));
    assert_eq!(found[1].state, State::Ended);
}

#[test]
fn subagent_records_share_the_scope_of_their_parent() {
    let records = [
        rec(Kind::SessionStart).by(10),
        rec(Kind::ShellStart).by(10).sub("agent-1"),
        rec(Kind::ShellEnd).by(10).sub("agent-1"),
        rec(Kind::ShellStart).by(10).sub("agent-2"),
    ];
    let found = run(&records, &Table::new().alive(10));
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].subagent_ids,
        BTreeSet::from(["agent-1".to_owned(), "agent-2".to_owned()])
    );
    assert_eq!(found[0].state, State::Active);
}

#[test]
fn a_subagent_record_never_opens_a_scope_of_its_own_beside_its_parent() {
    let records = [
        rec(Kind::SessionStart).by(10),
        rec(Kind::ShellStart).by(20).sub("agent-1"),
    ];
    let found = run(&records, &Table::new().alive(10).gone(20));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].identity, Some(kernel(10)));
    assert_eq!(found[0].state, State::Active);
    assert!(!found[0].agent_gone());
}

#[test]
fn subagent_session_tags_inherit_the_parent_owner_and_stop_events_do_not_end_it() {
    let records = [
        rec(Kind::SessionStart).by(10),
        rec(Kind::SessionEnd).by(10).sub("agent-1"),
        rec(Kind::SessionStart).by(10).sub("agent-1").tagged(9),
        rec(Kind::SubagentStart).by(10).sub("agent-1"),
        rec(Kind::SubagentStop).by(10).sub("agent-1"),
    ];
    let found = run(&records, &Table::new().alive(10));
    assert_eq!(found[0].state, State::Active);
    assert!(!found[0].session_ended);
    assert_eq!(found[0].tag_keys, BTreeSet::from([key(9)]));
    assert_eq!(
        found[0].subagents["agent-1"],
        agentdust_core::session::SubagentActivity {
            start_events: 2,
            stop_events: 2,
        }
    );
}

#[test]
fn repeated_and_late_subagent_events_are_observations_not_owner_death() {
    let records = [
        rec(Kind::SessionStart).by(10).tagged(1),
        rec(Kind::SubagentStart).by(10).sub("agent-1"),
        rec(Kind::SubagentStop).by(10).sub("agent-1"),
        rec(Kind::SubagentStop).by(10).sub("agent-1"),
        rec(Kind::SubagentStart).by(10).sub("agent-1"),
    ];
    let found = run(&records, &Table::new().alive(10));
    assert_eq!(found[0].state, State::Active);
    assert_eq!(found[0].tag_keys, BTreeSet::from([key(1)]));
    assert_eq!(
        found[0].subagents["agent-1"],
        agentdust_core::session::SubagentActivity {
            start_events: 2,
            stop_events: 2,
        }
    );
}

#[test]
fn subagent_lifecycle_without_an_id_or_exact_owner_does_not_attach_to_the_latest_scope() {
    let records = [
        rec(Kind::SessionStart).by(10).tagged(1),
        rec(Kind::SessionStart).by(20).tagged(2),
        rec(Kind::SubagentStop),
        rec(Kind::SubagentStart).sub("agent-unknown"),
    ];
    let found = run(&records, &Table::new().alive(10).alive(20));
    assert_eq!(found.len(), 2);
    assert!(found.iter().all(|scope| scope.subagents.is_empty()));
    assert_eq!(found[0].tag_keys, BTreeSet::from([key(1)]));
    assert_eq!(found[1].tag_keys, BTreeSet::from([key(2)]));
}

#[test]
fn the_first_record_of_a_session_may_be_a_subagent_record() {
    let found = run(
        &[rec(Kind::ShellStart).by(10).sub("agent-1")],
        &Table::new().alive(10),
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].state, State::Active);
    assert_eq!(found[0].subagent_ids.len(), 1);
}

#[test]
fn sessions_are_independent_and_come_out_in_order_of_first_appearance() {
    let records = [
        rec(Kind::SessionStart).session("b").by(10),
        rec(Kind::SessionStart).session("a").by(20),
        rec(Kind::SessionEnd).session("b").by(10),
    ];
    let found = run(&records, &Table::new().alive(10).alive(20));
    let ids: Vec<&str> = found.iter().map(|scope| scope.session_id.as_str()).collect();
    assert_eq!(ids, ["b", "a"]);
    assert_eq!(found[0].state, State::Ended);
    assert_eq!(found[1].state, State::Active);
}

#[test]
fn the_same_session_id_of_two_agents_is_two_scopes() {
    let records = [
        rec(Kind::SessionStart).agent(Agent::Claude).by(10),
        rec(Kind::SessionEnd).agent(Agent::Codex),
    ];
    let found = run(&records, &Table::new().alive(10));
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].agent, Agent::Claude);
    assert_eq!(found[0].state, State::Active);
    assert_eq!(found[1].agent, Agent::Codex);
    assert_eq!(found[1].state, State::Ended);
}

#[test]
fn two_sessions_of_one_agent_end_together_when_the_agent_is_gone() {
    let records = [
        rec(Kind::SessionStart).session("s1").by(10),
        rec(Kind::SessionStart).session("s2").by(10),
    ];
    let found = run(&records, &Table::new().gone(10));
    assert!(
        found
            .iter()
            .all(|scope| scope.state == State::Ended && scope.agent_gone())
    );
}

struct Counting {
    probed: Cell<Vec<i32>>,
}

impl LivenessProbe for Counting {
    fn probe(&self, identity: &KernelIdentity) -> Liveness {
        let mut seen = self.probed.take();
        seen.push(identity.pid);
        self.probed.set(seen);
        Liveness::Alive
    }
}

#[test]
fn each_identity_is_probed_once_however_many_scopes_share_it() {
    let records = [
        rec(Kind::SessionStart).session("s1").by(10),
        rec(Kind::SessionStart).session("s2").by(10),
        rec(Kind::ShellStart).session("s2").by(10),
        rec(Kind::SessionStart).session("s3").by(20),
    ];
    let probe = Counting {
        probed: Cell::new(Vec::new()),
    };
    scopes(&records, &probe);
    let mut seen = probe.probed.take();
    seen.sort_unstable();
    assert_eq!(seen, [10, 20]);
}

#[test]
fn a_scope_without_an_identity_is_never_probed() {
    let probe = Counting {
        probed: Cell::new(Vec::new()),
    };
    scopes(&[rec(Kind::SessionStart), rec(Kind::ShellStart)], &probe);
    assert!(probe.probed.take().is_empty());
}

#[test]
fn the_executable_name_comes_from_the_first_record_that_has_one() {
    let records = [rec(Kind::SessionStart).by(10)];
    let found = run(&records, &Table::new().alive(10));
    assert_eq!(
        found[0].exe_base.as_ref().map(|name| name.as_str()),
        Some("claude")
    );
}

#[test]
fn scopes_come_out_in_the_order_they_were_opened_across_sessions() {
    let records = [
        rec(Kind::SessionStart).session("b").by(10),
        rec(Kind::SessionStart).session("a").by(20),
        rec(Kind::SessionStart).session("b").by(30),
    ];
    let found = run(&records, &Table::new().alive(10).alive(20).alive(30));
    let order: Vec<(&str, i32)> = found
        .iter()
        .map(|scope| (scope.session_id.as_str(), scope.identity.as_ref().unwrap().pid))
        .collect();
    assert_eq!(order, [("b", 10), ("a", 20), ("b", 30)]);
}
