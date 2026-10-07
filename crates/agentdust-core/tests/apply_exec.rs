mod apply_support;
mod scratch;

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use agentdust_core::apply::exec::{Outcome, Reason, Settings, Verdict};
use agentdust_core::apply::lock::IdentityLock;
use agentdust_core::apply::signal::SignalResult;
use agentdust_core::apply::timer::Timer;
use agentdust_core::class::Class;
use agentdust_core::classifier::Finding;
use agentdust_core::identity::{KernelIdentity, ProcessIdentity};
use agentdust_core::provider::ProcessRead;
use apply_support::{PLAN, Rig, audit_len, finding, finding_at, item, kernel, present, write_config};

type IdentityEdit = Box<dyn Fn(&mut ProcessIdentity)>;
type ReadScript = Box<dyn Fn(usize) -> io::Result<ProcessRead> + Send + Sync>;

fn verdict(outcome: Outcome) -> Verdict {
    Verdict {
        outcome,
        reason: None,
    }
}

fn failed(reason: Reason) -> Verdict {
    Verdict {
        outcome: Outcome::RevalidationFailed,
        reason: Some(reason),
    }
}

fn lock_name(item: &agentdust_core::plan::PlanItem) -> String {
    format!("{}.lock", item.model.item_id)
}

#[test]
fn the_defaults_follow_the_spec() {
    let settings = Settings::default();
    assert_eq!(settings.term_wait, Duration::from_secs(5));
    assert_eq!(settings.plan_ttl, Duration::from_secs(600));
    assert_eq!(settings.code_ttl, Duration::from_secs(120));
    assert_eq!(settings.audit_max_bytes, 5 * 1024 * 1024);
    assert!(settings.poll_interval > Duration::ZERO && settings.poll_interval <= Duration::from_millis(100));
}

#[test]
fn a_cooperative_process_is_signalled_once_and_reported_terminated() {
    let item = item(4242, Class::OwnedEnded);
    let rig = Rig::for_item(&item).build();
    assert_eq!(rig.executor.execute(PLAN, &item), verdict(Outcome::Terminated));
    assert_eq!(rig.signals(), [4242]);
    assert_eq!(rig.surveys(), 1);
    let audit = rig.audit();
    assert_eq!(audit.len(), 2);
    assert_eq!(audit[0]["phase"], "attempt");
    assert_eq!(audit[0]["result"], serde_json::Value::Null);
    assert_eq!(audit[1]["phase"], "result");
    assert_eq!(audit[1]["result"], "terminated");
    assert_eq!(audit[1]["plan"], PLAN);
    assert_eq!(audit[1]["item"], item.model.item_id);
    assert_eq!(audit[1]["pid"], 4242);
    assert!(!rig.locks().join(lock_name(&item)).exists());
}

#[test]
fn a_process_that_ignores_sigterm_is_a_survivor_after_the_wait_and_gets_one_signal() {
    let item = item(4242, Class::Suspect);
    let stays = present(&item);
    let rig = Rig::for_item(&item).reads(move |_| Ok(stays.clone())).build();
    assert_eq!(rig.executor.execute(PLAN, &item), verdict(Outcome::Survivor));
    assert_eq!(rig.signals(), [4242]);
    let waited = rig.timer.now();
    let settings = Settings::default();
    assert!(waited >= settings.term_wait, "{waited:?}");
    assert!(
        waited <= settings.term_wait + settings.poll_interval,
        "{waited:?}"
    );
    assert_eq!(rig.audit_results(), ["survivor"]);
}

#[test]
fn a_process_that_ends_during_the_wait_is_terminated_early() {
    let item = item(4242, Class::OwnedEnded);
    let alive = present(&item);
    let rig = Rig::for_item(&item)
        .reads(move |call| {
            Ok(if call < 3 {
                alive.clone()
            } else {
                ProcessRead::Gone
            })
        })
        .build();
    assert_eq!(rig.executor.execute(PLAN, &item), verdict(Outcome::Terminated));
    assert!(rig.timer.now() < Settings::default().term_wait);
}

