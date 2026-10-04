use std::path::Path;
use std::time::Duration;

use agentdust_testkit::fixture::{
    FIXTURE_SECONDS, Fixture, Group, Label, Member, ParentExpectation, Plan, PlanProblem, Structure,
    parse_str,
};
use agentdust_testkit::spec::{MAX_SPAWN, ProcSpec};
use serde_json::{Value, json};

struct P {
    role: String,
    parent: Option<String>,
    label: Label,
    flags: Vec<String>,
    record_only: bool,
}

fn p(role: &str, parent: Option<&str>, label: Label) -> P {
    P {
        role: role.to_owned(),
        parent: parent.map(str::to_owned),
        label,
        flags: Vec::new(),
        record_only: false,
    }
}

impl P {
    fn flags(mut self, flags: &[&str]) -> Self {
        self.flags = flags.iter().map(|flag| (*flag).to_owned()).collect();
        self
    }

    fn record_only(mut self) -> Self {
        self.record_only = true;
        self
    }

    fn json(&self) -> Value {
        let mut value = json!({
            "role": self.role,
            "flags": self.flags,
            "expected": {
                "label": self.label.as_str(),
                "class": self.label.class().as_str(),
                "must_never_signal": self.label.must_never_signal()
            }
        });
        if let Some(parent) = &self.parent {
            value["parent_role"] = json!(parent);
        }
        if self.record_only {
            value["record_only"] = json!(true);
        }
        value
    }
}

fn fixture(processes: Vec<P>, session_ended: bool) -> Fixture {
    let value = json!({
        "schema_version": 1,
        "name": "plan",
        "description": "A fixture for the plan tests.",
        "agent": "claude",
        "processes": processes.iter().map(P::json).collect::<Vec<_>>(),
        "journal": [],
        "session_ended": session_ended
    });
    parse_str(Path::new("plan.json"), &value.to_string()).expect("a valid fixture")
}

fn plan(processes: Vec<P>, session_ended: bool) -> Plan {
    fixture(processes, session_ended).plan().expect("a plan")
}

fn refused(processes: Vec<P>, session_ended: bool) -> Vec<PlanProblem> {
    fixture(processes, session_ended)
        .plan()
        .expect_err("the plan should be refused")
}

fn spec(flags: &[&str]) -> ProcSpec {
    let spec = ProcSpec::parse(flags.iter().copied()).unwrap();
    match spec.seconds {
        Some(_) => spec,
        None => spec.seconds(FIXTURE_SECONDS),
    }
}

fn member(role: &str, label: Label, flags: &[&str], parent: ParentExpectation, own: bool) -> Member {
    Member {
        role: role.to_owned(),
        label,
        spec: spec(flags),
        structure: Structure {
            parent,
            own_session: own,
        },
    }
}

#[test]
fn a_lone_root_is_one_group_started_by_the_harness() {
    let plan = plan(vec![p("lone", None, Label::TrueUnknown)], false);
    assert_eq!(
        plan,
        Plan {
            groups: vec![Group {
                parent: member("lone", Label::TrueUnknown, &[], ParentExpectation::Harness, false),
                children: vec![],
                ends: false
            }],
            record_only: vec![],
            end_session: false
        }
    );
}

#[test]
fn a_parent_and_its_children_are_one_group_in_fixture_order() {
    let plan = plan(
        vec![
            p("agent", None, Label::TrueManaged),
            p("first", Some("agent"), Label::TrueLiveOwned),
            p("second", Some("agent"), Label::TrueLiveOwned),
        ],
        false,
    );
    assert_eq!(plan.groups.len(), 1);
    let group = &plan.groups[0];
    assert_eq!(group.parent.role, "agent");
    let roles: Vec<&str> = group.children.iter().map(|m| m.role.as_str()).collect();
    assert_eq!(roles, ["first", "second"]);
    assert!(!group.ends);
}

