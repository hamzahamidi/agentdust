mod session_support;

use std::collections::BTreeSet;

use agentdust_core::identity::KernelIdentity;
use agentdust_core::journal::{Agent, Kind, Record};
use agentdust_core::session::{Liveness, LivenessProbe, Scope, State, scopes};
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::sample::select;
use session_support::{Edit, rec};

const KINDS: [Kind; 6] = [
    Kind::SessionStart,
    Kind::SessionEnd,
    Kind::ShellStart,
    Kind::ShellEnd,
    Kind::Sample,
    Kind::ServerStart,
];

struct Hashed(u8);

impl LivenessProbe for Hashed {
    fn probe(&self, identity: &KernelIdentity) -> Liveness {
        let spread = identity.pid as u64 + identity.start_time_us + identity.boot_session_uuid.len() as u64;
        match (spread + u64::from(self.0)) % 3 {
            0 => Liveness::Alive,
            1 => Liveness::Gone,
            _ => Liveness::Unknown,
        }
    }
}

struct Constant(Liveness);

impl LivenessProbe for Constant {
    fn probe(&self, _: &KernelIdentity) -> Liveness {
        self.0
    }
}

fn arb_record() -> impl Strategy<Value = Record> {
    (
        select(KINDS.to_vec()),
        select(vec![Agent::Claude, Agent::Codex]),
        select(vec!["s1", "s2"]),
        proptest::option::of(1..=4i32),
        select(vec!["b1", "b2"]),
        proptest::option::of(select(vec!["a1", "a2"])),
        proptest::option::of(1..=3u8),
    )
        .prop_map(|(kind, agent, session, pid, boot, sub, tag)| {
            let mut record = rec(kind).session(session).agent(agent).on_boot(boot);
            if let Some(pid) = pid {
                record = record.by(pid);
            }
            if let Some(sub) = sub {
                record = record.sub(sub);
            }
            if let Some(tag) = tag {
                record = record.tagged(tag);
            }
            record
        })
}

fn arb_records() -> impl Strategy<Value = Vec<Record>> {
    vec(arb_record(), 0..24)
}

fn group_of(scope: &Scope) -> (Agent, String) {
    (scope.agent, scope.session_id.clone())
}

proptest! {
    #[test]
    fn ended_is_absorbing_when_records_are_added(
        before in arb_records(),
        after in arb_records(),
        seed in 0u8..3,
    ) {
        let probe = Hashed(seed);
        let first = scopes(&before, &probe);
        let mut all = before.clone();
        all.extend(after);
        let second = scopes(&all, &probe);
        prop_assert!(second.len() >= first.len());
        for (index, scope) in first.iter().enumerate() {
            let later = &second[index];
            prop_assert_eq!(group_of(scope), group_of(later));
            prop_assert_eq!(&scope.identity, &later.identity);
            if scope.state == State::Ended {
                prop_assert_eq!(later.state, State::Ended);
            }
            if scope.session_ended {
                prop_assert!(later.session_ended);
            }
            if scope.agent_gone() {
                prop_assert!(later.agent_gone());
            }
            prop_assert!(scope.tag_keys.is_subset(&later.tag_keys));
            prop_assert!(scope.subagent_ids.is_subset(&later.subagent_ids));
        }
    }

    #[test]
    fn a_scope_ends_only_with_evidence_of_its_own_session(records in arb_records(), seed in 0u8..3) {
        let probe = Hashed(seed);
        for scope in scopes(&records, &probe) {
            if scope.state != State::Ended {
                continue;
            }
            let ended_in_records = records.iter().any(|record| {
                record.kind == Kind::SessionEnd
                    && record.subagent_id.is_none()
                    && (record.agent, &record.session_id) == (scope.agent, &scope.session_id)
            });
            let gone = scope
                .identity
                .as_ref()
                .is_some_and(|identity| probe.probe(identity) == Liveness::Gone);
            prop_assert!(ended_in_records || gone);
            prop_assert_eq!(scope.session_ended, ended_in_records && scope.session_ended);
            prop_assert_eq!(scope.agent_gone(), gone);
        }
    }

    #[test]
    fn an_agent_is_gone_only_when_the_probe_says_so(records in arb_records()) {
        for liveness in [Liveness::Alive, Liveness::Unknown] {
            for scope in scopes(&records, &Constant(liveness)) {
                prop_assert!(!scope.agent_gone());
                prop_assert!(scope.state != State::Ended || scope.session_ended);
            }
        }
    }

    #[test]
    fn no_session_end_and_no_gone_agent_means_no_ended_scope(records in arb_records()) {
        let without_ends: Vec<Record> = records
            .into_iter()
            .filter(|record| record.kind != Kind::SessionEnd)
            .collect();
        for scope in scopes(&without_ends, &Constant(Liveness::Alive)) {
            prop_assert!(scope.state != State::Ended);
        }
    }

    #[test]
    fn a_session_is_unaffected_by_the_records_of_other_sessions(records in arb_records(), seed in 0u8..3) {
        let probe = Hashed(seed);
        let everything = scopes(&records, &probe);
        for (agent, session) in [(Agent::Claude, "s1"), (Agent::Claude, "s2"), (Agent::Codex, "s1"), (Agent::Codex, "s2")] {
            let own: Vec<Record> = records
                .iter()
                .filter(|record| record.agent == agent && record.session_id == session)
                .cloned()
                .collect();
            let alone = scopes(&own, &probe);
            let within: Vec<Scope> = everything
                .iter()
                .filter(|scope| scope.agent == agent && scope.session_id == session)
                .cloned()
                .collect();
            prop_assert_eq!(alone, within);
        }
    }

    #[test]
    fn a_repeated_session_start_with_an_identity_changes_nothing(records in arb_records(), seed in 0u8..3) {
        let probe = Hashed(seed);
        let baseline = scopes(&records, &probe);
        for record in records.iter().filter(|record| {
            record.kind == Kind::SessionStart && record.subagent_id.is_none() && record.agent_identity.is_some()
        }) {
            let mut repeated = records.clone();
            repeated.push(record.clone());
            prop_assert_eq!(&scopes(&repeated, &probe), &baseline);
        }
    }

    #[test]
    fn identities_are_unique_within_a_session_and_the_degraded_scope_is_never_anchored(
        records in arb_records(),
        seed in 0u8..3,
    ) {
        let found = scopes(&records, &Hashed(seed));
        let mut seen = BTreeSet::new();
        for scope in &found {
            if let Some(identity) = &scope.identity {
                let fresh = seen.insert((group_of(scope), identity.boot_session_uuid.clone(), identity.pid, identity.start_time_us));
                prop_assert!(fresh);
                prop_assert!(scope.liveness.is_some());
            } else {
                prop_assert_eq!(scope.liveness, None);
                prop_assert!(!scope.agent_gone());
                prop_assert!(scope.degraded());
            }
        }
    }

    #[test]
    fn every_record_is_accounted_for_in_exactly_one_session(records in arb_records()) {
        let found = scopes(&records, &Constant(Liveness::Unknown));
        let sessions: BTreeSet<(Agent, String)> = found.iter().map(group_of).collect();
        let expected: BTreeSet<(Agent, String)> = records
            .iter()
            .map(|record| (record.agent, record.session_id.clone()))
            .collect();
        prop_assert_eq!(sessions, expected);
    }
}