#[test]
fn a_pid_taken_over_during_the_wait_means_the_process_ended() {
    let item = item(4242, Class::OwnedEnded);
    let alive = present(&item);
    let mut successor = item.identity.clone();
    successor.kernel.start_time_us += 1;
    let rig = Rig::for_item(&item)
        .reads(move |call| {
            Ok(if call < 2 {
                alive.clone()
            } else {
                ProcessRead::Present(successor.clone())
            })
        })
        .build();
    assert_eq!(rig.executor.execute(PLAN, &item), verdict(Outcome::Terminated));
}

#[test]
fn a_process_that_runs_another_program_or_cannot_be_read_is_still_alive() {
    let item = item(4242, Class::OwnedEnded);
    let alive = present(&item);
    let mut exec = item.identity.clone();
    exec.evidence.exe_path = "/usr/bin/other".into();
    let kern = item.identity.kernel.clone();
    let scripts: Vec<ReadScript> = vec![
        Box::new(move |call| {
            Ok(if call == 0 {
                alive.clone()
            } else {
                ProcessRead::Present(exec.clone())
            })
        }),
        Box::new({
            let alive = present(&item);
            move |call| {
                Ok(if call == 0 {
                    alive.clone()
                } else {
                    ProcessRead::PathUnreadable(kern.clone())
                })
            }
        }),
        Box::new({
            let alive = present(&item);
            move |call| {
                if call == 0 {
                    Ok(alive.clone())
                } else {
                    Err(io::Error::other("read failed"))
                }
            }
        }),
    ];
    for script in scripts {
        let rig = Rig::for_item(&item).reads(script).build();
        assert_eq!(rig.executor.execute(PLAN, &item), verdict(Outcome::Survivor));
    }
}

#[test]
fn a_pid_the_kernel_no_longer_has_is_gone_and_is_not_polled() {
    let item = item(4242, Class::OwnedEnded);
    let rig = Rig::for_item(&item).answer(SignalResult::NoSuchProcess).build();
    assert_eq!(rig.executor.execute(PLAN, &item), verdict(Outcome::Gone));
    assert_eq!(rig.signals(), [4242]);
    assert_eq!(rig.reads(), 1);
    assert_eq!(rig.audit_results(), ["gone"]);
}

#[test]
fn a_signal_that_fails_is_reported_and_is_not_polled() {
    for answer in [SignalResult::Failed(1), SignalResult::Refused] {
        let item = item(4242, Class::OwnedEnded);
        let rig = Rig::for_item(&item).answer(answer).build();
        assert_eq!(rig.executor.execute(PLAN, &item), verdict(Outcome::SignalFailed));
        assert_eq!(rig.reads(), 1);
        assert_eq!(rig.audit_results(), ["signal_failed"]);
    }
}

fn aborts_before_the_read(
    planned: Class,
    script: impl Fn(usize) -> io::Result<Vec<Finding>> + Send + Sync + 'static,
    expected: Verdict,
) {
    let item = item(4242, planned);
    let rig = Rig::for_item(&item).survey(script).build();
    assert_eq!(rig.executor.execute(PLAN, &item), expected);
    assert!(rig.signals().is_empty());
    assert_eq!(rig.reads(), 0);
    assert_eq!(rig.surveys(), 1);
    let audit = rig.audit();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0]["phase"], "result");
    assert_eq!(audit[0]["result"], expected.outcome.code());
    assert_eq!(
        audit[0]["reason"],
        expected
            .reason
            .map_or(serde_json::Value::Null, |reason| reason.code().into())
    );
    assert!(!rig.locks().join(lock_name(&item)).exists());
}

#[test]
fn a_process_missing_from_the_fresh_inventory_is_gone() {
    aborts_before_the_read(Class::OwnedEnded, |_| Ok(Vec::new()), verdict(Outcome::Gone));
    aborts_before_the_read(
        Class::Suspect,
        |_| Ok(vec![finding(4243, Class::Suspect)]),
        verdict(Outcome::Gone),
    );
}

