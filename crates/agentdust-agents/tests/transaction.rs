use std::cell::RefCell;
use std::rc::Rc;

use agentdust_agents::transaction::{Step, run};

type Log = Rc<RefCell<Vec<String>>>;

struct Fake {
    name: &'static str,
    log: Log,
    apply_fails: bool,
    undo_fails: bool,
}

impl Fake {
    fn ok(name: &'static str, log: &Log) -> Box<dyn Step> {
        Box::new(Self {
            name,
            log: log.clone(),
            apply_fails: false,
            undo_fails: false,
        })
    }

    fn failing_apply(name: &'static str, log: &Log) -> Box<dyn Step> {
        Box::new(Self {
            name,
            log: log.clone(),
            apply_fails: true,
            undo_fails: false,
        })
    }

    fn failing_undo(name: &'static str, log: &Log) -> Box<dyn Step> {
        Box::new(Self {
            name,
            log: log.clone(),
            apply_fails: false,
            undo_fails: true,
        })
    }
}

impl Step for Fake {
    fn name(&self) -> String {
        self.name.to_owned()
    }

    fn apply(&mut self) -> Result<(), String> {
        self.log.borrow_mut().push(format!("apply {}", self.name));
        if self.apply_fails {
            Err(format!("{} broke", self.name))
        } else {
            Ok(())
        }
    }

    fn undo(&mut self) -> Result<(), String> {
        self.log.borrow_mut().push(format!("undo {}", self.name));
        if self.undo_fails {
            Err(format!("{} could not be undone", self.name))
        } else {
            Ok(())
        }
    }
}

fn log() -> Log {
    Rc::new(RefCell::new(Vec::new()))
}

#[test]
fn every_step_runs_in_order_when_all_succeed() {
    let log = log();
    let mut steps = vec![
        Fake::ok("one", &log),
        Fake::ok("two", &log),
        Fake::ok("three", &log),
    ];
    let report = run(&mut steps);
    assert_eq!(*log.borrow(), ["apply one", "apply two", "apply three"]);
    assert_eq!(report.completed, ["one", "two", "three"]);
    assert!(report.failure.is_none());
    assert!(report.rollback.is_empty());
    assert!(report.succeeded());
}

#[test]
fn a_failure_undoes_the_completed_steps_in_reverse_and_stops() {
    let log = log();
    let mut steps = vec![
        Fake::ok("one", &log),
        Fake::ok("two", &log),
        Fake::failing_apply("three", &log),
        Fake::ok("four", &log),
    ];
    let report = run(&mut steps);
    assert_eq!(
        *log.borrow(),
        ["apply one", "apply two", "apply three", "undo two", "undo one"]
    );
    assert_eq!(report.completed, ["one", "two"]);
    let failure = report.failure.as_ref().unwrap();
    assert_eq!(
        (failure.step.as_str(), failure.reason.as_str()),
        ("three", "three broke")
    );
    let undone: Vec<(&str, bool)> = report
        .rollback
        .iter()
        .map(|undo| (undo.step.as_str(), undo.result.is_ok()))
        .collect();
    assert_eq!(undone, [("two", true), ("one", true)]);
    assert!(!report.succeeded());
    assert!(report.rolled_back_cleanly());
}

#[test]
fn the_failing_step_itself_is_not_undone() {
    let log = log();
    let mut steps = vec![Fake::failing_apply("only", &log)];
    let report = run(&mut steps);
    assert_eq!(*log.borrow(), ["apply only"]);
    assert!(report.rollback.is_empty());
    assert!(report.rolled_back_cleanly());
}

#[test]
fn a_step_that_cannot_be_undone_does_not_stop_the_other_undos() {
    let log = log();
    let mut steps = vec![
        Fake::ok("one", &log),
        Fake::failing_undo("two", &log),
        Fake::ok("three", &log),
        Fake::failing_apply("four", &log),
    ];
    let report = run(&mut steps);
    assert_eq!(
        *log.borrow(),
        [
            "apply one",
            "apply two",
            "apply three",
            "apply four",
            "undo three",
            "undo two",
            "undo one"
        ]
    );
    assert!(!report.rolled_back_cleanly());
    let two = report.rollback.iter().find(|undo| undo.step == "two").unwrap();
    assert_eq!(two.result.as_ref().unwrap_err(), "two could not be undone");
    assert!(report.rollback.iter().filter(|undo| undo.result.is_ok()).count() == 2);
}

#[test]
fn no_steps_is_a_success() {
    let mut steps: Vec<Box<dyn Step>> = Vec::new();
    assert!(run(&mut steps).succeeded());
}
