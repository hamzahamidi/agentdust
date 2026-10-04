use std::fs;
use std::path::{Path, PathBuf};

use agentdust_core::class::Class;
use agentdust_core::journal::Kind;
use agentdust_testkit::fixture::{Fixture, Label, Problem, ProblemKind, load, load_dir, parse_str};
use serde_json::{Value, json};

const FILE: &str = "fixtures/m1/background_server.json";

fn truth(label: &str, class: &str, never: bool) -> Value {
    json!({ "label": label, "class": class, "must_never_signal": never })
}

fn process(role: &str, parent: Option<&str>, expected: Value) -> Value {
    let mut value = json!({ "role": role, "flags": [], "expected": expected });
    if let Some(parent) = parent {
        value["parent_role"] = json!(parent);
    }
    value
}

fn managed(role: &str) -> Value {
    process(role, None, truth("true_managed", "managed", true))
}

fn owned_ended(role: &str, parent: &str) -> Value {
    process(
        role,
        Some(parent),
        truth("true_owned_ended", "owned-ended", false),
    )
}

fn fixture() -> Value {
    json!({
        "schema_version": 1,
        "name": "background_server",
        "description": "A dev server the agent left behind.",
        "agent": "claude",
        "processes": [managed("agent"), owned_ended("server", "agent")],
        "journal": [
            { "kind": "session_start", "session_id": "s1" },
            { "kind": "shell_start", "session_id": "s1", "tool_use_id": "t1" },
            { "kind": "sample", "session_id": "s1", "roles": ["server"] }
        ],
        "session_ended": true
    })
}

fn with_processes(processes: Value) -> Value {
    let mut value = fixture();
    value["processes"] = processes;
    value["journal"] = json!([{ "kind": "session_start", "session_id": "s1" }]);
    value
}

fn check(value: &Value) -> Result<Fixture, Vec<Problem>> {
    parse_str(Path::new(FILE), &value.to_string())
}

fn problems(value: &Value) -> Vec<Problem> {
    check(value).expect_err("the fixture should be refused")
}

fn only(value: &Value) -> Problem {
    let mut found = problems(value);
    assert_eq!(found.len(), 1, "{found:#?}");
    found.remove(0)
}

fn kinds_of(found: &[Problem], role: &str) -> Vec<ProblemKind> {
    found
        .iter()
        .filter(|problem| problem.role.as_deref() == Some(role))
        .map(|problem| problem.kind.clone())
        .collect()
}

fn shape_text(problem: &Problem) -> &str {
    match &problem.kind {
        ProblemKind::Shape(text) => text,
        other => panic!("expected a shape problem, found {other:?}"),
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agentdust-fixture-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_valid_fixture_loads_and_equals_the_strict_parse_of_the_same_text() {
    let loaded = check(&fixture()).unwrap();
    let strict: Fixture = serde_json::from_value(fixture()).unwrap();
    assert_eq!(loaded, strict);
}

#[test]
fn text_that_is_not_json_is_one_syntax_problem_for_the_file() {
    let found = parse_str(Path::new(FILE), "{ \"schema_version\": 1,").unwrap_err();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].file, Path::new(FILE));
    assert_eq!(found[0].role, None);
    assert!(matches!(found[0].kind, ProblemKind::Syntax(_)), "{found:?}");
}

#[test]
fn json_that_is_not_an_object_is_a_shape_problem() {
    for text in ["[]", "null", "7"] {
        let found = parse_str(Path::new(FILE), text).unwrap_err();
        assert_eq!(found.len(), 1, "{text}");
        assert!(
            matches!(found[0].kind, ProblemKind::Shape(_)),
            "{text}: {found:?}"
        );
    }
}

#[test]
fn a_version_other_than_one_is_refused_and_nothing_else_is_reported() {
    for version in [json!(2), json!(0), json!(1.0), json!("1"), json!(null), json!(-1)] {
        let mut value = fixture();
        value["schema_version"] = version.clone();
        value["processes"] = json!([{ "role": 5 }]);
        value["unexpected"] = json!(true);
        let problem = only(&value);
        assert_eq!(
            problem.kind,
            ProblemKind::UnsupportedVersion {
                found: version.to_string()
            }
        );
        assert_eq!(problem.role, None);
    }
}

