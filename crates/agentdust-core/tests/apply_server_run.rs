mod apply_support;
mod scratch;

use std::sync::Mutex;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use agentdust_core::apply::exec::{Outcome, Reason};
use agentdust_core::apply::lock::IdentityLock;
use agentdust_core::apply::server::{ApplyError, Call, Report, Response, Step};
use agentdust_core::apply::timer::Timer;
use agentdust_core::class::Class;
use apply_support::{ServerRig, accept, call, finding};

fn owned(pids: &[i32]) -> Vec<agentdust_core::classifier::Finding> {
    pids.iter().map(|pid| finding(*pid, Class::OwnedEnded)).collect()
}

fn results(report: &Report) -> Vec<(Outcome, Option<Reason>)> {
    report
        .items
        .iter()
        .map(|item| (item.result, item.reason))
        .collect()
}

fn pending(rig: &ServerRig, pids: &[i32]) -> (Call, agentdust_core::apply::server::Challenge) {
    let created = rig.server.plan().unwrap();
    let chosen: Vec<_> = pids
        .iter()
        .map(|pid| created.items.iter().find(|item| item.pid == *pid).unwrap())
        .collect();
    let the_call = call(&created.plan_id, &chosen);
    let Step::Ask(challenge) = rig.server.begin(&the_call).unwrap() else {
        panic!("a prompt is expected");
    };
    (the_call, challenge)
}

fn finish(rig: &ServerRig, the_call: &Call, challenge: &agentdust_core::apply::server::Challenge) -> Report {
    match rig
        .server
        .answer(the_call, &challenge.nonce, accept(challenge))
        .unwrap()
    {
        Step::Done(report) => report,
        Step::Ask(_) => panic!("the call has one unit"),
    }
}

#[test]
fn a_change_of_class_during_approval_aborts_the_item_and_nothing_is_signalled() {
    let changes = [
        (Class::OwnedEnded, Class::Managed),
        (Class::OwnedEnded, Class::Unknown),
        (Class::OwnedEnded, Class::OwnedLive),
        (Class::OwnedEnded, Class::Suspect),
        (Class::OwnedEnded, Class::LikelyOwned),
        (Class::Suspect, Class::Managed),
        (Class::Suspect, Class::Unknown),
        (Class::Suspect, Class::OwnedEnded),
    ];
    for (planned, fresh) in changes {
        let rig = ServerRig::new(vec![finding(10, planned)]);
        let (the_call, challenge) = pending(&rig, &[10]);
        rig.world.set_class(10, fresh);
        let report = finish(&rig, &the_call, &challenge);
        assert_eq!(
            results(&report),
            [(Outcome::RevalidationFailed, Some(Reason::ClassChanged))],
            "{planned} to {fresh}"
        );
        assert!(rig.world.signals().is_empty());
        assert!(rig.world.alive(10));
    }
}

#[test]
fn one_item_that_fails_revalidation_does_not_stop_the_others_in_the_batch() {
    let rig = ServerRig::new(owned(&[10, 11, 12]));
    let (the_call, challenge) = pending(&rig, &[10, 11, 12]);
    rig.world.set_class(11, Class::Managed);
    let report = finish(&rig, &the_call, &challenge);
    assert_eq!(
        results(&report),
        [
            (Outcome::Terminated, None),
            (Outcome::RevalidationFailed, Some(Reason::ClassChanged)),
            (Outcome::Terminated, None)
        ]
    );
    assert_eq!(rig.world.signals(), [10, 12]);
}

#[test]
fn a_process_that_exits_during_approval_is_reported_gone() {
    let rig = ServerRig::new(owned(&[10, 11]));
    let (the_call, challenge) = pending(&rig, &[10, 11]);
    rig.world.exit(10);
    let report = finish(&rig, &the_call, &challenge);
    assert_eq!(
        results(&report),
        [(Outcome::Gone, None), (Outcome::Terminated, None)]
    );
    assert_eq!(rig.world.signals(), [11]);
}