#[test]
fn children_of_a_live_parent_expect_that_parent_and_those_of_an_ended_one_expect_launchd() {
    let processes = || {
        vec![
            p("agent", None, Label::TrueManaged),
            p("child", Some("agent"), Label::TrueUnknown),
        ]
    };
    let live = plan(processes(), false);
    assert_eq!(
        live.groups[0].children[0].structure.parent,
        ParentExpectation::Role("agent".to_owned())
    );
    assert!(!live.groups[0].ends);
    assert!(!live.end_session);

    let ended = plan(processes(), true);
    assert_eq!(
        ended.groups[0].children[0].structure.parent,
        ParentExpectation::Launchd
    );
    assert!(ended.groups[0].ends);
    assert!(ended.end_session);
}

#[test]
fn a_root_without_children_survives_the_end_of_the_session() {
    let plan = plan(
        vec![
            p("agent", None, Label::TrueManaged),
            p("child", Some("agent"), Label::TrueOwnedEnded),
            p("bystander", None, Label::TrueUnknown),
        ],
        true,
    );
    assert_eq!(plan.groups.len(), 2);
    assert!(plan.groups[0].ends);
    assert!(!plan.groups[1].ends);
    assert_eq!(plan.groups[1].parent.structure.parent, ParentExpectation::Harness);
}

#[test]
fn the_flags_become_the_spec_of_every_member_and_setsid_means_an_own_session() {
    let plan = plan(
        vec![
            p("agent", None, Label::TrueManaged).flags(&["--setsid", "--ignore-term"]),
            p("tunnel", Some("agent"), Label::TrueDetached).flags(&["--ignore-term", "--setsid"]),
        ],
        true,
    );
    let group = &plan.groups[0];
    let expected = spec(&["--setsid", "--ignore-term"]);
    assert_eq!(group.parent.spec, expected);
    assert_eq!(group.children[0].spec, expected);
    assert!(group.parent.structure.own_session);
    assert!(group.children[0].structure.own_session);
}

#[test]
fn without_setsid_no_member_expects_a_session_of_its_own() {
    let plan = plan(
        vec![
            p("agent", None, Label::TrueManaged),
            p("child", Some("agent"), Label::TrueOwnedEnded),
        ],
        true,
    );
    assert!(!plan.groups[0].parent.structure.own_session);
    assert!(!plan.groups[0].children[0].structure.own_session);
}

#[test]
fn record_only_processes_are_listed_and_never_started() {
    let plan = plan(
        vec![
            p("launchd_job", None, Label::TrueManaged).record_only(),
            p("agent", None, Label::TrueManaged),
            p("helper", None, Label::TrueManaged).record_only(),
        ],
        false,
    );
    assert_eq!(plan.record_only, ["launchd_job", "helper"]);
    assert_eq!(plan.groups.len(), 1);
    assert_eq!(plan.groups[0].parent.role, "agent");
}

#[test]
fn a_record_only_child_of_a_live_parent_is_not_started_either() {
    let plan = plan(
        vec![
            p("agent", None, Label::TrueManaged),
            p("helper", Some("agent"), Label::TrueManaged).record_only(),
        ],
        false,
    );
    assert_eq!(plan.record_only, ["helper"]);
    assert!(plan.groups[0].children.is_empty());
}

#[test]
fn a_fixture_with_no_processes_has_an_empty_plan() {
    let plan = plan(vec![], true);
    assert!(plan.groups.is_empty());
    assert!(plan.record_only.is_empty());
    assert!(plan.end_session);
}

#[test]
fn a_grandchild_cannot_be_started() {
    let found = refused(
        vec![
            p("agent", None, Label::TrueManaged),
            p("shell", Some("agent"), Label::TrueUnknown),
            p("server", Some("shell"), Label::TrueUnknown),
        ],
        false,
    );
    assert_eq!(
        found,
        [PlanProblem::TooDeep {
            role: "server".to_owned()
        }]
    );
}

#[test]
fn a_child_whose_flags_differ_from_its_parents_cannot_be_started() {
    let found = refused(
        vec![
            p("agent", None, Label::TrueManaged),
            p("same", Some("agent"), Label::TrueUnknown),
            p("other", Some("agent"), Label::TrueUnknown).flags(&["--ignore-term"]),
        ],
        false,
    );
    assert_eq!(
        found,
        [PlanProblem::FlagsDiffer {
            role: "other".to_owned(),
            parent: "agent".to_owned()
        }]
    );
}

