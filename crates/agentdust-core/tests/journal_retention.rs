use std::time::Duration;

use agentdust_core::journal::retention::{Degraded, Entry, Plan, Policy, plan};
use agentdust_core::journal::{Agent, Kind, Record};

const NOW: u64 = 1_000_000;
const LIMIT: u64 = 1_000;
const CURRENT: &str = "current";

struct Row {
    record: Record,
    bytes: u64,
    droppable: bool,
}

fn row(kind: Kind, session: &str, wall_ts: u64) -> Row {
    Row {
        record: Record {
            v: 1,
            kind,
            agent: Agent::Claude,
            session_id: session.to_owned(),
            subagent_id: None,
            tool_use_id: None,
            wall_ts,
            mono_ts: wall_ts,
            boot: CURRENT.to_owned(),
            cwd_key: None,
            exe_base: None,
        },
        bytes: 100,
        droppable: true,
    }
}

impl Row {
    fn boot(mut self, boot: &str) -> Self {
        self.record.boot = boot.to_owned();
        self
    }

    fn agent(mut self, agent: Agent) -> Self {
        self.record.agent = agent;
        self
    }

    fn sub(mut self, id: &str) -> Self {
        self.record.subagent_id = Some(id.to_owned());
        self
    }

    fn bytes(mut self, bytes: u64) -> Self {
        self.bytes = bytes;
        self
    }

    fn fixed(mut self) -> Self {
        self.droppable = false;
        self
    }
}

fn policy(max_age_ms: u64, max_bytes: u64) -> Policy {
    Policy {
        ended_max_age: Duration::from_millis(max_age_ms),
        max_bytes,
    }
}

fn run(rows: &[Row], policy: &Policy) -> Plan {
    let entries: Vec<Entry<'_>> = rows
        .iter()
        .map(|row| Entry {
            record: &row.record,
            bytes: row.bytes,
            droppable: row.droppable,
        })
        .collect();
    plan(&entries, policy, NOW, CURRENT)
}

fn roomy() -> Policy {
    policy(LIMIT, u64::MAX)
}

fn degraded(agent: Agent, session: &str, dropped_records: usize) -> Degraded {
    Degraded {
        agent,
        session_id: session.to_owned(),
        dropped_records,
    }
}

#[test]
fn the_default_limits_are_the_ones_in_the_spec() {
    assert_eq!(
        Policy::default(),
        Policy {
            ended_max_age: Duration::from_secs(14 * 24 * 60 * 60),
            max_bytes: 20_000_000,
        }
    );
}

#[test]
fn an_empty_journal_plans_nothing() {
    let result = run(&[], &roomy());
    assert!(result.keep.is_empty());
    assert_eq!(result.dropped_earlier_boot + result.dropped_aged, 0);
    assert_eq!(result.dropped_over_cap + result.dropped_pinned, 0);
    assert!(result.degraded.is_empty());
}

#[test]
fn young_records_under_the_cap_are_all_kept() {
    let rows = [
        row(Kind::SessionStart, "a", NOW - 50),
        row(Kind::SessionEnd, "a", NOW - 40),
        row(Kind::ShellStart, "b", NOW - 30),
    ];
    let result = run(&rows, &roomy());
    assert_eq!(result.keep, [true, true, true]);
    assert!(result.degraded.is_empty());
}

#[test]
fn the_plan_has_one_decision_per_entry_and_the_counts_add_up() {
    let rows = [
        row(Kind::ShellStart, "a", NOW - 5).boot("old"),
        row(Kind::SessionEnd, "b", NOW - 9_000),
        row(Kind::ShellStart, "c", NOW - 5),
    ];
    let result = run(&rows, &roomy());
    assert_eq!(result.keep.len(), rows.len());
    let dropped = result.keep.iter().filter(|keep| !**keep).count();
    assert_eq!(
        dropped,
        result.dropped_earlier_boot + result.dropped_aged + result.dropped_over_cap + result.dropped_pinned
    );
}

#[test]
fn records_of_an_earlier_boot_are_dropped_whatever_their_age_or_state() {
    let rows = [
        row(Kind::ShellStart, "a", NOW - 5).boot("old"),
        row(Kind::SessionEnd, "b", NOW - 5).boot("older"),
        row(Kind::ShellStart, "c", NOW - 5),
    ];
    let result = run(&rows, &roomy());
    assert_eq!(result.keep, [false, false, true]);
    assert_eq!(result.dropped_earlier_boot, 2);
}

#[test]
fn an_earlier_boot_record_that_cannot_be_dropped_is_kept() {
    let rows = [row(Kind::ShellStart, "a", NOW - 5).boot("old").fixed()];
    let result = run(&rows, &roomy());
    assert_eq!(result.keep, [true]);
    assert_eq!(result.dropped_earlier_boot, 0);
}

