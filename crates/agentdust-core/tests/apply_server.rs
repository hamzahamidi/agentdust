mod apply_support;
mod scratch;

use std::fs;
use std::sync::Arc;
use std::time::Duration;

use agentdust_core::apply::exec::Outcome;
use agentdust_core::apply::server::{ApplyError, Call, Challenge, Report, Response, Server, Step};
use agentdust_core::class::Class;
use agentdust_core::config::Disabled;
use agentdust_core::finding::HumanDisplay;
use agentdust_core::plan::Created;
use apply_support::{ServerRig, accept, call, code_of, describe, finding, finding_at, write_config};

type Responder = Box<dyn Fn(&Challenge) -> Response>;
type Retry = (Arc<Server>, Call, String, Response);

fn owned(pids: &[i32]) -> Vec<agentdust_core::classifier::Finding> {
    pids.iter().map(|pid| finding(*pid, Class::OwnedEnded)).collect()
}

fn plan(rig: &ServerRig) -> Created {
    rig.server.plan().unwrap()
}

fn ask(rig: &ServerRig, call: &Call) -> Challenge {
    match rig.server.begin(call).unwrap() {
        Step::Ask(challenge) => challenge,
        Step::Done(report) => panic!("expected a prompt, got {report:?}"),
    }
}

fn done(step: Step) -> Report {
    match step {
        Step::Done(report) => report,
        Step::Ask(challenge) => panic!("expected the end, got a prompt: {}", challenge.message),
    }
}

fn results(report: &Report) -> Vec<Outcome> {
    report.items.iter().map(|item| item.result).collect()
}

#[test]
fn only_owned_ended_and_suspect_items_are_planned_and_other_ids_are_refused() {
    let classes = [
        (10, Class::OwnedEnded),
        (20, Class::Suspect),
        (30, Class::Managed),
        (40, Class::OwnedLive),
        (50, Class::LikelyOwned),
        (60, Class::Unknown),
    ];
    let rig = ServerRig::new(classes.iter().map(|(pid, class)| finding(*pid, *class)).collect());
    let created = plan(&rig);
    assert_eq!(created.items.len(), 2);
    for (pid, class) in &classes[2..] {
        let model = describe(&finding(*pid, *class));
        assert_eq!(
            rig.server.begin(&call(&created.plan_id, &[&model])),
            Err(ApplyError::UnknownItem { index: 0 })
        );
        assert_eq!(
            rig.server
                .begin(&call(&created.plan_id, &[&created.items[0], &model])),
            Err(ApplyError::UnknownItem { index: 1 })
        );
    }
    assert!(rig.world.signals().is_empty());
    assert!(matches!(
        rig.server.begin(&call(&created.plan_id, &[&created.items[0]])),
        Ok(Step::Ask(_))
    ));
}

#[test]
fn an_unknown_plan_and_a_malformed_plan_id_are_refused() {
    let rig = ServerRig::new(owned(&[10]));
    let created = plan(&rig);
    for id in ["", "0", &"f".repeat(32), "../etc", &"x".repeat(5000)] {
        assert_eq!(
            rig.server.begin(&call(id, &[&created.items[0]])),
            Err(ApplyError::UnknownPlan),
            "{id:.16}"
        );
    }
}

#[test]
fn a_call_needs_at_least_one_item_and_at_most_ten() {
    let rig = ServerRig::new(owned(&(10..22).collect::<Vec<_>>()));
    let created = plan(&rig);
    let refs: Vec<_> = created.items.iter().collect();
    assert_eq!(
        rig.server.begin(&call(&created.plan_id, &[])),
        Err(ApplyError::NoItems)
    );
    assert_eq!(
        rig.server.begin(&call(&created.plan_id, &refs[..11])),
        Err(ApplyError::TooManyItems { given: 11, max: 10 })
    );
    assert!(matches!(
        rig.server.begin(&call(&created.plan_id, &refs[..10])),
        Ok(Step::Ask(_))
    ));
}