#[test]
fn flags_are_compared_as_a_spec_so_their_order_does_not_matter() {
    let plan = plan(
        vec![
            p("agent", None, Label::TrueManaged).flags(&["--setsid", "--ignore-term"]),
            p("child", Some("agent"), Label::TrueUnknown).flags(&["--ignore-term", "--setsid"]),
        ],
        false,
    );
    assert_eq!(plan.groups[0].children.len(), 1);
}

#[test]
fn more_children_than_one_fixture_process_can_start_are_refused() {
    let mut processes = vec![p("agent", None, Label::TrueManaged)];
    processes
        .extend((0..=MAX_SPAWN).map(|index| p(&format!("child_{index}"), Some("agent"), Label::TrueUnknown)));
    let found = refused(processes, false);
    assert_eq!(
        found,
        [PlanProblem::TooManyChildren {
            parent: "agent".to_owned(),
            count: MAX_SPAWN + 1,
            max: MAX_SPAWN
        }]
    );
}

#[test]
fn exactly_the_maximum_number_of_children_is_accepted() {
    let mut processes = vec![p("agent", None, Label::TrueManaged)];
    processes
        .extend((0..MAX_SPAWN).map(|index| p(&format!("child_{index}"), Some("agent"), Label::TrueUnknown)));
    assert_eq!(plan(processes, false).groups[0].children.len(), MAX_SPAWN);
}

#[test]
fn a_live_child_of_a_record_only_parent_cannot_be_started() {
    let found = refused(
        vec![
            p("job", None, Label::TrueManaged).record_only(),
            p("child", Some("job"), Label::TrueUnknown),
        ],
        false,
    );
    assert_eq!(
        found,
        [PlanProblem::LiveChildOfRecordOnly {
            role: "child".to_owned(),
            parent: "job".to_owned()
        }]
    );
}

#[test]
fn an_owned_ended_process_must_be_the_child_of_a_parent_that_ended() {
    let live_parent = refused(
        vec![
            p("agent", None, Label::TrueManaged),
            p("child", Some("agent"), Label::TrueOwnedEnded),
        ],
        false,
    );
    assert_eq!(
        live_parent,
        [PlanProblem::MustBeOrphaned {
            role: "child".to_owned(),
            label: Label::TrueOwnedEnded
        }]
    );
    let no_parent = refused(vec![p("alone", None, Label::TrueOwnedEnded)], true);
    assert_eq!(
        no_parent,
        [PlanProblem::MustBeOrphaned {
            role: "alone".to_owned(),
            label: Label::TrueOwnedEnded
        }]
    );
}

#[test]
fn a_detached_process_must_be_orphaned_and_call_setsid() {
    let no_setsid = refused(
        vec![
            p("agent", None, Label::TrueManaged),
            p("tunnel", Some("agent"), Label::TrueDetached),
        ],
        true,
    );
    assert_eq!(
        no_setsid,
        [PlanProblem::MustHaveOwnSession {
            role: "tunnel".to_owned()
        }]
    );
    let live_parent = refused(
        vec![
            p("agent", None, Label::TrueManaged).flags(&["--setsid"]),
            p("tunnel", Some("agent"), Label::TrueDetached).flags(&["--setsid"]),
        ],
        false,
    );
    assert_eq!(
        live_parent,
        [PlanProblem::MustBeOrphaned {
            role: "tunnel".to_owned(),
            label: Label::TrueDetached
        }]
    );
}

#[test]
fn a_live_owned_process_must_be_the_child_of_a_parent_that_lives() {
    let ended_parent = refused(
        vec![
            p("agent", None, Label::TrueManaged),
            p("child", Some("agent"), Label::TrueLiveOwned),
        ],
        true,
    );
    assert_eq!(
        ended_parent,
        [PlanProblem::MustHaveLiveParent {
            role: "child".to_owned()
        }]
    );
    let no_parent = refused(vec![p("alone", None, Label::TrueLiveOwned)], false);
    assert_eq!(
        no_parent,
        [PlanProblem::MustHaveLiveParent {
            role: "alone".to_owned()
        }]
    );
}

