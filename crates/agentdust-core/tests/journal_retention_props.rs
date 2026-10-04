use std::collections::{HashMap, HashSet};
use std::time::Duration;

use agentdust_core::journal::retention::{Entry, Plan, Policy, plan};
use agentdust_core::journal::{Agent, Kind, Record};
use proptest::prelude::*;

const CURRENT: &str = "current";

#[derive(Debug, Clone)]
struct Row {
    record: Record,
    bytes: u64,
    droppable: bool,
}

#[derive(Debug, Clone)]
struct Case {
    rows: Vec<Row>,
    now: u64,
    max_age: u64,
    max_bytes: u64,
}

type ScopeKey = (u8, String, String);

fn kind() -> impl Strategy<Value = Kind> {
    prop_oneof![
        Just(Kind::SessionStart),
        Just(Kind::SessionEnd),
        Just(Kind::ShellStart),
        Just(Kind::ShellEnd),
        Just(Kind::Sample),
        Just(Kind::ServerStart),
    ]
}

fn agent() -> impl Strategy<Value = Agent> {
    prop_oneof![Just(Agent::Claude), Just(Agent::Codex)]
}

fn row() -> impl Strategy<Value = Row> {
    (
        kind(),
        agent(),
        0usize..4,
        prop::bool::weighted(0.3),
        0u64..3_000,
        1u64..300,
        prop::bool::weighted(0.8),
    )
        .prop_map(
            |(kind, agent, session, old_boot, wall_ts, bytes, droppable)| Row {
                record: Record {
                    v: 1,
                    kind,
                    agent,
                    session_id: format!("s{session}"),
                    subagent_id: None,
                    tool_use_id: None,
                    wall_ts,
                    mono_ts: wall_ts,
                    boot: if old_boot { "old" } else { CURRENT }.to_owned(),
                    cwd_key: None,
                    exe_base: None,
                },
                bytes,
                droppable,
            },
        )
}

fn case() -> impl Strategy<Value = Case> {
    (
        prop::collection::vec(row(), 0..30),
        1_000u64..4_000,
        0u64..2_000,
        0u64..4_000,
    )
        .prop_map(|(rows, now, max_age, max_bytes)| Case {
            rows,
            now,
            max_age,
            max_bytes,
        })
}

fn run(rows: &[Row], now: u64, max_age: u64, max_bytes: u64) -> Plan {
    let entries: Vec<Entry<'_>> = rows
        .iter()
        .map(|row| Entry {
            record: &row.record,
            bytes: row.bytes,
            droppable: row.droppable,
        })
        .collect();
    let policy = Policy {
        ended_max_age: Duration::from_millis(max_age),
        max_bytes,
    };
    plan(&entries, &policy, now, CURRENT)
}

fn scope(row: &Row) -> ScopeKey {
    (
        row.record.agent as u8,
        row.record.session_id.clone(),
        row.record.boot.clone(),
    )
}

struct Scopes {
    ended: HashSet<ScopeKey>,
    newest: HashMap<ScopeKey, u64>,
}

fn scopes(rows: &[Row]) -> Scopes {
    let mut found = Scopes {
        ended: HashSet::new(),
        newest: HashMap::new(),
    };
    for row in rows {
        if row.record.kind == Kind::SessionEnd {
            found.ended.insert(scope(row));
        }
        let newest = found.newest.entry(scope(row)).or_default();
        *newest = (*newest).max(row.record.wall_ts);
    }
    found
}

