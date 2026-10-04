use std::process::{Child, Command, Stdio};
use std::time::Duration;

use agentdust_core::darwin::{self, DarwinProvider};
use agentdust_core::provider::{ProcessProvider, ProcessRead};
use agentdust_core::revalidate::{Field, Revalidation, revalidate};

use super::{Entry, Error, Harness, Launched, Signal};
use crate::report::Report;
use crate::wait_until;

const HANG_GUARD: Duration = Duration::from_secs(60);
const SETTLE: Duration = Duration::from_millis(300);

fn sleeper() -> Child {
    Command::new("/bin/sleep")
        .arg("120")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn harness() -> Harness {
    Harness::new("/bin/sleep").unwrap()
}

fn register(harness: &mut Harness, child: Child) -> i32 {
    let pid = child.id() as i32;
    let ProcessRead::Present(identity) = harness.provider.read(pid).unwrap() else {
        panic!("the sleeper should be readable");
    };
    let report = Report {
        pid,
        ppid: 0,
        sid: 0,
        env: None,
    };
    harness.entries.push(Entry::direct(pid, identity, report, child));
    pid
}

fn child_of(harness: &mut Harness, pid: i32) -> &mut Child {
    let entry = harness.entries.iter_mut().find(|e| e.pid == pid).unwrap();
    entry.child.as_mut().expect("not a direct child")
}

fn survives(child: &mut Child) -> bool {
    std::thread::sleep(SETTLE);
    child.try_wait().unwrap().is_none()
}

fn alive(pid: i32) -> bool {
    let boot = darwin::boot_session_uuid().unwrap();
    darwin::process_info(pid, &boot).unwrap().is_some()
}

#[test]
fn a_pid_that_is_not_registered_is_refused_and_never_signalled() {
    let mut harness = harness();
    register(&mut harness, sleeper());
    let mut stranger = sleeper();
    let pid = stranger.id() as i32;
    let result = harness.deliver(pid, Signal::Term);
    assert!(
        matches!(result, Err(Error::Unregistered(p)) if p == pid),
        "{result:?}"
    );
    assert!(survives(&mut stranger));
    assert!(harness.signal_log().is_empty());
    stranger.kill().unwrap();
    stranger.wait().unwrap();
}

#[test]
fn a_registered_pid_with_the_identity_it_had_is_signalled_and_logged() {
    let mut harness = harness();
    let pid = register(&mut harness, sleeper());
    let identity = harness.entries[0].identity.clone();
    harness.deliver(pid, Signal::Term).unwrap();
    assert!(wait_until(
        || revalidate(&identity, &harness.provider) == Revalidation::Gone,
        HANG_GUARD
    ));
    let log = harness.signal_log();
    assert_eq!(log.len(), 1);
    assert_eq!((log[0].pid, log[0].signal), (pid, Signal::Term));
}

#[test]
fn a_registered_pid_whose_identity_no_longer_matches_is_refused_for_each_field() {
    type Tamper = fn(&mut Entry);
    let cases: [(Field, Tamper); 3] = [
        (Field::StartTime, |e| e.identity.kernel.start_time_us += 1),
        (Field::Uid, |e| e.identity.kernel.uid += 1),
        (Field::ExePath, |e| e.identity.evidence.exe_path.push("other")),
    ];
    for (field, tamper) in cases {
        let mut harness = harness();
        let pid = register(&mut harness, sleeper());
        tamper(&mut harness.entries[0]);
        let result = harness.deliver(pid, Signal::Term);
        assert!(
            matches!(&result, Err(Error::IdentityChanged { pid: p, found }) if *p == pid && *found == Revalidation::Changed(field)),
            "{field:?}: {result:?}"
        );
        assert!(survives(child_of(&mut harness, pid)), "{field:?}");
        assert!(harness.signal_log().is_empty(), "{field:?}");
    }
}

#[test]
fn a_registered_pid_that_has_exited_is_refused_and_not_logged() {
    let mut harness = harness();
    let pid = register(&mut harness, sleeper());
    let child = child_of(&mut harness, pid);
    child.kill().unwrap();
    child.wait().unwrap();
    let result = harness.deliver(pid, Signal::Kill);
    assert!(
        matches!(result, Err(Error::NotRunning(p)) if p == pid),
        "{result:?}"
    );
    assert!(harness.signal_log().is_empty());
}

#[test]
fn dropping_kills_a_registered_child_even_when_its_identity_cannot_be_revalidated() {
    let mut harness = harness();
    let pid = register(&mut harness, sleeper());
    let identity = harness.entries[0].identity.clone();
    harness.entries[0].identity.kernel.start_time_us += 1;
    drop(harness);
    let provider = DarwinProvider::new().unwrap();
    assert_ne!(revalidate(&identity, &provider), Revalidation::Match, "pid {pid}");
}

#[test]
fn a_fixture_that_does_not_report_in_time_is_an_error_and_is_killed_with_its_launch() {
    let mut harness = harness();
    let ready = Duration::from_millis(50);
    harness.ready = ready;
    let child = sleeper();
    let pid = child.id() as i32;
    let mut launched = Launched {
        child: Some(child),
        report_path: harness.scratch.join("never.report"),
        stderr_path: harness.scratch.join("never.stderr"),
    };
    let result = harness.await_report(&mut launched);
    assert!(
        matches!(result, Err(Error::NotReady(waited)) if waited == ready),
        "{result:?}"
    );
    assert!(alive(pid));
    assert!(harness.entries.is_empty());
    drop(launched);
    assert!(!alive(pid));
}