#[test]
fn the_limit_of_ten_counts_owned_ended_and_suspect_items_together() {
    let mut findings = owned(&[10, 11, 12, 13, 14, 15]);
    findings.extend((20..25).map(|pid| finding(pid, Class::Suspect)));
    let rig = ServerRig::new(findings);
    let created = plan(&rig);
    let refs: Vec<_> = created.items.iter().collect();
    assert_eq!(refs.len(), 11);
    assert_eq!(
        rig.server.begin(&call(&created.plan_id, &refs)),
        Err(ApplyError::TooManyItems { given: 11, max: 10 })
    );
}

#[test]
fn an_item_named_twice_is_refused_and_nothing_is_claimed() {
    let rig = ServerRig::new(owned(&[10, 11]));
    let created = plan(&rig);
    let (a, b) = (&created.items[0], &created.items[1]);
    assert_eq!(
        rig.server.begin(&call(&created.plan_id, &[a, b, a])),
        Err(ApplyError::DuplicateItem { index: 2 })
    );
    assert!(matches!(
        rig.server.begin(&call(&created.plan_id, &[a, b])),
        Ok(Step::Ask(_))
    ));
}

#[test]
fn ten_owned_ended_items_are_one_prompt_with_one_code() {
    let pids: Vec<i32> = (10..20).collect();
    let rig = ServerRig::new(owned(&pids));
    let created = plan(&rig);
    let refs: Vec<_> = created.items.iter().collect();
    let challenge = ask(&rig, &call(&created.plan_id, &refs));
    assert_eq!((challenge.index, challenge.total), (0, 1));
    assert_eq!(challenge.item_ids.len(), 10);
    for model in &created.items {
        assert!(
            challenge.message.contains(&HumanDisplay::prompt(model)),
            "{}",
            model.item_id
        );
    }
    let report = done(
        rig.server
            .answer(
                &call(&created.plan_id, &refs),
                &challenge.nonce,
                accept(&challenge),
            )
            .unwrap(),
    );
    assert_eq!(results(&report), vec![Outcome::Terminated; 10]);
    assert_eq!(rig.world.signals(), pids);
}

#[test]
fn the_owned_batch_prompt_is_this_text() {
    let rig = ServerRig::new(owned(&[10, 11]));
    let created = plan(&rig);
    let refs: Vec<_> = created.items.iter().collect();
    let challenge = ask(&rig, &call(&created.plan_id, &refs));
    let message = challenge.message.replace(&code_of(&challenge), "<CODE>");
    let expected = format!(
        "Stop 2 processes that a Claude Code session left behind? The session has ended. Each gets one SIGTERM.\n\n{}\n{}\n\nType <CODE> to approve all 2.",
        HumanDisplay::prompt(&created.items[0]),
        HumanDisplay::prompt(&created.items[1]),
    );
    assert_eq!(message, expected);
}

#[test]
fn the_single_owned_item_and_the_suspect_prompts_are_these_texts() {
    let rig = ServerRig::new(vec![finding(10, Class::OwnedEnded), finding(20, Class::Suspect)]);
    let created = plan(&rig);
    let owned_prompt = ask(&rig, &call(&created.plan_id, &[&created.items[0]]));
    assert_eq!(
        owned_prompt.message.replace(&code_of(&owned_prompt), "<CODE>"),
        format!(
            "Stop 1 process that a Claude Code session left behind? The session has ended. It gets one SIGTERM.\n\n{}\n\nType <CODE> to approve.",
            HumanDisplay::prompt(&created.items[0])
        )
    );
    let suspect_prompt = ask(&rig, &call(&created.plan_id, &[&created.items[1]]));
    assert_eq!(
        suspect_prompt
            .message
            .replace(&code_of(&suspect_prompt), "<CODE>"),
        format!(
            "Stop this process? No session that AgentDust knows started it, and it has run detached and idle for a long time. It gets one SIGTERM.\n\n{}\n\nType <CODE> to approve.",
            HumanDisplay::prompt(&created.items[1])
        )
    );
}