#[test]
fn an_ended_session_older_than_the_limit_is_dropped_whole() {
    let rows = [
        row(Kind::SessionStart, "a", NOW - 5_000),
        row(Kind::ShellStart, "a", NOW - 4_000),
        row(Kind::SessionEnd, "a", NOW - 3_000),
        row(Kind::SessionStart, "b", NOW - 10),
    ];
    let result = run(&rows, &roomy());
    assert_eq!(result.keep, [false, false, false, true]);
    assert_eq!(result.dropped_aged, 3);
}

#[test]
fn the_age_limit_is_exclusive() {
    let rows = [
        row(Kind::SessionEnd, "at-limit", NOW - LIMIT),
        row(Kind::SessionEnd, "past-limit", NOW - LIMIT - 1),
    ];
    let result = run(&rows, &roomy());
    assert_eq!(result.keep, [true, false]);
}

#[test]
fn a_zero_age_limit_drops_an_ended_session_older_than_now_and_keeps_one_from_now() {
    let rows = [
        row(Kind::SessionEnd, "now", NOW),
        row(Kind::SessionEnd, "before", NOW - 1),
    ];
    let result = run(&rows, &policy(0, u64::MAX));
    assert_eq!(result.keep, [true, false]);
}

#[test]
fn age_runs_from_the_newest_record_of_the_session() {
    let rows = [
        row(Kind::SessionStart, "a", NOW - 5_000),
        row(Kind::SessionEnd, "a", NOW - 4_000),
        row(Kind::ShellEnd, "a", NOW - 10),
    ];
    let result = run(&rows, &roomy());
    assert_eq!(result.keep, [true, true, true]);
}

#[test]
fn a_session_without_an_end_record_is_pinned_however_old() {
    let rows = [
        row(Kind::SessionStart, "a", NOW - 900_000),
        row(Kind::ShellStart, "a", NOW - 800_000),
    ];
    let result = run(&rows, &roomy());
    assert_eq!(result.keep, [true, true]);
    assert!(result.degraded.is_empty());
}

#[test]
fn a_timestamp_in_the_future_has_no_age() {
    let rows = [row(Kind::SessionEnd, "a", NOW + 5_000_000)];
    let result = run(&rows, &roomy());
    assert_eq!(result.keep, [true]);
}

#[test]
fn a_subagent_record_belongs_to_its_parent_session() {
    let fresh = [
        row(Kind::SessionStart, "a", NOW - 5_000),
        row(Kind::SessionEnd, "a", NOW - 4_000),
        row(Kind::ShellStart, "a", NOW - 10).sub("agent-1"),
    ];
    assert_eq!(run(&fresh, &roomy()).keep, [true, true, true]);
    let stale = [
        row(Kind::SessionStart, "a", NOW - 5_000),
        row(Kind::SessionEnd, "a", NOW - 4_000),
        row(Kind::ShellStart, "a", NOW - 4_500).sub("agent-1"),
    ];
    assert_eq!(run(&stale, &roomy()).keep, [false, false, false]);
}

#[test]
fn the_same_session_id_under_two_agents_is_two_sessions() {
    let rows = [
        row(Kind::SessionEnd, "s", NOW - 5_000),
        row(Kind::ShellStart, "s", NOW - 5_000).agent(Agent::Codex),
    ];
    let result = run(&rows, &roomy());
    assert_eq!(result.keep, [false, true]);
}

#[test]
fn a_session_is_judged_separately_in_each_boot() {
    let rows = [
        row(Kind::SessionStart, "s", NOW - 85_000).boot("old"),
        row(Kind::SessionEnd, "s", NOW - 80_000).boot("old"),
        row(Kind::SessionStart, "s", NOW - 90_000),
    ];
    let result = run(&rows, &roomy());
    assert_eq!(result.keep, [false, false, true]);
    assert_eq!(result.dropped_earlier_boot, 2);
}

#[test]
fn the_end_record_may_sit_in_a_file_that_cannot_be_dropped() {
    let rows = [
        row(Kind::SessionStart, "a", NOW - 5_000),
        row(Kind::SessionEnd, "a", NOW - 5_000).fixed(),
    ];
    let result = run(&rows, &roomy());
    assert_eq!(result.keep, [false, true]);
}

#[test]
fn over_the_cap_the_oldest_ended_session_goes_first() {
    let rows = [
        row(Kind::SessionStart, "e1", NOW - 300),
        row(Kind::SessionEnd, "e1", NOW - 300),
        row(Kind::SessionStart, "e2", NOW - 200),
        row(Kind::SessionEnd, "e2", NOW - 200),
        row(Kind::SessionStart, "e3", NOW - 100),
        row(Kind::SessionEnd, "e3", NOW - 100),
    ];
    let result = run(&rows, &policy(LIMIT, 450));
    assert_eq!(result.keep, [false, false, true, true, true, true]);
    assert_eq!(result.dropped_over_cap, 2);
    assert!(result.degraded.is_empty());
}