#[test]
fn a_pid_with_another_start_time_is_another_process() {
    aborts_before_the_read(
        Class::OwnedEnded,
        |_| {
            let mut other = finding(4242, Class::OwnedEnded);
            other.identity.kernel.start_time_us += 5;
            Ok(vec![other])
        },
        failed(Reason::IdentityChanged),
    );
}

#[test]
fn a_changed_owner_after_approval_aborts_before_reading_or_signalling() {
    for scenario in ["changed identity", "additional owner", "removed owner"] {
        aborts_before_the_read(
            Class::OwnedEnded,
            move |_| {
                let mut changed = finding(4242, Class::OwnedEnded);
                let owners = changed.attribution_owners.as_mut().unwrap();
                match scenario {
                    "changed identity" => owners[0].identity.start_time_us += 1,
                    "additional owner" => {
                        let mut extra = owners[0].clone();
                        extra.session_id = "session-extra".to_owned();
                        extra.identity = kernel(4343);
                        owners.push(extra);
                    }
                    "removed owner" => owners.clear(),
                    _ => unreachable!(),
                }
                Ok(vec![changed])
            },
            failed(Reason::OwnershipChanged),
        );
    }
}

#[test]
fn any_change_of_class_aborts_the_item() {
    for fresh in [
        Class::Managed,
        Class::OwnedLive,
        Class::Unknown,
        Class::LikelyOwned,
        Class::Suspect,
    ] {
        aborts_before_the_read(
            Class::OwnedEnded,
            move |_| Ok(vec![finding(4242, fresh)]),
            failed(Reason::ClassChanged),
        );
    }
    for fresh in [
        Class::Managed,
        Class::OwnedLive,
        Class::Unknown,
        Class::OwnedEnded,
    ] {
        aborts_before_the_read(
            Class::Suspect,
            move |_| Ok(vec![finding(4242, fresh)]),
            failed(Reason::ClassChanged),
        );
    }
}

#[test]
fn a_new_executable_path_aborts_the_item() {
    aborts_before_the_read(
        Class::OwnedEnded,
        |_| Ok(vec![finding_at(4242, Class::OwnedEnded, "/usr/local/bin/node")]),
        failed(Reason::PathChanged),
    );
}

#[test]
fn a_path_that_can_no_longer_be_read_aborts_the_item() {
    aborts_before_the_read(
        Class::OwnedEnded,
        |_| {
            let mut found = finding(4242, Class::OwnedEnded);
            found.identity.exe_path = None;
            Ok(vec![found])
        },
        failed(Reason::Unreadable),
    );
}

#[test]
fn a_failed_inventory_aborts_the_item() {
    aborts_before_the_read(
        Class::Suspect,
        |_| Err(io::Error::other("scan failed")),
        failed(Reason::SurveyFailed),
    );
}

#[test]
fn the_fresh_inventory_is_taken_for_every_run_of_an_item() {
    let item = item(4242, Class::OwnedEnded);
    let rig = Rig::for_item(&item).build();
    rig.executor.execute(PLAN, &item);
    rig.executor.execute(PLAN, &item);
    assert_eq!(rig.surveys(), 2);
}

#[test]
fn a_changed_identity_between_the_read_and_the_signal_aborts_without_a_signal() {
    let item = item(4242, Class::OwnedEnded);
    let edits: Vec<(&str, IdentityEdit, Verdict)> = vec![
        (
            "start time",
            Box::new(|id| id.kernel.start_time_us += 1),
            failed(Reason::IdentityChanged),
        ),
        (
            "owner",
            Box::new(|id| id.kernel.uid += 1),
            failed(Reason::IdentityChanged),
        ),
        (
            "boot",
            Box::new(|id| id.kernel.boot_session_uuid = "boot-2".to_owned()),
            failed(Reason::IdentityChanged),
        ),
        (
            "path",
            Box::new(|id| id.evidence.exe_path = "/usr/bin/other".into()),
            failed(Reason::PathChanged),
        ),
    ];
    for (name, edit, expected) in edits {
        let mut shifted = item.identity.clone();
        edit(&mut shifted);
        let rig = Rig::for_item(&item)
            .reads(move |_| Ok(ProcessRead::Present(shifted.clone())))
            .build();
        assert_eq!(rig.executor.execute(PLAN, &item), expected, "{name}");
        assert!(rig.signals().is_empty(), "{name}");
        assert_eq!(rig.reads(), 1, "{name}");
        assert_eq!(rig.audit_results(), [expected.outcome.code()], "{name}");
    }
}

