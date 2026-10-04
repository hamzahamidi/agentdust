use agentdust_core::class::{Actionability, Class, actionable_as};
use agentdust_core::journal::{Agent, Kind};
use agentdust_testkit::fixture::{Fixture, FixtureEvent, FixtureProcess, GroundTruth, Label, SCHEMA_VERSION};
use serde_json::{Value, json};

type Edit = fn(&mut Value);

const LABELS: [(Label, &str, Class, bool); 5] = [
    (
        Label::TrueOwnedEnded,
        "true_owned_ended",
        Class::OwnedEnded,
        false,
    ),
    (Label::TrueLiveOwned, "true_live_owned", Class::OwnedLive, true),
    (Label::TrueDetached, "true_detached", Class::Suspect, false),
    (Label::TrueManaged, "true_managed", Class::Managed, true),
    (Label::TrueUnknown, "true_unknown", Class::Unknown, true),
];

fn expected() -> Value {
    json!({ "label": "true_owned_ended", "class": "owned-ended", "must_never_signal": false })
}

fn fixture() -> Value {
    json!({
        "schema_version": 1,
        "name": "background_server",
        "description": "A dev server the agent left behind.",
        "agent": "claude",
        "processes": [
            { "role": "agent", "flags": [], "expected":
                { "label": "true_managed", "class": "managed", "must_never_signal": true } },
            { "role": "server", "parent_role": "agent", "flags": ["--setsid"],
              "record_only": false, "expected": expected() }
        ],
        "journal": [
            { "kind": "session_start", "session_id": "s1" },
            { "kind": "shell_start", "session_id": "s1", "subagent_id": "sub1",
              "tool_use_id": "t1", "exe_base": "node" },
            { "kind": "sample", "session_id": "s1", "roles": ["server"] }
        ],
        "session_ended": true
    })
}

fn parse(value: &Value) -> Result<Fixture, serde_json::Error> {
    serde_json::from_value(value.clone())
}

fn error_of(value: &Value) -> String {
    parse(value)
        .expect_err("the fixture should be refused")
        .to_string()
}

#[test]
fn the_schema_version_is_one() {
    assert_eq!(SCHEMA_VERSION, 1);
}

#[test]
fn the_five_labels_are_the_ones_in_spec_9_2_in_that_order() {
    let names: Vec<&str> = LABELS.iter().map(|(_, name, _, _)| *name).collect();
    assert_eq!(
        names,
        [
            "true_owned_ended",
            "true_live_owned",
            "true_detached",
            "true_managed",
            "true_unknown"
        ]
    );
    let listed: Vec<Label> = LABELS.iter().map(|(label, _, _, _)| *label).collect();
    assert_eq!(Label::ALL.to_vec(), listed);
}

#[test]
fn true_owned_ended_expects_owned_ended_and_may_be_signalled() {
    assert_eq!(Label::TrueOwnedEnded.class(), Class::OwnedEnded);
    assert!(!Label::TrueOwnedEnded.must_never_signal());
}

#[test]
fn true_live_owned_expects_owned_live_and_must_never_be_signalled() {
    assert_eq!(Label::TrueLiveOwned.class(), Class::OwnedLive);
    assert!(Label::TrueLiveOwned.must_never_signal());
}

#[test]
fn true_detached_expects_suspect_and_may_be_signalled_after_its_own_approval() {
    assert_eq!(Label::TrueDetached.class(), Class::Suspect);
    assert!(!Label::TrueDetached.must_never_signal());
}

#[test]
fn true_managed_expects_managed_and_must_never_be_signalled() {
    assert_eq!(Label::TrueManaged.class(), Class::Managed);
    assert!(Label::TrueManaged.must_never_signal());
}

#[test]
fn true_unknown_expects_unknown_and_must_never_be_signalled() {
    assert_eq!(Label::TrueUnknown.class(), Class::Unknown);
    assert!(Label::TrueUnknown.must_never_signal());
}

#[test]
fn must_never_signal_agrees_with_what_the_class_may_be_asked_to_do() {
    for label in Label::ALL {
        let signalled = matches!(
            actionable_as(label.class()),
            Actionability::BatchCode | Actionability::PerItemCode
        );
        assert_eq!(label.must_never_signal(), !signalled, "{label:?}");
    }
}