#[test]
fn a_pid_that_goes_to_another_process_during_approval_is_not_signalled() {
    let rig = ServerRig::new(owned(&[10]));
    let (the_call, challenge) = pending(&rig, &[10]);
    rig.world.replace(10);
    let report = finish(&rig, &the_call, &challenge);
    assert_eq!(
        results(&report),
        [(Outcome::RevalidationFailed, Some(Reason::IdentityChanged))]
    );
    assert!(rig.world.signals().is_empty());
}

#[test]
fn a_new_executable_during_approval_is_not_signalled() {
    let rig = ServerRig::new(owned(&[10]));
    let (the_call, challenge) = pending(&rig, &[10]);
    rig.world.set_path(10, "/usr/bin/other");
    let report = finish(&rig, &the_call, &challenge);
    assert_eq!(
        results(&report),
        [(Outcome::RevalidationFailed, Some(Reason::PathChanged))]
    );
    assert!(rig.world.signals().is_empty());
}

#[test]
fn a_process_that_ignores_sigterm_is_a_survivor_and_gets_one_signal() {
    let rig = ServerRig::new(vec![finding(20, Class::Suspect)]);
    rig.world.stubborn(20);
    let (the_call, challenge) = pending(&rig, &[20]);
    let report = finish(&rig, &the_call, &challenge);
    assert_eq!(results(&report), [(Outcome::Survivor, None)]);
    assert_eq!(rig.world.signals(), [20]);
    assert!(rig.world.alive(20));
    assert!(rig.timer.now() >= Duration::from_secs(5));
}

#[test]
fn only_the_approved_pids_are_signalled_whatever_else_runs() {
    let mut findings = owned(&[10, 11]);
    findings.push(finding(30, Class::Managed));
    findings.push(finding(31, Class::Suspect));
    let rig = ServerRig::new(findings);
    let (the_call, challenge) = pending(&rig, &[11]);
    finish(&rig, &the_call, &challenge);
    assert_eq!(rig.world.signals(), [11]);
    assert!(rig.world.alive(10) && rig.world.alive(30) && rig.world.alive(31));
}

#[test]
fn an_item_another_server_holds_is_handled_elsewhere_and_can_be_asked_about_again() {
    let rig = ServerRig::new(owned(&[10]));
    let (the_call, challenge) = pending(&rig, &[10]);
    let created_id = the_call.item_ids[0].clone();
    let held = IdentityLock::try_acquire(&rig.locks(), &created_id)
        .unwrap()
        .unwrap();
    let report = finish(&rig, &the_call, &challenge);
    assert_eq!(results(&report), [(Outcome::HandledElsewhere, None)]);
    assert!(rig.world.signals().is_empty());
    drop(held);
    let Step::Ask(again) = rig.server.begin(&the_call).unwrap() else {
        panic!("the item is free to be asked about again");
    };
    let report = finish(&rig, &the_call, &again);
    assert_eq!(results(&report), [(Outcome::Terminated, None)]);
}

#[test]
fn two_servers_that_apply_the_same_item_signal_it_once() {
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let release_rx = Mutex::new(release_rx);
    let rig = ServerRig::hooked(
        owned(&[10]),
        Some(Box::new(move |_| {
            entered_tx.send(()).unwrap();
            release_rx.lock().unwrap().recv().unwrap();
        })),
    );
    let other = rig.another_server(None);
    let first_plan = rig.server.plan().unwrap();
    let second_plan = other.plan().unwrap();
    let first_call = call(&first_plan.plan_id, &[&first_plan.items[0]]);
    let second_call = call(&second_plan.plan_id, &[&second_plan.items[0]]);

    let first = {
        let server = rig.server.clone();
        thread::spawn(move || {
            server
                .run(&first_call, &mut |challenge| accept(challenge))
                .unwrap()
        })
    };
    entered_rx.recv().unwrap();
    let second = other
        .run(&second_call, &mut |challenge| accept(challenge))
        .unwrap();
    assert_eq!(results(&second), [(Outcome::HandledElsewhere, None)]);
    release_tx.send(()).unwrap();
    let first = first.join().unwrap();
    assert_eq!(results(&first), [(Outcome::Terminated, None)]);
    assert_eq!(rig.world.signals(), [10]);
}