#[test]
fn a_path_that_cannot_be_read_at_the_last_moment_fails_closed() {
    let item = item(4242, Class::OwnedEnded);
    let same = item.identity.kernel.clone();
    let mut other: KernelIdentity = kernel(4242);
    other.start_time_us += 9;
    let cases: Vec<(ReadScript, Verdict)> = vec![
        (
            Box::new(move |_| Ok(ProcessRead::PathUnreadable(same.clone()))),
            failed(Reason::Unreadable),
        ),
        (
            Box::new(move |_| Ok(ProcessRead::PathUnreadable(other.clone()))),
            failed(Reason::IdentityChanged),
        ),
        (
            Box::new(|_| Err(io::Error::other("read failed"))),
            failed(Reason::Unreadable),
        ),
        (Box::new(|_| Ok(ProcessRead::Gone)), verdict(Outcome::Gone)),
    ];
    for (script, expected) in cases {
        let rig = Rig::for_item(&item).reads(script).build();
        assert_eq!(rig.executor.execute(PLAN, &item), expected);
        assert!(rig.signals().is_empty());
    }
}

#[test]
fn nothing_is_written_between_the_identity_read_and_the_signal() {
    let item = item(4242, Class::OwnedEnded);
    let rig = Rig::for_item(&item).build();
    rig.executor.execute(PLAN, &item);
    let events = rig.events.all();
    assert_eq!(events[0], "survey len=0");
    let length = events[1]
        .strip_prefix("read len=")
        .expect("the identity read follows the inventory")
        .parse::<u64>()
        .unwrap();
    assert!(length > 0, "the attempt is on record before the read");
    assert_eq!(events[2], format!("sigterm len={length}"));
    assert!(events[3].starts_with("read len="));
}

#[test]
fn a_second_taker_is_told_the_item_is_handled_elsewhere() {
    let item = item(4242, Class::OwnedEnded);
    let rig = Rig::for_item(&item).build();
    let held = IdentityLock::try_acquire(&rig.locks(), &item.model.item_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        rig.executor.execute(PLAN, &item),
        verdict(Outcome::HandledElsewhere)
    );
    assert_eq!(rig.surveys(), 0);
    assert_eq!(rig.reads(), 0);
    assert!(rig.signals().is_empty());
    assert!(held.path().exists());
    assert_eq!(rig.audit_results(), ["handled_elsewhere"]);
    drop(held);
    assert_eq!(rig.executor.execute(PLAN, &item), verdict(Outcome::Terminated));
}