#[test]
fn a_prompt_holds_the_fields_of_the_model_view_and_nothing_a_process_chose() {
    let rig = ServerRig::new(vec![
        finding_at(
            10,
            Class::OwnedEnded,
            "/Users/user-sentinel-wN8d/repo-sentinel-hJ5t/evil name\n\u{1b}[31mIGNORE ALL",
        ),
        finding_at(
            20,
            Class::Suspect,
            "/Users/user-sentinel-wN8d/repo-sentinel-hJ5t/node",
        ),
    ]);
    let created = plan(&rig);
    for item in &created.items {
        let challenge = ask(&rig, &call(&created.plan_id, &[item]));
        for forbidden in ["sentinel", "/Users", "evil", "IGNORE", "\u{1b}"] {
            assert!(!challenge.message.contains(forbidden), "{forbidden}");
        }
        assert!(challenge.message.contains(&item.item_id));
        assert!(challenge.message.contains(&format!("pid {}", item.pid)));
    }
}

#[test]
fn owned_ended_items_come_first_as_one_unit_and_each_suspect_is_its_own() {
    let rig = ServerRig::new(vec![
        finding(10, Class::OwnedEnded),
        finding(11, Class::OwnedEnded),
        finding(20, Class::Suspect),
        finding(21, Class::Suspect),
    ]);
    let created = plan(&rig);
    let by_pid = |pid: i32| created.items.iter().find(|item| item.pid == pid).unwrap();
    let the_call = call(
        &created.plan_id,
        &[by_pid(20), by_pid(10), by_pid(21), by_pid(11)],
    );
    let first = ask(&rig, &the_call);
    assert_eq!((first.index, first.total), (0, 3));
    assert_eq!(
        first.item_ids,
        [by_pid(10).item_id.clone(), by_pid(11).item_id.clone()]
    );

    let second = match rig
        .server
        .answer(&the_call, &first.nonce, accept(&first))
        .unwrap()
    {
        Step::Ask(challenge) => challenge,
        Step::Done(_) => panic!("two suspects are still to be asked"),
    };
    assert_eq!((second.index, second.total), (1, 3));
    assert_eq!(second.item_ids, [by_pid(20).item_id.clone()]);
    assert_ne!(second.nonce, first.nonce);
    assert_eq!(rig.world.signals(), [10, 11]);

    let third = match rig
        .server
        .answer(&the_call, &second.nonce, accept(&second))
        .unwrap()
    {
        Step::Ask(challenge) => challenge,
        Step::Done(_) => panic!("one suspect is still to be asked"),
    };
    assert_eq!((third.index, third.total), (2, 3));
    assert_eq!(third.item_ids, [by_pid(21).item_id.clone()]);

    let report = done(
        rig.server
            .answer(&the_call, &third.nonce, accept(&third))
            .unwrap(),
    );
    assert_eq!(
        report
            .items
            .iter()
            .map(|item| item.item_id.clone())
            .collect::<Vec<_>>(),
        the_call.item_ids
    );
    assert_eq!(results(&report), vec![Outcome::Terminated; 4]);
    assert_eq!(rig.world.signals(), [10, 11, 20, 21]);
}

#[test]
fn a_code_for_one_unit_does_not_approve_another() {
    let rig = ServerRig::new(vec![finding(20, Class::Suspect), finding(21, Class::Suspect)]);
    let created = plan(&rig);
    let refs: Vec<_> = created.items.iter().collect();
    let the_call = call(&created.plan_id, &refs);
    let first = ask(&rig, &the_call);
    let Step::Ask(second) = rig
        .server
        .answer(&the_call, &first.nonce, accept(&first))
        .unwrap()
    else {
        panic!("a second suspect is still to be asked");
    };
    let borrowed = if code_of(&first) == code_of(&second) {
        "QQQQ".to_owned()
    } else {
        code_of(&first)
    };
    let report = done(
        rig.server
            .answer(&the_call, &second.nonce, Response::Accept(Some(borrowed)))
            .unwrap(),
    );
    assert_eq!(results(&report), [Outcome::Terminated, Outcome::WrongCode]);
    assert_eq!(rig.world.signals(), [20]);
}