#[test]
fn many_servers_applying_one_item_at_once_signal_it_at_most_once() {
    let rig = ServerRig::new(owned(&[10]));
    let servers: Vec<_> = (0..4).map(|_| rig.another_server(None)).collect();
    let calls: Vec<Call> = servers
        .iter()
        .map(|server| {
            let created = server.plan().unwrap();
            call(&created.plan_id, &[&created.items[0]])
        })
        .collect();
    let outcomes: Vec<Outcome> = thread::scope(|scope| {
        let handles: Vec<_> = servers
            .iter()
            .zip(&calls)
            .map(|(server, the_call)| {
                scope.spawn(move || {
                    server
                        .run(the_call, &mut |challenge| accept(challenge))
                        .unwrap()
                        .items[0]
                        .result
                })
            })
            .collect();
        handles.into_iter().map(|handle| handle.join().unwrap()).collect()
    });
    assert_eq!(rig.world.signals(), [10]);
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == Outcome::Terminated)
            .count(),
        1
    );
    for outcome in &outcomes {
        assert!(
            matches!(
                outcome,
                Outcome::Terminated | Outcome::HandledElsewhere | Outcome::Gone
            ),
            "{outcome:?}"
        );
    }
}

#[test]
fn every_asked_item_has_one_result_line_and_signalled_items_have_an_attempt_line_too() {
    let rig = ServerRig::new(vec![
        finding(10, Class::OwnedEnded),
        finding(20, Class::Suspect),
        finding(21, Class::Suspect),
        finding(22, Class::Suspect),
    ]);
    rig.world.stubborn(22);
    let created = rig.server.plan().unwrap();
    let refs: Vec<_> = created.items.iter().collect();
    let the_call = call(&created.plan_id, &refs);
    let mut turn = 0;
    let report = rig
        .server
        .run(&the_call, &mut |challenge| {
            turn += 1;
            match turn {
                1 | 4 => accept(challenge),
                2 => Response::Decline,
                _ => Response::Accept(Some("WRONG".to_owned())),
            }
        })
        .unwrap();
    assert_eq!(
        report.items.iter().map(|item| item.result).collect::<Vec<_>>(),
        [
            Outcome::Terminated,
            Outcome::Declined,
            Outcome::WrongCode,
            Outcome::Survivor
        ]
    );
    let audit = rig.audit();
    let results: Vec<(String, String)> = audit
        .iter()
        .filter(|line| line["phase"] == "result")
        .map(|line| {
            (
                line["item"].as_str().unwrap().to_owned(),
                line["result"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let expected: Vec<(String, String)> = report
        .items
        .iter()
        .map(|item| (item.item_id.clone(), item.result.code().to_owned()))
        .collect();
    assert_eq!(results, expected);
    let attempts: Vec<i64> = audit
        .iter()
        .filter(|line| line["phase"] == "attempt")
        .map(|line| line["pid"].as_i64().unwrap())
        .collect();
    assert_eq!(attempts, [10, 22]);
    assert!(audit.iter().all(|line| line["plan"] == created.plan_id.as_str()));
}

#[test]
fn run_stops_on_a_refused_call_before_asking_anything() {
    let rig = ServerRig::new(owned(&[10]));
    let created = rig.server.plan().unwrap();
    let mut asked = 0;
    let refused = rig.server.run(&call(&created.plan_id, &[]), &mut |_| {
        asked += 1;
        Response::Decline
    });
    assert_eq!(refused, Err(ApplyError::NoItems));
    assert_eq!(asked, 0);
}

#[test]
fn run_asks_once_per_unit_in_order_and_returns_the_report() {
    let rig = ServerRig::new(vec![
        finding(20, Class::Suspect),
        finding(10, Class::OwnedEnded),
        finding(21, Class::Suspect),
    ]);
    let created = rig.server.plan().unwrap();
    let refs: Vec<_> = created.items.iter().collect();
    let the_call = call(&created.plan_id, &refs);
    let mut seen = Vec::new();
    let report = rig
        .server
        .run(&the_call, &mut |challenge| {
            seen.push((challenge.index, challenge.total, challenge.item_ids.len()));
            accept(challenge)
        })
        .unwrap();
    assert_eq!(seen, [(0, 3, 1), (1, 3, 1), (2, 3, 1)]);
    assert_eq!(report.plan_id, created.plan_id);
    assert_eq!(report.items.len(), 3);
}
