use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use agentdust_core::class::{Actionability, Class, actionable_as};
use agentdust_core::journal::Kind;
use agentdust_testkit::fixture::{Fixture, Label, Loaded, ParentExpectation, load_dir};
use agentdust_testkit::spec::ProcSpec;

const MINIMUM_FIXTURES: usize = 14;
const MINIMUM_PER_LABEL: usize = 2;
const MINIMUM_EXIT_AFTER_MS: u64 = 5_000;
const LONGEST_WAIT: Duration = Duration::from_secs(60);

const PROTECTED: [(&str, bool); 6] = [
    ("system_process_pid_one", true),
    ("other_user_process", true),
    ("agent_process_is_protected", false),
    ("launchd_job_lookalike", true),
    ("homebrew_service_lookalike", true),
    ("app_helper_process", true),
];

const EDGE_CASES: [&str; 5] = [
    "resumed_session_leftovers",
    "subagent_leftover_after_session_end",
    "session_end_without_descendants",
    "abrupt_termination_no_session_end",
    "session_end_event_agent_still_alive",
];

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/m1")
}

fn corpus() -> Vec<Loaded> {
    match load_dir(&corpus_dir()) {
        Ok(loaded) => loaded,
        Err(problems) => {
            let lines: Vec<String> = problems.iter().map(ToString::to_string).collect();
            panic!("the corpus does not load:\n{}", lines.join("\n"));
        }
    }
}

fn named<'a>(corpus: &'a [Loaded], name: &str) -> &'a Fixture {
    corpus
        .iter()
        .map(|loaded| &loaded.fixture)
        .find(|fixture| fixture.name == name)
        .unwrap_or_else(|| panic!("the corpus has no fixture named {name}"))
}

fn kinds(fixture: &Fixture, kind: Kind) -> usize {
    fixture.journal.iter().filter(|event| event.kind == kind).count()
}

fn observed_labels(fixture: &Fixture) -> HashSet<Label> {
    fixture
        .observed()
        .iter()
        .map(|process| process.expected.label)
        .collect()
}

#[test]
fn the_corpus_directory_loads_without_a_single_problem() {
    let loaded = corpus();
    assert!(!loaded.is_empty());
}

#[test]
fn the_corpus_has_at_least_fourteen_fixtures() {
    let loaded = corpus();
    assert!(
        loaded.len() >= MINIMUM_FIXTURES,
        "{} fixtures, need {MINIMUM_FIXTURES}",
        loaded.len()
    );
}

#[test]
fn every_label_is_observed_in_at_least_two_fixtures() {
    let loaded = corpus();
    let mut fixtures_per_label: BTreeMap<&str, usize> = BTreeMap::new();
    for item in &loaded {
        for label in observed_labels(&item.fixture) {
            *fixtures_per_label.entry(label.as_str()).or_default() += 1;
        }
    }
    for label in Label::ALL {
        let count = fixtures_per_label.get(label.as_str()).copied().unwrap_or(0);
        assert!(
            count >= MINIMUM_PER_LABEL,
            "{label} is observed in {count} fixtures: {fixtures_per_label:?}"
        );
    }
}

#[test]
fn every_fixture_can_be_planned_for_the_harness() {
    for item in corpus() {
        if let Err(problems) = item.fixture.plan() {
            let lines: Vec<String> = problems.iter().map(ToString::to_string).collect();
            panic!("{}: {}", item.path.display(), lines.join("; "));
        }
    }
}

#[test]
fn the_protected_cases_are_present_and_every_observed_process_in_them_is_managed() {
    let loaded = corpus();
    for (name, record_only) in PROTECTED {
        let fixture = named(&loaded, name);
        let observed = fixture.observed();
        assert!(!observed.is_empty(), "{name} has nothing to observe");
        for process in observed {
            assert_eq!(
                process.expected.label,
                Label::TrueManaged,
                "{name}/{}",
                process.role
            );
            assert_eq!(process.expected.class, Class::Managed, "{name}/{}", process.role);
            assert!(process.expected.must_never_signal, "{name}/{}", process.role);
            assert_eq!(process.record_only, record_only, "{name}/{}", process.role);
        }
    }
}

#[test]
fn the_other_user_case_and_the_pid_one_case_each_stand_alone_in_their_fixture() {
    let loaded = corpus();
    for name in ["system_process_pid_one", "other_user_process"] {
        let fixture = named(&loaded, name);
        assert_eq!(fixture.processes.len(), 1, "{name}");
        assert!(fixture.processes[0].parent_role.is_none(), "{name}");
        assert!(fixture.journal.is_empty(), "{name}");
    }
}