#[test]
fn processes_that_are_unknown_or_managed_have_no_structure_to_prove() {
    for label in [Label::TrueUnknown, Label::TrueManaged] {
        for ended in [false, true] {
            let plan = plan(
                vec![
                    p("agent", None, Label::TrueManaged),
                    p("child", Some("agent"), label),
                    p("bystander", None, label),
                ],
                ended,
            );
            assert_eq!(plan.groups.len(), 2, "{label:?} {ended}");
        }
    }
}

#[test]
fn the_label_of_a_parent_that_ends_with_the_session_is_not_checked_against_the_structure() {
    let plan = plan(
        vec![
            p("agent", None, Label::TrueLiveOwned),
            p("child", Some("agent"), Label::TrueOwnedEnded),
        ],
        true,
    );
    assert!(plan.groups[0].ends);
}

#[test]
fn every_problem_of_the_plan_is_returned_at_once() {
    let found = refused(
        vec![
            p("agent", None, Label::TrueManaged),
            p("shell", Some("agent"), Label::TrueOwnedEnded),
            p("deep", Some("shell"), Label::TrueUnknown),
            p("odd", Some("agent"), Label::TrueUnknown).flags(&["--setsid"]),
            p("tunnel", Some("agent"), Label::TrueDetached),
        ],
        false,
    );
    assert!(found.contains(&PlanProblem::TooDeep {
        role: "deep".to_owned()
    }));
    assert!(found.contains(&PlanProblem::FlagsDiffer {
        role: "odd".to_owned(),
        parent: "agent".to_owned()
    }));
    assert!(found.contains(&PlanProblem::MustBeOrphaned {
        role: "shell".to_owned(),
        label: Label::TrueOwnedEnded
    }));
    assert!(found.contains(&PlanProblem::MustBeOrphaned {
        role: "tunnel".to_owned(),
        label: Label::TrueDetached
    }));
    assert!(found.contains(&PlanProblem::MustHaveOwnSession {
        role: "tunnel".to_owned()
    }));
}

#[test]
fn a_plan_is_the_same_every_time() {
    let processes = || {
        vec![
            p("agent", None, Label::TrueManaged),
            p("a", Some("agent"), Label::TrueOwnedEnded),
            p("b", Some("agent"), Label::TrueOwnedEnded),
            p("job", None, Label::TrueManaged).record_only(),
        ]
    };
    assert_eq!(plan(processes(), true), plan(processes(), true));
}

#[test]
fn a_process_is_found_by_role_and_children_come_back_in_order() {
    let fixture = fixture(
        vec![
            p("agent", None, Label::TrueManaged),
            p("b", Some("agent"), Label::TrueUnknown),
            p("a", Some("agent"), Label::TrueUnknown),
        ],
        false,
    );
    assert_eq!(fixture.process("a").unwrap().role, "a");
    assert!(fixture.process("ghost").is_none());
    let children: Vec<&str> = fixture
        .children_of("agent")
        .iter()
        .map(|process| process.role.as_str())
        .collect();
    assert_eq!(children, ["b", "a"]);
    assert!(fixture.children_of("a").is_empty());
}

#[test]
fn the_observed_processes_are_all_but_the_parents_the_session_end_removed() {
    let ended = fixture(
        vec![
            p("agent", None, Label::TrueManaged),
            p("child", Some("agent"), Label::TrueOwnedEnded),
            p("bystander", None, Label::TrueUnknown),
            p("job", None, Label::TrueManaged).record_only(),
        ],
        true,
    );
    let roles = |fixture: &Fixture| -> Vec<String> {
        fixture
            .observed()
            .iter()
            .map(|process| process.role.clone())
            .collect()
    };
    assert_eq!(roles(&ended), ["child", "bystander", "job"]);

    let live = fixture(
        vec![
            p("agent", None, Label::TrueManaged),
            p("child", Some("agent"), Label::TrueLiveOwned),
        ],
        false,
    );
    assert_eq!(roles(&live), ["agent", "child"]);
}