#[test]
fn labels_serialise_to_their_spec_names_and_back() {
    for (label, name, _, _) in LABELS {
        assert_eq!(serde_json::to_string(&label).unwrap(), format!("\"{name}\""));
        assert_eq!(
            serde_json::from_str::<Label>(&format!("\"{name}\"")).unwrap(),
            label
        );
        assert_eq!(label.as_str(), name);
    }
}

#[test]
fn a_name_that_is_not_a_label_is_refused() {
    for name in [
        "owned_ended",
        "true-owned-ended",
        "TrueOwnedEnded",
        "true_likely_owned",
        "",
    ] {
        assert!(
            serde_json::from_str::<Label>(&format!("\"{name}\"")).is_err(),
            "{name:?} was accepted"
        );
    }
}

#[test]
fn canonical_ground_truth_carries_the_class_and_flag_of_its_label() {
    for (label, _, class, never) in LABELS {
        let truth = GroundTruth::canonical(label);
        assert_eq!(truth.label, label);
        assert_eq!(truth.class, class);
        assert_eq!(truth.must_never_signal, never);
    }
}

#[test]
fn a_complete_fixture_parses_into_every_field() {
    let parsed = parse(&fixture()).unwrap();
    assert_eq!(parsed.schema_version, 1);
    assert_eq!(parsed.name, "background_server");
    assert_eq!(parsed.description, "A dev server the agent left behind.");
    assert_eq!(parsed.agent, Agent::Claude);
    assert!(parsed.session_ended);
    assert_eq!(parsed.processes.len(), 2);
    let server = &parsed.processes[1];
    assert_eq!(server.role, "server");
    assert_eq!(server.parent_role.as_deref(), Some("agent"));
    assert_eq!(server.flags, ["--setsid"]);
    assert!(!server.record_only);
    assert_eq!(server.expected, GroundTruth::canonical(Label::TrueOwnedEnded));
    assert_eq!(parsed.journal.len(), 3);
    let shell = &parsed.journal[1];
    assert_eq!(shell.kind, Kind::ShellStart);
    assert_eq!(shell.session_id, "s1");
    assert_eq!(shell.subagent_id.as_deref(), Some("sub1"));
    assert_eq!(shell.tool_use_id.as_deref(), Some("t1"));
    assert_eq!(shell.exe_base.as_ref().map(|base| base.as_str()), Some("node"));
    assert_eq!(parsed.journal[2].roles, ["server"]);
}

#[test]
fn optional_fields_default_to_absent() {
    let parsed = parse(&fixture()).unwrap();
    let agent: &FixtureProcess = &parsed.processes[0];
    assert_eq!(agent.parent_role, None);
    assert!(!agent.record_only);
    let start: &FixtureEvent = &parsed.journal[0];
    assert_eq!(start.subagent_id, None);
    assert_eq!(start.tool_use_id, None);
    assert_eq!(start.exe_base, None);
    assert!(start.roles.is_empty());
}

#[test]
fn a_fixture_serialises_without_its_absent_fields_and_parses_back_equal() {
    let parsed = parse(&fixture()).unwrap();
    let written = serde_json::to_value(&parsed).unwrap();
    assert!(written["processes"][0].get("parent_role").is_none());
    assert!(written["processes"][0].get("record_only").is_none());
    assert!(written["journal"][0].get("roles").is_none());
    assert!(written["journal"][0].get("exe_base").is_none());
    assert_eq!(parse(&written).unwrap(), parsed);
}

#[test]
fn an_empty_process_list_and_an_empty_journal_are_valid_shapes() {
    let mut value = fixture();
    value["processes"] = json!([]);
    value["journal"] = json!([]);
    let parsed = parse(&value).unwrap();
    assert!(parsed.processes.is_empty());
    assert!(parsed.journal.is_empty());
}