#[test]
fn every_edge_case_is_present() {
    let loaded = corpus();
    for name in EDGE_CASES {
        named(&loaded, name);
    }
}

#[test]
fn a_resumed_session_has_two_starts_for_one_session_and_a_leftover_from_each() {
    let loaded = corpus();
    let fixture = named(&loaded, "resumed_session_leftovers");
    let starts: Vec<&str> = fixture
        .journal
        .iter()
        .filter(|event| event.kind == Kind::SessionStart)
        .map(|event| event.session_id.as_str())
        .collect();
    assert_eq!(starts.len(), 2);
    assert_eq!(starts[0], starts[1]);
    assert!(fixture.session_ended);
    let leftovers = fixture
        .observed()
        .iter()
        .filter(|process| process.expected.label == Label::TrueOwnedEnded)
        .count();
    assert_eq!(leftovers, 2);
}

#[test]
fn a_subagent_case_journals_a_subagent_and_leaves_an_owned_ended_process() {
    let loaded = corpus();
    let fixture = named(&loaded, "subagent_leftover_after_session_end");
    assert!(fixture.journal.iter().any(|event| event.subagent_id.is_some()));
    assert!(fixture.session_ended);
    assert!(observed_labels(fixture).contains(&Label::TrueOwnedEnded));
}

#[test]
fn a_session_that_ended_without_descendants_has_nothing_to_observe() {
    let loaded = corpus();
    let fixture = named(&loaded, "session_end_without_descendants");
    assert_eq!(kinds(fixture, Kind::SessionStart), 1);
    assert_eq!(kinds(fixture, Kind::SessionEnd), 1);
    assert!(fixture.session_ended);
    assert!(fixture.processes.is_empty());
}

#[test]
fn an_abrupt_termination_has_no_session_end_and_an_unpaired_shell_call() {
    let loaded = corpus();
    let fixture = named(&loaded, "abrupt_termination_no_session_end");
    assert!(fixture.session_ended);
    assert_eq!(kinds(fixture, Kind::SessionEnd), 0);
    assert!(kinds(fixture, Kind::ShellStart) > kinds(fixture, Kind::ShellEnd));
    let labels: Vec<Label> = fixture
        .observed()
        .iter()
        .map(|process| process.expected.label)
        .collect();
    assert_eq!(labels, [Label::TrueOwnedEnded]);
}

#[test]
fn an_unknown_process_always_has_a_parent_that_is_alive_and_is_not_launchd() {
    let mut checked = 0;
    for item in corpus() {
        let plan = item.fixture.plan().unwrap();
        for group in &plan.groups {
            let observed = (!group.ends).then_some(&group.parent);
            for member in observed.into_iter().chain(&group.children) {
                if member.label == Label::TrueUnknown {
                    assert_ne!(
                        member.structure.parent,
                        ParentExpectation::Launchd,
                        "{}/{}",
                        item.fixture.name,
                        member.role
                    );
                    checked += 1;
                }
            }
        }
    }
    assert!(checked >= 3, "{checked} unknown processes");
}

#[test]
fn a_live_launcher_chain_has_two_unknown_processes_one_under_the_other() {
    let loaded = corpus();
    let fixture = named(&loaded, "unrelated_process_with_live_launcher");
    assert!(!fixture.session_ended);
    assert_eq!(fixture.processes.len(), 2);
    assert!(
        fixture
            .processes
            .iter()
            .all(|process| process.expected.label == Label::TrueUnknown)
    );
    let child = fixture
        .processes
        .iter()
        .find(|process| process.parent_role.is_some())
        .unwrap();
    assert!(fixture.process(child.parent_role.as_deref().unwrap()).is_some());
}

#[test]
fn a_session_end_event_does_not_end_the_agent_in_the_clear_case() {
    let loaded = corpus();
    let fixture = named(&loaded, "session_end_event_agent_still_alive");
    assert!(kinds(fixture, Kind::SessionEnd) >= 1);
    assert!(!fixture.session_ended);
    let labels = observed_labels(fixture);
    assert!(labels.contains(&Label::TrueLiveOwned));
    assert!(!labels.contains(&Label::TrueOwnedEnded));
}

#[test]
fn a_process_that_must_never_be_signalled_is_never_expected_to_be_actionable() {
    for item in corpus() {
        for process in item.fixture.observed() {
            if process.expected.must_never_signal {
                assert!(
                    matches!(
                        actionable_as(process.expected.class),
                        Actionability::Never | Actionability::ReportOnly
                    ),
                    "{}/{}",
                    item.fixture.name,
                    process.role
                );
            }
        }
    }
}