#[test]
fn a_record_only_parent_is_observed_and_never_ends_with_the_session() {
    let value = json!({
        "schema_version": 1,
        "name": "plan",
        "description": "A record only parent with a started child.",
        "agent": "claude",
        "processes": [
            p("job", None, Label::TrueManaged).record_only().json(),
            p("child", Some("job"), Label::TrueUnknown).json()
        ],
        "journal": [],
        "session_ended": true
    });
    let fixture = parse_str(Path::new("plan.json"), &value.to_string()).unwrap();
    let roles: Vec<&str> = fixture
        .observed()
        .iter()
        .map(|process| process.role.as_str())
        .collect();
    assert_eq!(roles, ["job", "child"]);
    assert!(!fixture.ends_with_session(fixture.process("job").unwrap()));
}

#[test]
fn a_process_without_a_lifetime_gets_the_fixture_lifetime() {
    let plan = plan(
        vec![
            p("agent", None, Label::TrueManaged),
            p("child", Some("agent"), Label::TrueUnknown),
            p("bystander", None, Label::TrueUnknown),
        ],
        false,
    );
    let members: Vec<&Member> = plan
        .groups
        .iter()
        .flat_map(|group| std::iter::once(&group.parent).chain(&group.children))
        .collect();
    assert_eq!(members.len(), 3);
    for member in members {
        assert_eq!(member.spec.seconds, Some(FIXTURE_SECONDS), "{}", member.role);
        assert_eq!(
            member.spec.lifetime(),
            Duration::from_secs(FIXTURE_SECONDS),
            "{}",
            member.role
        );
    }
}

#[test]
fn the_fixture_lifetime_is_two_minutes() {
    assert_eq!(FIXTURE_SECONDS, 120);
}

#[test]
fn a_lifetime_written_in_the_flags_is_kept() {
    let plan = plan(
        vec![
            p("agent", None, Label::TrueManaged).flags(&["20"]),
            p("child", Some("agent"), Label::TrueUnknown).flags(&["20"]),
        ],
        false,
    );
    let group = &plan.groups[0];
    assert_eq!(group.parent.spec.seconds, Some(20));
    assert_eq!(group.children[0].spec.seconds, Some(20));
}

#[test]
fn an_exit_time_shortens_the_lifetime_and_leaves_the_fixture_lifetime_beside_it() {
    let plan = plan(
        vec![
            p("agent", None, Label::TrueManaged).flags(&["--exit-after-ms", "5000"]),
            p("child", Some("agent"), Label::TrueUnknown).flags(&["--exit-after-ms", "5000"]),
        ],
        false,
    );
    let child = &plan.groups[0].children[0];
    assert_eq!(child.spec.seconds, Some(FIXTURE_SECONDS));
    assert_eq!(child.spec.exit_after_ms, Some(5000));
    assert_eq!(child.spec.lifetime(), Duration::from_secs(5));
}

#[test]
fn a_child_with_another_lifetime_than_its_parent_cannot_be_started() {
    let found = refused(
        vec![
            p("agent", None, Label::TrueManaged),
            p("short", Some("agent"), Label::TrueUnknown).flags(&["20"]),
        ],
        false,
    );
    assert_eq!(
        found,
        [PlanProblem::FlagsDiffer {
            role: "short".to_owned(),
            parent: "agent".to_owned()
        }]
    );
}

#[test]
fn flags_that_do_not_parse_are_a_plan_problem_and_not_replaced_by_defaults() {
    let value = json!({
        "schema_version": 1,
        "name": "plan",
        "description": "A fixture that never went through the loader.",
        "agent": "claude",
        "processes": [
            { "role": "agent", "flags": ["--detach"], "expected":
                { "label": "true_managed", "class": "managed", "must_never_signal": true } },
            { "role": "other", "flags": ["--exit-after-ms"], "expected":
                { "label": "true_unknown", "class": "unknown", "must_never_signal": true } }
        ],
        "journal": [],
        "session_ended": false
    });
    let fixture: Fixture = serde_json::from_value(value).unwrap();
    let found = fixture.plan().expect_err("the plan should be refused");
    assert_eq!(found.len(), 2, "{found:?}");
    match &found[0] {
        PlanProblem::BadFlags { role, reason } => {
            assert_eq!(role, "agent");
            assert!(reason.contains("--detach"), "{reason}");
        }
        other => panic!("expected bad flags, found {other:?}"),
    }
    assert!(matches!(&found[1], PlanProblem::BadFlags { role, .. } if role == "other"));
}