#[test]
fn a_missing_version_is_a_shape_problem_that_names_the_field() {
    let mut value = fixture();
    value.as_object_mut().unwrap().remove("schema_version");
    let problem = only(&value);
    assert!(shape_text(&problem).contains("schema_version"));
}

#[test]
fn an_unknown_top_level_field_is_one_shape_problem_that_names_it() {
    let mut value = fixture();
    value["owner"] = json!("someone");
    let problem = only(&value);
    assert_eq!(problem.file, Path::new(FILE));
    assert_eq!(problem.role, None);
    assert!(shape_text(&problem).contains("owner"));
}

#[test]
fn a_process_without_a_label_is_reported_with_its_role() {
    let mut value = fixture();
    value["processes"][1]["expected"]
        .as_object_mut()
        .unwrap()
        .remove("label");
    let problem = only(&value);
    assert_eq!(problem.role.as_deref(), Some("server"));
    assert!(shape_text(&problem).contains("label"));
}

#[test]
fn a_process_without_an_expected_block_is_reported_with_its_role() {
    let mut value = fixture();
    value["processes"][1].as_object_mut().unwrap().remove("expected");
    let problem = only(&value);
    assert_eq!(problem.role.as_deref(), Some("server"));
    assert!(shape_text(&problem).contains("expected"));
}

#[test]
fn every_process_without_a_label_is_reported_not_only_the_first() {
    let value = with_processes(json!([
        managed("agent"),
        { "role": "one", "parent_role": "agent", "flags": [], "expected": {} },
        { "role": "two", "parent_role": "agent", "flags": [] },
        { "role": "three", "parent_role": "agent", "flags": [],
          "expected": { "class": "owned-ended", "must_never_signal": false } }
    ]));
    let found = problems(&value);
    assert_eq!(found.len(), 3, "{found:#?}");
    for role in ["one", "two", "three"] {
        assert_eq!(kinds_of(&found, role).len(), 1, "{role}");
    }
}

#[test]
fn an_unknown_field_inside_a_process_is_reported_with_its_role() {
    let mut value = fixture();
    value["processes"][1]["pid"] = json!(4242);
    let problem = only(&value);
    assert_eq!(problem.role.as_deref(), Some("server"));
    assert!(shape_text(&problem).contains("pid"));
}

#[test]
fn a_process_that_has_no_role_is_reported_by_its_position() {
    let mut value = fixture();
    value["journal"] = json!([]);
    value["processes"][1].as_object_mut().unwrap().remove("role");
    let problem = only(&value);
    assert_eq!(problem.role, None);
    assert!(shape_text(&problem).contains("processes[1]"));
}

#[test]
fn a_journal_event_that_does_not_parse_is_reported_by_its_position() {
    let mut value = fixture();
    value["journal"][1]["kind"] = json!("tool_start");
    let problem = only(&value);
    assert_eq!(problem.role, None);
    assert!(shape_text(&problem).contains("journal[1]"));
}

#[test]
fn a_process_that_failed_to_parse_still_counts_as_a_known_parent() {
    let mut value = fixture();
    value["processes"][0]["flags"] = json!("not a list");
    let found = problems(&value);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].role.as_deref(), Some("agent"));
    assert!(matches!(found[0].kind, ProblemKind::Shape(_)));
}

#[test]
fn a_duplicate_role_is_reported_once_with_its_count() {
    let value = with_processes(json!([
        managed("agent"),
        owned_ended("server", "agent"),
        owned_ended("server", "agent"),
        owned_ended("server", "agent"),
        managed("other"),
        managed("other")
    ]));
    let found = problems(&value);
    assert_eq!(
        kinds_of(&found, "server"),
        [ProblemKind::DuplicateRole { count: 3 }]
    );
    assert_eq!(
        kinds_of(&found, "other"),
        [ProblemKind::DuplicateRole { count: 2 }]
    );
    assert_eq!(found.len(), 2, "{found:#?}");
}