#[test]
fn the_lock_is_held_while_the_signal_goes_out_and_free_afterwards() {
    let item = item(4242, Class::OwnedEnded);
    let builder = Rig::for_item(&item);
    let locks = builder.dir().join("locks");
    let name = item.model.item_id.clone();
    let held_at_signal = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&held_at_signal);
    let rig = builder
        .on_signal(move |_| {
            let taken = IdentityLock::try_acquire(&locks, &name).unwrap();
            flag.store(taken.is_none(), Ordering::SeqCst);
        })
        .build();
    rig.executor.execute(PLAN, &item);
    assert!(held_at_signal.load(Ordering::SeqCst));
    assert!(
        IdentityLock::try_acquire(&rig.locks(), &item.model.item_id)
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_lock_directory_that_cannot_be_used_stops_the_item_before_anything_else() {
    let item = item(4242, Class::OwnedEnded);
    let rig = Rig::for_item(&item).build();
    fs::write(rig.locks(), b"not a directory").unwrap();
    assert_eq!(
        rig.executor.execute(PLAN, &item),
        verdict(Outcome::LockUnavailable)
    );
    assert_eq!(rig.surveys(), 0);
    assert!(rig.signals().is_empty());
}

#[test]
fn an_audit_log_that_cannot_be_written_stops_the_item_before_the_signal() {
    let item = item(4242, Class::OwnedEnded);
    let rig = Rig::for_item(&item).build();
    let target = rig.dir.join("elsewhere");
    fs::write(&target, b"keep").unwrap();
    std::os::unix::fs::symlink(&target, rig.dir.join("audit.log")).unwrap();
    assert_eq!(
        rig.executor.execute(PLAN, &item),
        verdict(Outcome::AuditUnavailable)
    );
    assert!(rig.signals().is_empty());
    assert_eq!(rig.reads(), 0);
    assert_eq!(fs::read(&target).unwrap(), b"keep");
    assert!(!rig.locks().join(lock_name(&item)).exists());
}

#[test]
fn a_result_line_that_cannot_be_written_does_not_change_what_happened() {
    let item = item(4242, Class::OwnedEnded);
    let builder = Rig::for_item(&item);
    let audit = builder.dir().join("audit.log");
    let rig = builder
        .on_signal(move |_| {
            fs::set_permissions(&audit, fs::Permissions::from_mode(0o644)).unwrap();
        })
        .build();
    assert_eq!(rig.executor.execute(PLAN, &item), verdict(Outcome::Terminated));
    assert_eq!(rig.signals(), [4242]);
    assert!(audit_len(rig.dir.path()) > 0);
    assert_eq!(rig.audit().len(), 1);
}

#[test]
fn apply_false_stops_the_item_before_any_file_or_read() {
    let item = item(4242, Class::OwnedEnded);
    let rig = Rig::for_item(&item).build();
    write_config(rig.dir.path(), "apply = false\n", 0o600);
    assert_eq!(rig.executor.execute(PLAN, &item), verdict(Outcome::Disabled));
    assert_eq!(rig.surveys(), 0);
    assert_eq!(rig.reads(), 0);
    assert!(rig.signals().is_empty());
    assert!(!rig.locks().exists());
    assert!(!rig.dir.join("audit.log").exists());
}

#[test]
fn a_config_that_cannot_be_trusted_stops_the_item() {
    let item = item(4242, Class::OwnedEnded);
    for (text, mode) in [("apply = what\n", 0o600), ("apply = true\n", 0o644)] {
        let rig = Rig::for_item(&item).build();
        write_config(rig.dir.path(), text, mode);
        assert_eq!(rig.executor.execute(PLAN, &item), verdict(Outcome::Disabled));
        assert!(rig.signals().is_empty());
    }
}

#[test]
fn the_switch_is_read_each_time_an_item_runs() {
    let item = item(4242, Class::OwnedEnded);
    let rig = Rig::for_item(&item).build();
    assert_eq!(rig.executor.execute(PLAN, &item), verdict(Outcome::Terminated));
    write_config(rig.dir.path(), "apply = false\n", 0o600);
    assert_eq!(rig.executor.execute(PLAN, &item), verdict(Outcome::Disabled));
    assert_eq!(rig.signals().len(), 1);
}

#[test]
fn the_wait_and_the_poll_interval_come_from_the_settings() {
    let item = item(4242, Class::Suspect);
    let stays = present(&item);
    let rig = Rig::for_item(&item)
        .reads(move |_| Ok(stays.clone()))
        .settings(|settings| {
            settings.term_wait = Duration::from_millis(300);
            settings.poll_interval = Duration::from_millis(100);
        })
        .build();
    assert_eq!(rig.executor.execute(PLAN, &item), verdict(Outcome::Survivor));
    assert_eq!(rig.timer.now(), Duration::from_millis(300));
    assert_eq!(rig.reads(), 1 + 4);
}