#[test]
fn an_ended_session_goes_whole_and_not_record_by_record() {
    let rows = [
        row(Kind::SessionStart, "old", NOW - 300),
        row(Kind::ShellStart, "old", NOW - 290),
        row(Kind::SessionEnd, "old", NOW - 280),
        row(Kind::SessionEnd, "new", NOW - 100),
    ];
    let result = run(&rows, &policy(LIMIT, 350));
    assert_eq!(result.keep, [false, false, false, true]);
}

#[test]
fn a_pinned_session_survives_while_an_ended_one_can_still_go() {
    let rows = [
        row(Kind::ShellStart, "pinned", NOW - 9_000),
        row(Kind::SessionEnd, "ended", NOW - 100),
    ];
    let result = run(&rows, &policy(LIMIT, 100));
    assert_eq!(result.keep, [true, false]);
    assert!(result.degraded.is_empty());
}

#[test]
fn pinned_evidence_is_dropped_oldest_first_keeps_the_start_and_is_reported() {
    let rows = [
        row(Kind::SessionStart, "p", NOW - 500),
        row(Kind::ShellStart, "p", NOW - 400),
        row(Kind::ShellEnd, "p", NOW - 300),
        row(Kind::ShellStart, "p", NOW - 200),
        row(Kind::ShellEnd, "p", NOW - 100),
    ];
    let result = run(&rows, &policy(LIMIT, 300));
    assert_eq!(result.keep, [true, false, false, true, true]);
    assert_eq!(result.dropped_pinned, 2);
    assert_eq!(result.degraded, [degraded(Agent::Claude, "p", 2)]);
}

#[test]
fn the_start_of_a_pinned_session_goes_last() {
    let rows = [
        row(Kind::SessionStart, "p", NOW - 500),
        row(Kind::ShellStart, "p", NOW - 100),
    ];
    let one = run(&rows, &policy(LIMIT, 100));
    assert_eq!(one.keep, [true, false]);
    let none = run(&rows, &policy(LIMIT, 0));
    assert_eq!(none.keep, [false, false]);
    assert_eq!(none.degraded, [degraded(Agent::Claude, "p", 2)]);
}

#[test]
fn the_least_recently_active_pinned_session_loses_evidence_first() {
    let rows = [
        row(Kind::ShellStart, "quiet", NOW - 900),
        row(Kind::ShellStart, "quiet", NOW - 800),
        row(Kind::ShellStart, "busy", NOW - 100),
        row(Kind::ShellStart, "busy", NOW - 50),
    ];
    let result = run(&rows, &policy(LIMIT, 300));
    assert_eq!(result.keep, [false, true, true, true]);
    assert_eq!(result.degraded, [degraded(Agent::Claude, "quiet", 1)]);
}

#[test]
fn each_degraded_session_is_listed_once_in_a_stable_order() {
    let rows = [
        row(Kind::ShellStart, "b", NOW - 900),
        row(Kind::ShellStart, "b", NOW - 890),
        row(Kind::ShellStart, "a", NOW - 900),
        row(Kind::ShellStart, "a", NOW - 890),
        row(Kind::ShellStart, "x", NOW - 900).agent(Agent::Codex),
    ];
    let result = run(&rows, &policy(LIMIT, 0));
    assert_eq!(
        result.degraded,
        [
            degraded(Agent::Claude, "a", 2),
            degraded(Agent::Claude, "b", 2),
            degraded(Agent::Codex, "x", 1),
        ]
    );
}

#[test]
fn records_that_cannot_be_dropped_count_toward_the_cap_and_are_never_dropped() {
    let rows = [
        row(Kind::SessionEnd, "ended", NOW - 100).bytes(200),
        row(Kind::ShellStart, "pinned", NOW - 100),
        row(Kind::ShellStart, "held", NOW - 100).bytes(300).fixed(),
    ];
    let some = run(&rows, &policy(LIMIT, 350));
    assert_eq!(some.keep, [false, false, true]);
    assert_eq!(some.degraded, [degraded(Agent::Claude, "pinned", 1)]);
    let all = run(&rows, &policy(LIMIT, 100));
    assert_eq!(all.keep, [false, false, true]);
    assert_eq!(all.degraded, [degraded(Agent::Claude, "pinned", 1)]);
}

#[test]
fn nothing_is_dropped_for_the_cap_when_every_record_fits() {
    let rows = [
        row(Kind::SessionEnd, "a", NOW - 100),
        row(Kind::ShellStart, "b", NOW - 100),
    ];
    let result = run(&rows, &policy(LIMIT, 200));
    assert_eq!(result.keep, [true, true]);
}

#[test]
fn a_cap_of_zero_drops_everything_that_can_be_dropped() {
    let rows = [
        row(Kind::SessionEnd, "a", NOW - 100),
        row(Kind::ShellStart, "b", NOW - 100),
        row(Kind::ShellStart, "c", NOW - 100).fixed(),
    ];
    let result = run(&rows, &policy(LIMIT, 0));
    assert_eq!(result.keep, [false, false, true]);
}