#[test]
fn a_parent_role_that_does_not_exist_is_reported_with_the_child_role() {
    let mut value = fixture();
    value["processes"][1]["parent_role"] = json!("ghost");
    let problem = only(&value);
    assert_eq!(problem.role.as_deref(), Some("server"));
    assert_eq!(
        problem.kind,
        ProblemKind::MissingParent {
            parent: "ghost".to_owned()
        }
    );
}

#[test]
fn every_missing_parent_is_reported() {
    let value = with_processes(json!([
        managed("agent"),
        owned_ended("a", "ghost"),
        owned_ended("b", "phantom"),
        owned_ended("c", "agent")
    ]));
    let found = problems(&value);
    assert_eq!(found.len(), 2, "{found:#?}");
    assert_eq!(kinds_of(&found, "a").len(), 1);
    assert_eq!(kinds_of(&found, "b").len(), 1);
}

#[test]
fn a_process_that_is_its_own_parent_is_a_cycle() {
    let value = with_processes(json!([managed("agent"), owned_ended("loop", "loop")]));
    let problem = only(&value);
    assert_eq!(problem.role.as_deref(), Some("loop"));
    assert_eq!(
        problem.kind,
        ProblemKind::Cycle {
            path: vec!["loop".to_owned(), "loop".to_owned()]
        }
    );
}

#[test]
fn a_cycle_is_reported_once_under_its_smallest_role_with_the_path() {
    let value = with_processes(json!([
        managed("agent"),
        owned_ended("c", "b"),
        owned_ended("b", "a"),
        owned_ended("a", "c"),
        owned_ended("tail", "c")
    ]));
    let found = problems(&value);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].role.as_deref(), Some("a"));
    assert_eq!(
        found[0].kind,
        ProblemKind::Cycle {
            path: ["a", "c", "b", "a"].map(str::to_owned).to_vec()
        }
    );
}

#[test]
fn two_separate_cycles_are_two_problems_and_a_missing_parent_still_shows() {
    let value = with_processes(json!([
        owned_ended("a", "b"),
        owned_ended("b", "a"),
        owned_ended("x", "y"),
        owned_ended("y", "x"),
        owned_ended("z", "ghost")
    ]));
    let found = problems(&value);
    assert_eq!(found.len(), 3, "{found:#?}");
    assert_eq!(kinds_of(&found, "a").len(), 1);
    assert_eq!(kinds_of(&found, "x").len(), 1);
    assert_eq!(kinds_of(&found, "z").len(), 1);
}

#[test]
fn a_class_that_contradicts_the_label_is_reported_for_every_pair() {
    for label in Label::ALL {
        for class in Class::ALL {
            if class == label.class() {
                continue;
            }
            let value = with_processes(json!([process(
                "agent",
                None,
                truth(label.as_str(), class.as_str(), label.must_never_signal())
            )]));
            let problem = only(&value);
            assert_eq!(problem.role.as_deref(), Some("agent"));
            assert_eq!(
                problem.kind,
                ProblemKind::ClassContradictsLabel {
                    label,
                    found: class,
                    expected: label.class()
                },
                "{label:?} with {class:?}"
            );
        }
    }
}

#[test]
fn a_must_never_signal_that_contradicts_the_label_is_reported_for_every_label() {
    for label in Label::ALL {
        let value = with_processes(json!([process(
            "agent",
            None,
            truth(label.as_str(), label.class().as_str(), !label.must_never_signal())
        )]));
        let problem = only(&value);
        assert_eq!(problem.role.as_deref(), Some("agent"));
        assert_eq!(
            problem.kind,
            ProblemKind::MustNeverSignalContradictsLabel {
                label,
                found: !label.must_never_signal(),
                expected: label.must_never_signal()
            }
        );
    }
}