#[test]
fn a_refused_unit_does_not_stop_the_units_after_it() {
    let rig = ServerRig::new(vec![
        finding(10, Class::OwnedEnded),
        finding(20, Class::Suspect),
        finding(21, Class::Suspect),
    ]);
    let created = plan(&rig);
    let refs: Vec<_> = created.items.iter().collect();
    let the_call = call(&created.plan_id, &refs);
    let first = ask(&rig, &the_call);
    let Step::Ask(second) = rig
        .server
        .answer(&the_call, &first.nonce, Response::Decline)
        .unwrap()
    else {
        panic!("the suspects are still to be asked");
    };
    let Step::Ask(third) = rig
        .server
        .answer(&the_call, &second.nonce, accept(&second))
        .unwrap()
    else {
        panic!("the last suspect is still to be asked");
    };
    let report = done(
        rig.server
            .answer(&the_call, &third.nonce, Response::Cancel)
            .unwrap(),
    );
    assert_eq!(
        results(&report),
        [Outcome::Declined, Outcome::Terminated, Outcome::Cancelled]
    );
    assert_eq!(rig.world.signals(), [20]);
}

fn answered(response: impl FnOnce(&Challenge) -> Response) -> (Report, ServerRig) {
    let rig = ServerRig::new(owned(&[10, 11]));
    let created = plan(&rig);
    let refs: Vec<_> = created.items.iter().collect();
    let the_call = call(&created.plan_id, &refs);
    let challenge = ask(&rig, &the_call);
    let response = response(&challenge);
    let report = done(rig.server.answer(&the_call, &challenge.nonce, response).unwrap());
    (report, rig)
}

#[test]
fn only_the_exact_code_approves() {
    let (report, rig) = answered(accept);
    assert_eq!(results(&report), [Outcome::Terminated; 2]);
    assert_eq!(rig.world.signals(), [10, 11]);
}

#[test]
fn any_other_answer_ends_the_approval_and_signals_nothing() {
    let cases: Vec<(&str, Responder, Outcome)> = vec![
        (
            "lower case",
            Box::new(|c| Response::Accept(Some(code_of(c).to_lowercase()))),
            Outcome::WrongCode,
        ),
        (
            "padded",
            Box::new(|c| Response::Accept(Some(format!(" {} ", code_of(c))))),
            Outcome::WrongCode,
        ),
        (
            "longer",
            Box::new(|c| Response::Accept(Some(format!("{}A", code_of(c))))),
            Outcome::WrongCode,
        ),
        (
            "blank",
            Box::new(|_| Response::Accept(Some("    ".to_owned()))),
            Outcome::WrongCode,
        ),
        (
            "empty text",
            Box::new(|_| Response::Accept(Some(String::new()))),
            Outcome::Empty,
        ),
        ("no content", Box::new(|_| Response::Accept(None)), Outcome::Empty),
        ("decline", Box::new(|_| Response::Decline), Outcome::Declined),
        ("cancel", Box::new(|_| Response::Cancel), Outcome::Cancelled),
        ("timeout", Box::new(|_| Response::Timeout), Outcome::TimedOut),
    ];
    for (name, response, expected) in cases {
        let (report, rig) = answered(response);
        assert_eq!(results(&report), [expected; 2], "{name}");
        assert!(rig.world.signals().is_empty(), "{name}");
        assert!(rig.world.alive(10) && rig.world.alive(11), "{name}");
    }
}

#[test]
fn a_refused_item_can_be_asked_about_again() {
    let (report, rig) = answered(|_| Response::Decline);
    assert_eq!(results(&report), [Outcome::Declined; 2]);
    let created = rig.server.plan().unwrap();
    let refs: Vec<_> = created.items.iter().collect();
    let again = ask(&rig, &call(&created.plan_id, &refs));
    assert!(again.message.contains("Type "));
}