fn kept_bytes(rows: &[Row], plan: &Plan) -> u64 {
    rows.iter()
        .zip(&plan.keep)
        .filter(|(_, keep)| **keep)
        .map(|(row, _)| row.bytes)
        .sum()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn planning_is_deterministic_and_counts_every_drop(case in case()) {
        let first = run(&case.rows, case.now, case.max_age, case.max_bytes);
        let second = run(&case.rows, case.now, case.max_age, case.max_bytes);
        prop_assert_eq!(&first, &second);
        prop_assert_eq!(first.keep.len(), case.rows.len());
        let dropped = first.keep.iter().filter(|keep| !**keep).count();
        prop_assert_eq!(
            dropped,
            first.dropped_earlier_boot + first.dropped_aged + first.dropped_over_cap + first.dropped_pinned
        );
    }

    #[test]
    fn a_record_that_cannot_be_dropped_is_always_kept(case in case()) {
        let result = run(&case.rows, case.now, case.max_age, case.max_bytes);
        for (row, keep) in case.rows.iter().zip(&result.keep) {
            prop_assert!(row.droppable || *keep);
        }
    }

    #[test]
    fn an_earlier_boot_record_that_can_be_dropped_never_survives(case in case()) {
        let result = run(&case.rows, case.now, case.max_age, case.max_bytes);
        for (row, keep) in case.rows.iter().zip(&result.keep) {
            prop_assert!(!(row.droppable && row.record.boot != CURRENT && *keep));
        }
    }

    #[test]
    fn the_kept_records_fit_the_cap_unless_only_held_records_remain(case in case()) {
        let result = run(&case.rows, case.now, case.max_age, case.max_bytes);
        let only_held = case
            .rows
            .iter()
            .zip(&result.keep)
            .all(|(row, keep)| !*keep || !row.droppable);
        prop_assert!(kept_bytes(&case.rows, &result) <= case.max_bytes || only_held);
    }

    #[test]
    fn an_aged_ended_record_is_dropped(case in case()) {
        let result = run(&case.rows, case.now, case.max_age, case.max_bytes);
        let found = scopes(&case.rows);
        for (row, keep) in case.rows.iter().zip(&result.keep) {
            let key = scope(row);
            let age = case.now.saturating_sub(found.newest[&key]);
            if row.droppable && row.record.boot == CURRENT && found.ended.contains(&key) && age > case.max_age {
                prop_assert!(!*keep);
            }
        }
    }

    #[test]
    fn a_young_or_pinned_record_is_kept_when_everything_fits(case in case()) {
        let total: u64 = case.rows.iter().map(|row| row.bytes).sum();
        let result = run(&case.rows, case.now, case.max_age, total.max(case.max_bytes));
        let found = scopes(&case.rows);
        for (row, keep) in case.rows.iter().zip(&result.keep) {
            let key = scope(row);
            let age = case.now.saturating_sub(found.newest[&key]);
            let ended = found.ended.contains(&key);
            if row.record.boot == CURRENT && (!ended || age <= case.max_age) {
                prop_assert!(*keep);
            }
        }
    }

    #[test]
    fn pinned_evidence_is_dropped_only_after_every_ended_record_and_with_a_report(case in case()) {
        let result = run(&case.rows, case.now, case.max_age, case.max_bytes);
        let found = scopes(&case.rows);
        let mut lost: HashMap<(u8, String), usize> = HashMap::new();
        let mut ended_survivor = false;
        for (row, keep) in case.rows.iter().zip(&result.keep) {
            let key = scope(row);
            if row.record.boot != CURRENT || !row.droppable {
                continue;
            }
            if found.ended.contains(&key) {
                ended_survivor |= *keep;
            } else if !*keep {
                *lost.entry((row.record.agent as u8, row.record.session_id.clone())).or_default() += 1;
            }
        }
        let reported: HashMap<(u8, String), usize> = result
            .degraded
            .iter()
            .map(|d| ((d.agent as u8, d.session_id.clone()), d.dropped_records))
            .collect();
        prop_assert_eq!(&lost, &reported);
        prop_assert_eq!(lost.values().sum::<usize>(), result.dropped_pinned);
        if result.dropped_pinned > 0 {
            prop_assert!(!ended_survivor);
        }
    }

    #[test]
    fn planning_what_was_kept_changes_nothing(case in case()) {
        let first = run(&case.rows, case.now, case.max_age, case.max_bytes);
        let kept: Vec<Row> = case
            .rows
            .iter()
            .zip(&first.keep)
            .filter(|(_, keep)| **keep)
            .map(|(row, _)| row.clone())
            .collect();
        let second = run(&kept, case.now, case.max_age, case.max_bytes);
        prop_assert!(second.keep.iter().all(|keep| *keep));
        prop_assert!(second.degraded.is_empty());
    }
}