#[test]
fn a_wrong_class_and_a_wrong_flag_on_one_process_are_two_problems() {
    let value = with_processes(json!([process(
        "agent",
        None,
        truth("true_detached", "owned-ended", true)
    )]));
    let found = problems(&value);
    assert_eq!(kinds_of(&found, "agent").len(), 2, "{found:#?}");
}

#[test]
fn flags_that_the_fixture_process_accepts_are_valid() {
    let mut value = fixture();
    value["processes"][1]["flags"] = json!(["--setsid", "--ignore-term", "--exit-after-ms", "1500", "20"]);
    assert!(check(&value).is_ok());
}

#[test]
fn flags_that_do_not_parse_are_reported_with_the_reason() {
    let cases: [Value; 5] = [
        json!(["--detach"]),
        json!(["--exit-after-ms"]),
        json!(["--exit-after-ms", "soon"]),
        json!(["--setsid", "--setsid"]),
        json!([""]),
    ];
    for flags in cases {
        let mut value = fixture();
        value["processes"][1]["flags"] = flags.clone();
        let problem = only(&value);
        assert_eq!(problem.role.as_deref(), Some("server"), "{flags}");
        assert!(
            matches!(problem.kind, ProblemKind::BadFlags(_)),
            "{flags}: {problem:?}"
        );
    }
}

#[test]
fn flags_the_harness_sets_itself_are_refused() {
    let cases: [(Value, &str); 3] = [
        (json!(["--spawn", "2"]), "--spawn"),
        (json!(["--report-file", "/tmp/r"]), "--report-file"),
        (
            json!(["--report-file", "/tmp/r", "--echo-env", "PATH"]),
            "--echo-env",
        ),
    ];
    for (flags, owned) in cases {
        let mut value = fixture();
        value["processes"][1]["flags"] = flags.clone();
        let found = problems(&value);
        assert!(
            kinds_of(&found, "server").contains(&ProblemKind::HarnessOwnedFlag(owned)),
            "{flags}: {found:#?}"
        );
    }
}

#[test]
fn record_only_is_allowed_for_managed_processes_only() {
    for label in Label::ALL {
        let mut record = process("agent", None, canonical(label));
        record["record_only"] = json!(true);
        let value = with_processes(json!([record]));
        if label == Label::TrueManaged {
            assert!(check(&value).is_ok(), "{label:?}");
        } else {
            let problem = only(&value);
            assert_eq!(problem.role.as_deref(), Some("agent"));
            assert_eq!(problem.kind, ProblemKind::RecordOnlyNotAllowed { label });
        }
    }
}

fn canonical(label: Label) -> Value {
    truth(label.as_str(), label.class().as_str(), label.must_never_signal())
}

#[test]
fn a_role_must_be_one_to_sixty_four_of_lowercase_letters_digits_and_underscores() {
    let bad = [
        String::new(),
        "Agent".to_owned(),
        "has space".to_owned(),
        "a-b".to_owned(),
        "dot.ted".to_owned(),
        "ünï".to_owned(),
        "tab\t".to_owned(),
        "x".repeat(65),
    ];
    for role in bad {
        let value = with_processes(json!([managed(&role)]));
        let problem = only(&value);
        assert_eq!(problem.role.as_deref(), Some(role.as_str()));
        assert_eq!(problem.kind, ProblemKind::BadRole, "{role:?}");
    }
    for role in ["a", "agent_2", "x".repeat(64).as_str(), "0"] {
        let value = with_processes(json!([managed(role)]));
        assert!(check(&value).is_ok(), "{role:?}");
    }
}

#[test]
fn roles_may_only_be_listed_on_a_sample_event() {
    for kind in [
        "session_start",
        "session_end",
        "shell_start",
        "shell_end",
        "server_start",
    ] {
        let mut value = fixture();
        value["journal"] = json!([
            { "kind": kind, "session_id": "s1", "tool_use_id": "t1", "roles": ["server"] }
        ]);
        let problem = only(&value);
        assert_eq!(problem.role, None);
        assert!(
            matches!(problem.kind, ProblemKind::EventRolesNotAllowed { index: 0, .. }),
            "{kind}: {problem:?}"
        );
    }
}