#[test]
fn a_code_is_valid_for_just_under_two_minutes() {
    let rig = ServerRig::new(owned(&[10]));
    let created = plan(&rig);
    let the_call = call(&created.plan_id, &[&created.items[0]]);
    let challenge = ask(&rig, &the_call);
    rig.timer
        .advance(Duration::from_secs(120) - Duration::from_nanos(1));
    let report = done(
        rig.server
            .answer(&the_call, &challenge.nonce, accept(&challenge))
            .unwrap(),
    );
    assert_eq!(results(&report), [Outcome::Terminated]);
}

#[test]
fn a_code_that_is_two_minutes_old_is_expired() {
    let rig = ServerRig::new(owned(&[10]));
    let created = plan(&rig);
    let the_call = call(&created.plan_id, &[&created.items[0]]);
    let challenge = ask(&rig, &the_call);
    rig.timer.advance(Duration::from_secs(120));
    let report = done(
        rig.server
            .answer(&the_call, &challenge.nonce, accept(&challenge))
            .unwrap(),
    );
    assert_eq!(results(&report), [Outcome::Expired]);
    assert!(rig.world.signals().is_empty());
}

#[test]
fn a_decline_after_expiry_is_still_a_decline() {
    let rig = ServerRig::new(owned(&[10]));
    let created = plan(&rig);
    let the_call = call(&created.plan_id, &[&created.items[0]]);
    let challenge = ask(&rig, &the_call);
    rig.timer.advance(Duration::from_secs(300));
    let report = done(
        rig.server
            .answer(&the_call, &challenge.nonce, Response::Decline)
            .unwrap(),
    );
    assert_eq!(results(&report), [Outcome::Declined]);
}

#[test]
fn a_nonce_that_was_never_issued_or_is_malformed_is_refused() {
    let rig = ServerRig::new(owned(&[10]));
    let created = plan(&rig);
    let the_call = call(&created.plan_id, &[&created.items[0]]);
    let challenge = ask(&rig, &the_call);
    let response = accept(&challenge);
    for nonce in [
        "",
        "0",
        &"0".repeat(32),
        &"f".repeat(32),
        "not hex at all",
        "\0",
        &challenge.nonce.to_uppercase(),
        &format!("{} ", challenge.nonce),
        &challenge.nonce[..31],
        &"a".repeat(100_000),
    ] {
        assert_eq!(
            rig.server.answer(&the_call, nonce, response.clone()),
            Err(ApplyError::UnknownRequestState),
            "{nonce:.20}"
        );
    }
    assert!(rig.world.signals().is_empty());
    let report = done(rig.server.answer(&the_call, &challenge.nonce, response).unwrap());
    assert_eq!(results(&report), [Outcome::Terminated]);
}

#[test]
fn a_nonce_cannot_be_used_twice() {
    let rig = ServerRig::new(owned(&[10]));
    let created = plan(&rig);
    let the_call = call(&created.plan_id, &[&created.items[0]]);
    let challenge = ask(&rig, &the_call);
    let response = accept(&challenge);
    done(
        rig.server
            .answer(&the_call, &challenge.nonce, response.clone())
            .unwrap(),
    );
    assert_eq!(
        rig.server.answer(&the_call, &challenge.nonce, response),
        Err(ApplyError::UnknownRequestState)
    );
    assert_eq!(rig.world.signals(), [10]);
}