#[test]
fn only_true_owned_ended_processes_are_expected_to_be_owned_ended() {
    for item in corpus() {
        for process in &item.fixture.processes {
            assert_eq!(
                process.expected.class == Class::OwnedEnded,
                process.expected.label == Label::TrueOwnedEnded,
                "{}/{}",
                item.fixture.name,
                process.role
            );
        }
    }
}

#[test]
fn no_unknown_or_managed_process_is_expected_to_be_actionable() {
    for item in corpus() {
        for process in &item.fixture.processes {
            if matches!(process.expected.label, Label::TrueManaged | Label::TrueUnknown) {
                assert_eq!(
                    actionable_as(process.expected.class),
                    Actionability::Never,
                    "{}/{}",
                    item.fixture.name,
                    process.role
                );
            }
        }
    }
}

#[test]
fn the_corpus_covers_sigterm_ignoring_detached_and_short_lived_processes() {
    let loaded = corpus();
    let has_flag = |flag: &str| {
        loaded.iter().any(|item| {
            item.fixture
                .processes
                .iter()
                .any(|process| process.flags.iter().any(|f| f == flag))
        })
    };
    assert!(has_flag("--ignore-term"));
    assert!(has_flag("--setsid"));
    assert!(has_flag("--exit-after-ms"));
    let ignoring_and_detached = loaded.iter().any(|item| {
        item.fixture.processes.iter().any(|process| {
            process.flags.iter().any(|f| f == "--ignore-term")
                && process.flags.iter().any(|f| f == "--setsid")
        })
    });
    assert!(ignoring_and_detached);
}

#[test]
fn a_process_that_exits_by_itself_keeps_a_margin_of_five_seconds() {
    let mut checked = 0;
    for item in corpus() {
        for process in &item.fixture.processes {
            let spec = ProcSpec::parse(process.flags.iter().map(String::as_str)).unwrap();
            if let Some(millis) = spec.exit_after_ms {
                assert!(
                    millis >= MINIMUM_EXIT_AFTER_MS,
                    "{}/{}: --exit-after-ms {millis}",
                    item.fixture.name,
                    process.role
                );
                checked += 1;
            }
        }
    }
    assert!(checked >= 2, "{checked} processes exit by themselves");
}

#[test]
fn no_process_of_the_corpus_can_pass_a_wait_by_running_out_of_time() {
    let mut members = 0;
    for item in corpus() {
        let plan = item.fixture.plan().unwrap();
        for group in &plan.groups {
            for member in std::iter::once(&group.parent).chain(&group.children) {
                assert!(
                    member.spec.exit_after_ms.is_some() || member.spec.lifetime() > LONGEST_WAIT,
                    "{}/{}",
                    item.fixture.name,
                    member.role
                );
                members += 1;
            }
        }
    }
    assert!(members >= 20, "{members} members");
}

#[test]
fn the_corpus_has_live_fixtures_and_record_only_ones() {
    let loaded = corpus();
    let record_only = loaded
        .iter()
        .filter(|item| item.fixture.processes.iter().any(|process| process.record_only))
        .count();
    let live = loaded
        .iter()
        .filter(|item| item.fixture.processes.iter().any(|process| !process.record_only))
        .count();
    assert!(record_only >= 5, "{record_only} record-only fixtures");
    assert!(live >= 10, "{live} live fixtures");
}

#[test]
fn every_fixture_explains_itself() {
    for item in corpus() {
        let text = item.fixture.description.trim();
        assert!(text.len() >= 40, "{}: description {text:?}", item.fixture.name);
        assert!(text.ends_with('.'), "{}: {text:?}", item.fixture.name);
    }
}

#[test]
fn no_fixture_file_names_a_home_directory() {
    let home = std::env::var("HOME").ok().filter(|home| home.len() > 1);
    let needles: Vec<&str> = ["/Users/", "/home/", "/private/"]
        .into_iter()
        .chain(home.as_deref())
        .collect();
    let mut checked = 0;
    for entry in fs::read_dir(corpus_dir()).expect("the corpus directory exists") {
        let path = entry.unwrap().path();
        let text = fs::read_to_string(&path).unwrap();
        for needle in &needles {
            assert!(!text.contains(needle), "{} contains {needle}", path.display());
        }
        checked += 1;
    }
    assert!(checked >= MINIMUM_FIXTURES);
}