#[test]
fn a_sample_may_only_name_roles_of_the_fixture() {
    let mut value = fixture();
    value["journal"] = json!([
        { "kind": "sample", "session_id": "s1", "roles": ["server", "ghost", "phantom"] }
    ]);
    let found = problems(&value);
    assert_eq!(found.len(), 2, "{found:#?}");
    assert_eq!(
        found[0].kind,
        ProblemKind::EventUnknownRole {
            index: 0,
            role: "ghost".to_owned()
        }
    );
    assert_eq!(
        found[1].kind,
        ProblemKind::EventUnknownRole {
            index: 0,
            role: "phantom".to_owned()
        }
    );
}

#[test]
fn a_shell_event_needs_a_tool_use_id_and_other_events_do_not() {
    for (kind, event) in [(Kind::ShellStart, "shell_start"), (Kind::ShellEnd, "shell_end")] {
        let mut value = fixture();
        value["journal"] = json!([{ "kind": event, "session_id": "s1" }]);
        let problem = only(&value);
        assert_eq!(problem.kind, ProblemKind::EventNeedsToolUseId { index: 0, kind });
    }
    let mut value = fixture();
    value["journal"] = json!([
        { "kind": "session_start", "session_id": "s1" },
        { "kind": "session_end", "session_id": "s1" },
        { "kind": "server_start", "session_id": "s1" },
        { "kind": "sample", "session_id": "s1" }
    ]);
    assert!(check(&value).is_ok());
}

#[test]
fn every_problem_is_returned_at_once_with_the_file_and_the_role() {
    let mut value = fixture();
    value["processes"] = json!([
        managed("agent"),
        managed("agent"),
        owned_ended("lost", "ghost"),
        owned_ended("spin", "spin"),
        process("wrong", Some("agent"), truth("true_detached", "managed", true)),
        { "role": "bare", "flags": ["--detach"], "expected": {} },
        { "role": "Bad Role", "flags": [], "expected":
            { "label": "true_unknown", "class": "unknown", "must_never_signal": true } }
    ]);
    value["journal"] = json!([
        { "kind": "sample", "session_id": "s1", "roles": ["nowhere"] }
    ]);
    let found = problems(&value);
    assert!(found.iter().all(|problem| problem.file == Path::new(FILE)));
    assert_eq!(
        kinds_of(&found, "agent"),
        [ProblemKind::DuplicateRole { count: 2 }]
    );
    assert_eq!(kinds_of(&found, "lost").len(), 1);
    assert_eq!(kinds_of(&found, "spin").len(), 1);
    assert_eq!(kinds_of(&found, "wrong").len(), 2);
    assert_eq!(kinds_of(&found, "bare").len(), 1);
    assert_eq!(kinds_of(&found, "Bad Role"), [ProblemKind::BadRole]);
    assert!(
        found
            .iter()
            .any(|problem| matches!(problem.kind, ProblemKind::EventUnknownRole { index: 0, .. }))
    );
    assert_eq!(found.len(), 8, "{found:#?}");
}

#[test]
fn the_problems_come_back_in_the_same_order_every_time() {
    let value = with_processes(json!([
        owned_ended("b", "ghost"),
        owned_ended("a", "phantom"),
        managed("c"),
        managed("c")
    ]));
    assert_eq!(problems(&value), problems(&value));
}

#[test]
fn a_problem_prints_the_file_the_role_and_the_reason() {
    let problem = Problem {
        file: PathBuf::from("fixtures/m1/x.json"),
        role: Some("server".to_owned()),
        kind: ProblemKind::MissingParent {
            parent: "ghost".to_owned(),
        },
    };
    assert_eq!(
        problem.to_string(),
        "fixtures/m1/x.json: role server: parent_role \"ghost\" is not a role of this fixture"
    );
    let without_role = Problem {
        role: None,
        kind: ProblemKind::Syntax("eof".to_owned()),
        ..problem
    };
    assert_eq!(
        without_role.to_string(),
        "fixtures/m1/x.json: not valid JSON: eof"
    );
}