#[test]
fn every_change_to_the_arguments_burns_the_nonce_and_signals_nothing() {
    let rig_for = || {
        let rig = ServerRig::new(owned(&[10, 11, 12]));
        let created = plan(&rig);
        (rig, created)
    };
    type Change = Box<dyn Fn(&Call, &Created) -> Call>;
    let changes: Vec<(&str, Change)> = vec![
        (
            "plan id",
            Box::new(|c, _| Call {
                plan_id: "0".repeat(32),
                ..c.clone()
            }),
        ),
        (
            "reordered",
            Box::new(|c, _| Call {
                item_ids: c.item_ids.iter().rev().cloned().collect(),
                ..c.clone()
            }),
        ),
        (
            "added",
            Box::new(|c, made| {
                let mut ids = c.item_ids.clone();
                ids.push(made.items[2].item_id.clone());
                Call {
                    item_ids: ids,
                    ..c.clone()
                }
            }),
        ),
        (
            "removed",
            Box::new(|c, _| Call {
                item_ids: c.item_ids[..1].to_vec(),
                ..c.clone()
            }),
        ),
        (
            "duplicated",
            Box::new(|c, _| Call {
                item_ids: vec![c.item_ids[0].clone(), c.item_ids[0].clone()],
                ..c.clone()
            }),
        ),
        (
            "replaced",
            Box::new(|c, made| Call {
                item_ids: vec![c.item_ids[0].clone(), made.items[2].item_id.clone()],
                ..c.clone()
            }),
        ),
    ];
    for (name, change) in changes {
        let (rig, created) = rig_for();
        let original = call(&created.plan_id, &[&created.items[0], &created.items[1]]);
        let challenge = ask(&rig, &original);
        let response = accept(&challenge);
        assert_eq!(
            rig.server
                .answer(&change(&original, &created), &challenge.nonce, response.clone()),
            Err(ApplyError::ArgumentsChanged),
            "{name}"
        );
        assert_eq!(
            rig.server.answer(&original, &challenge.nonce, response),
            Err(ApplyError::UnknownRequestState),
            "{name}"
        );
        assert!(rig.world.signals().is_empty(), "{name}");
        assert!(matches!(rig.server.begin(&original), Ok(Step::Ask(_))), "{name}");
    }
}

#[test]
fn the_nonce_is_gone_before_the_first_signal_goes_out() {
    use std::sync::{Mutex, OnceLock};
    let seen: Arc<Mutex<Vec<Result<Step, ApplyError>>>> = Arc::default();
    let slot: Arc<OnceLock<Retry>> = Arc::default();
    let (hook_slot, hook_seen) = (Arc::clone(&slot), Arc::clone(&seen));
    let rig = ServerRig::hooked(
        owned(&[10]),
        Some(Box::new(move |_| {
            if let Some((server, call, nonce, response)) = hook_slot.get() {
                hook_seen
                    .lock()
                    .unwrap()
                    .push(server.answer(call, nonce, response.clone()));
            }
        })),
    );
    let created = plan(&rig);
    let the_call = call(&created.plan_id, &[&created.items[0]]);
    let challenge = ask(&rig, &the_call);
    let response = accept(&challenge);
    slot.set((
        Arc::clone(&rig.server),
        the_call.clone(),
        challenge.nonce.clone(),
        response.clone(),
    ))
    .ok()
    .unwrap();
    done(rig.server.answer(&the_call, &challenge.nonce, response).unwrap());
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0], Err(ApplyError::UnknownRequestState));
    assert_eq!(rig.world.signals(), [10]);
}

#[test]
fn no_lock_is_held_while_a_prompt_waits_and_a_second_apply_goes_through() {
    let rig = ServerRig::new(owned(&[10, 11]));
    let created = plan(&rig);
    let (a, b) = (&created.items[0], &created.items[1]);
    let first_call = call(&created.plan_id, &[a]);
    let waiting = ask(&rig, &first_call);
    let lock_files = || fs::read_dir(rig.locks()).map(|read| read.count()).unwrap_or(0);
    assert_eq!(lock_files(), 0);

    let second_call = call(&created.plan_id, &[b]);
    let second = rig
        .server
        .run(&second_call, &mut |challenge| accept(challenge))
        .unwrap();
    assert_eq!(results(&second), [Outcome::Terminated]);
    assert_eq!(lock_files(), 0);
    assert_eq!(rig.world.signals(), [11]);

    let first = done(
        rig.server
            .answer(&first_call, &waiting.nonce, accept(&waiting))
            .unwrap(),
    );
    assert_eq!(results(&first), [Outcome::Terminated]);
}