#[test]
fn an_unknown_field_is_refused_at_every_level() {
    let mut at_top = fixture();
    at_top["owner"] = json!("someone");
    let mut in_process = fixture();
    in_process["processes"][0]["pid"] = json!(1);
    let mut in_expected = fixture();
    in_expected["processes"][0]["expected"]["confidence"] = json!(0.9);
    let mut in_event = fixture();
    in_event["journal"][0]["cwd"] = json!("/tmp");
    for (value, field) in [
        (at_top, "owner"),
        (in_process, "pid"),
        (in_expected, "confidence"),
        (in_event, "cwd"),
    ] {
        let message = error_of(&value);
        assert!(message.contains("unknown field"), "{message}");
        assert!(message.contains(field), "{message}");
    }
}

#[test]
fn every_required_field_must_be_present() {
    let required: [(&[&str], &str); 13] = [
        (&["schema_version"], "schema_version"),
        (&["name"], "name"),
        (&["description"], "description"),
        (&["agent"], "agent"),
        (&["processes"], "processes"),
        (&["journal"], "journal"),
        (&["session_ended"], "session_ended"),
        (&["processes", "0", "role"], "role"),
        (&["processes", "0", "flags"], "flags"),
        (&["processes", "0", "expected"], "expected"),
        (&["processes", "0", "expected", "label"], "label"),
        (&["processes", "0", "expected", "class"], "class"),
        (&["journal", "0", "kind"], "kind"),
    ];
    for (path, field) in required {
        let mut value = fixture();
        let mut slot = &mut value;
        for key in &path[..path.len() - 1] {
            slot = match key.parse::<usize>() {
                Ok(index) => &mut slot[index],
                Err(_) => &mut slot[*key],
            };
        }
        slot.as_object_mut().unwrap().remove(path[path.len() - 1]);
        let message = error_of(&value);
        assert!(message.contains("missing field"), "{path:?}: {message}");
        assert!(message.contains(field), "{path:?}: {message}");
    }
}

#[test]
fn must_never_signal_and_the_session_id_of_an_event_are_required_too() {
    let mut no_flag = fixture();
    no_flag["processes"][0]["expected"]
        .as_object_mut()
        .unwrap()
        .remove("must_never_signal");
    assert!(error_of(&no_flag).contains("must_never_signal"));
    let mut no_session = fixture();
    no_session["journal"][0]
        .as_object_mut()
        .unwrap()
        .remove("session_id");
    assert!(error_of(&no_session).contains("session_id"));
}

#[test]
fn values_of_the_wrong_type_are_refused() {
    let edits: [(&str, Edit); 8] = [
        ("schema_version as text", |v| v["schema_version"] = json!("1")),
        ("name as number", |v| v["name"] = json!(7)),
        ("session_ended as text", |v| v["session_ended"] = json!("true")),
        ("flags as text", |v| {
            v["processes"][0]["flags"] = json!("--setsid")
        }),
        ("flag as number", |v| v["processes"][0]["flags"] = json!([1])),
        ("record_only as number", |v| {
            v["processes"][1]["record_only"] = json!(1)
        }),
        ("processes as object", |v| v["processes"] = json!({})),
        ("roles as text", |v| v["journal"][2]["roles"] = json!("server")),
    ];
    for (what, edit) in edits {
        let mut value = fixture();
        edit(&mut value);
        assert!(parse(&value).is_err(), "{what} was accepted");
    }
}

#[test]
fn an_unknown_agent_kind_or_event_kind_is_refused() {
    let mut agent = fixture();
    agent["agent"] = json!("windsurf");
    assert!(parse(&agent).is_err());
    let mut kind = fixture();
    kind["journal"][0]["kind"] = json!("tool_start");
    assert!(parse(&kind).is_err());
}

#[test]
fn an_event_exe_base_follows_the_journal_rule() {
    let mut control = fixture();
    control["journal"][1]["exe_base"] = json!("no\u{1b}[31mde");
    assert!(parse(&control).is_err());
    let mut long = fixture();
    long["journal"][1]["exe_base"] = json!("x".repeat(65));
    assert!(parse(&long).is_err());
}

#[test]
fn a_fixture_that_is_not_an_object_is_refused() {
    for text in ["[]", "null", "7", "\"fixture\""] {
        assert!(serde_json::from_str::<Fixture>(text).is_err(), "{text}");
    }
}