#[test]
fn a_label_prints_its_serialised_name() {
    for label in Label::ALL {
        assert_eq!(label.to_string(), label.as_str());
    }
}

#[test]
fn load_reads_a_file_and_checks_that_its_name_matches_the_file_name() {
    let dir = scratch("load");
    let good = dir.join("background_server.json");
    fs::write(&good, fixture().to_string()).unwrap();
    assert_eq!(load(&good).unwrap().name, "background_server");

    let renamed = dir.join("other_name.json");
    fs::write(&renamed, fixture().to_string()).unwrap();
    let found = load(&renamed).unwrap_err();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].file, renamed);
    assert_eq!(
        found[0].kind,
        ProblemKind::NameMismatch {
            name: "background_server".to_owned(),
            stem: "other_name".to_owned()
        }
    );
}

#[test]
fn load_reports_a_file_that_cannot_be_read() {
    let dir = scratch("missing");
    let path = dir.join("absent.json");
    let found = load(&path).unwrap_err();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].file, path);
    assert!(matches!(found[0].kind, ProblemKind::Read(_)), "{found:?}");
}

#[test]
fn load_reports_the_problems_of_the_file_alongside_a_name_mismatch() {
    let dir = scratch("both");
    let path = dir.join("renamed.json");
    let mut value = fixture();
    value["processes"][1]["parent_role"] = json!("ghost");
    fs::write(&path, value.to_string()).unwrap();
    let found = load(&path).unwrap_err();
    assert_eq!(found.len(), 2, "{found:#?}");
    assert!(found.iter().all(|problem| problem.file == path));
}

#[test]
fn load_dir_reads_the_json_files_in_name_order_and_skips_everything_else() {
    let dir = scratch("dir");
    for name in ["zeta", "alpha", "middle"] {
        let mut value = fixture();
        value["name"] = json!(name);
        fs::write(dir.join(format!("{name}.json")), value.to_string()).unwrap();
    }
    fs::write(dir.join("README.md"), "not a fixture").unwrap();
    fs::write(dir.join("notes.json.bak"), "not json").unwrap();
    fs::create_dir(dir.join("nested.json")).unwrap();
    let loaded = load_dir(&dir).unwrap();
    let names: Vec<&str> = loaded.iter().map(|item| item.fixture.name.as_str()).collect();
    assert_eq!(names, ["alpha", "middle", "zeta"]);
    assert_eq!(loaded[0].path, dir.join("alpha.json"));
}

#[test]
fn load_dir_returns_the_problems_of_every_bad_file_not_only_the_first() {
    let dir = scratch("dir_bad");
    let mut first = fixture();
    first["name"] = json!("first");
    first["processes"][1]["parent_role"] = json!("ghost");
    let mut second = fixture();
    second["name"] = json!("second");
    second["processes"][1]["parent_role"] = json!("phantom");
    let mut third = fixture();
    third["name"] = json!("third");
    fs::write(dir.join("first.json"), first.to_string()).unwrap();
    fs::write(dir.join("second.json"), second.to_string()).unwrap();
    fs::write(dir.join("third.json"), third.to_string()).unwrap();
    fs::write(dir.join("broken.json"), "{").unwrap();
    let found = load_dir(&dir).unwrap_err();
    assert_eq!(found.len(), 3, "{found:#?}");
    let files: Vec<&Path> = found.iter().map(|problem| problem.file.as_path()).collect();
    assert_eq!(
        files,
        [
            dir.join("broken.json"),
            dir.join("first.json"),
            dir.join("second.json")
        ]
    );
    assert_eq!(found[1].role.as_deref(), Some("server"));
}

#[test]
fn load_dir_of_an_empty_directory_is_empty_and_of_a_missing_one_is_a_problem() {
    let dir = scratch("dir_empty");
    assert!(load_dir(&dir).unwrap().is_empty());
    let found = load_dir(&dir.join("absent")).unwrap_err();
    assert_eq!(found.len(), 1);
    assert!(matches!(found[0].kind, ProblemKind::Read(_)));
}