#[test]
fn an_item_that_is_waiting_for_approval_cannot_be_asked_about_again() {
    let rig = ServerRig::new(owned(&[10]));
    let created = plan(&rig);
    let the_call = call(&created.plan_id, &[&created.items[0]]);
    ask(&rig, &the_call);
    assert_eq!(
        rig.server.begin(&the_call),
        Err(ApplyError::ItemUnavailable { index: 0 })
    );
}

#[test]
fn a_prompt_nobody_answers_frees_its_items_once_its_code_has_expired() {
    let rig = ServerRig::new(owned(&[10]));
    let created = plan(&rig);
    let the_call = call(&created.plan_id, &[&created.items[0]]);
    let abandoned = ask(&rig, &the_call);
    rig.timer.advance(Duration::from_secs(120));
    let fresh = ask(&rig, &the_call);
    assert_ne!(fresh.nonce, abandoned.nonce);
    assert_eq!(
        rig.server.answer(&the_call, &abandoned.nonce, accept(&abandoned)),
        Err(ApplyError::UnknownRequestState)
    );
    assert!(rig.world.signals().is_empty());
}

#[test]
fn an_item_that_was_applied_is_not_offered_again() {
    let rig = ServerRig::new(owned(&[10]));
    let created = plan(&rig);
    let the_call = call(&created.plan_id, &[&created.items[0]]);
    rig.server.run(&the_call, &mut |c| accept(c)).unwrap();
    assert_eq!(
        rig.server.begin(&the_call),
        Err(ApplyError::ItemUnavailable { index: 0 })
    );
}

#[test]
fn a_plan_is_refused_after_ten_minutes() {
    let rig = ServerRig::new(owned(&[10]));
    let created = plan(&rig);
    let the_call = call(&created.plan_id, &[&created.items[0]]);
    rig.timer
        .advance(Duration::from_secs(600) - Duration::from_nanos(1));
    assert!(matches!(rig.server.begin(&the_call), Ok(Step::Ask(_))));
    let again = ServerRig::new(owned(&[10]));
    let created = plan(&again);
    let the_call = call(&created.plan_id, &[&created.items[0]]);
    again.timer.advance(Duration::from_secs(600));
    assert_eq!(again.server.begin(&the_call), Err(ApplyError::UnknownPlan));
    assert!(again.world.signals().is_empty());
}

#[test]
fn a_plan_that_expires_while_a_prompt_waits_signals_nothing() {
    let rig = ServerRig::new(owned(&[10]));
    let created = plan(&rig);
    let the_call = call(&created.plan_id, &[&created.items[0]]);
    rig.timer.advance(Duration::from_secs(599));
    let challenge = ask(&rig, &the_call);
    rig.timer.advance(Duration::from_secs(5));
    let report = done(
        rig.server
            .answer(&the_call, &challenge.nonce, accept(&challenge))
            .unwrap(),
    );
    assert_eq!(results(&report), [Outcome::PlanExpired]);
    assert!(rig.world.signals().is_empty());
}

#[test]
fn a_new_server_refuses_the_plan_and_the_prompt_of_an_old_one() {
    let rig = ServerRig::new(owned(&[10]));
    let created = plan(&rig);
    let the_call = call(&created.plan_id, &[&created.items[0]]);
    let challenge = ask(&rig, &the_call);
    let restarted = rig.another_server(None);
    assert_eq!(restarted.begin(&the_call), Err(ApplyError::UnknownPlan));
    assert_eq!(
        restarted.answer(&the_call, &challenge.nonce, accept(&challenge)),
        Err(ApplyError::UnknownRequestState)
    );
    assert!(rig.world.signals().is_empty());
}

#[test]
fn apply_false_refuses_a_call_and_still_lets_a_plan_be_made() {
    let rig = ServerRig::new(owned(&[10]));
    write_config(rig.dir.path(), "apply = false\n", 0o600);
    let created = plan(&rig);
    assert_eq!(created.items.len(), 1);
    assert_eq!(
        rig.server.begin(&call(&created.plan_id, &[&created.items[0]])),
        Err(ApplyError::Disabled(Disabled::SwitchedOff))
    );
    assert!(rig.world.signals().is_empty());
}

#[test]
fn a_config_that_cannot_be_read_or_parsed_refuses_a_call() {
    let rig = ServerRig::new(owned(&[10]));
    let created = plan(&rig);
    let the_call = call(&created.plan_id, &[&created.items[0]]);
    write_config(rig.dir.path(), "apply = [", 0o600);
    assert_eq!(
        rig.server.begin(&the_call),
        Err(ApplyError::Disabled(Disabled::Invalid))
    );
    write_config(rig.dir.path(), "apply = true\n", 0o644);
    assert!(matches!(
        rig.server.begin(&the_call),
        Err(ApplyError::Disabled(Disabled::Unsafe(_)))
    ));
    write_config(rig.dir.path(), "apply = true\n", 0o600);
    assert!(matches!(rig.server.begin(&the_call), Ok(Step::Ask(_))));
}

#[test]
fn apply_false_set_while_a_prompt_waits_refuses_the_answer_and_frees_the_items() {
    let rig = ServerRig::new(owned(&[10]));
    let created = plan(&rig);
    let the_call = call(&created.plan_id, &[&created.items[0]]);
    let challenge = ask(&rig, &the_call);
    write_config(rig.dir.path(), "apply = false\n", 0o600);
    assert_eq!(
        rig.server.answer(&the_call, &challenge.nonce, accept(&challenge)),
        Err(ApplyError::Disabled(Disabled::SwitchedOff))
    );
    assert!(rig.world.signals().is_empty());
    write_config(rig.dir.path(), "apply = true\n", 0o600);
    assert!(matches!(rig.server.begin(&the_call), Ok(Step::Ask(_))));
}

#[test]
fn apply_false_set_during_a_batch_stops_the_items_after_it_and_the_units_after_that() {
    use std::sync::OnceLock;
    let dir_slot: Arc<OnceLock<std::path::PathBuf>> = Arc::default();
    let hook_dir = Arc::clone(&dir_slot);
    let rig = ServerRig::hooked(
        vec![
            finding(10, Class::OwnedEnded),
            finding(11, Class::OwnedEnded),
            finding(20, Class::Suspect),
        ],
        Some(Box::new(move |pid| {
            if pid == 10 {
                write_config(hook_dir.get().unwrap(), "apply = false\n", 0o600);
            }
        })),
    );
    dir_slot.set(rig.dir.path().to_path_buf()).unwrap();
    let created = plan(&rig);
    let refs: Vec<_> = created.items.iter().collect();
    let the_call = call(&created.plan_id, &refs);
    let report = rig
        .server
        .run(&the_call, &mut |challenge| accept(challenge))
        .unwrap();
    assert_eq!(
        results(&report),
        [Outcome::Terminated, Outcome::Disabled, Outcome::Disabled]
    );
    assert_eq!(rig.world.signals(), [10]);
}

#[test]
fn the_inspection_report_cannot_be_passed_back_to_apply() {
    let rig = ServerRig::new(owned(&[10]));
    let created = plan(&rig);
    let path = rig.dir.join("inspection").join(&created.report);
    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains(&created.plan_id));
    let item_ids = vec![created.items[0].item_id.clone()];
    for plan_id in [
        created.report.clone(),
        path.to_string_lossy().into_owned(),
        text,
        format!("{}.txt", created.plan_id),
    ] {
        let attempt = Call {
            plan_id,
            item_ids: item_ids.clone(),
        };
        assert_eq!(rig.server.begin(&attempt), Err(ApplyError::UnknownPlan));
    }
    assert!(rig.world.signals().is_empty());
}
