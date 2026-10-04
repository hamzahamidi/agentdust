#![cfg(target_os = "macos")]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::time::{Duration, Instant};

use agentdust_core::darwin::DarwinProvider;
use agentdust_core::identity::ProcessIdentity;
use agentdust_core::revalidate::{Revalidation, revalidate};
use agentdust_testkit::harness::{Error, Harness, Signal};
use agentdust_testkit::spec::ProcSpec;
use agentdust_testkit::wait_until;

const FIXTURE: &str = env!("CARGO_BIN_EXE_fixture-sleeper");
const HANG_GUARD: Duration = Duration::from_secs(60);
const SETTLE: Duration = Duration::from_millis(300);
const QUICK_SETTLE: Duration = Duration::from_millis(100);
const LIFE: u64 = 120;
const EXIT_AFTER_MS: u64 = 5_000;

fn harness() -> Harness {
    Harness::new(FIXTURE).unwrap()
}

fn long() -> ProcSpec {
    ProcSpec::new().seconds(LIFE)
}

fn gone(identity: &ProcessIdentity) -> bool {
    let provider = DarwinProvider::new().unwrap();
    revalidate(identity, &provider) != Revalidation::Match
}

#[test]
fn a_spawned_process_is_registered_with_its_kernel_identity_and_its_report() {
    let mut harness = harness();
    let handle = harness.spawn(&long()).unwrap();
    let pid = harness.pid(handle);
    assert!(pid > 0);
    assert_eq!(harness.identity(handle).kernel.pid, pid);
    assert_eq!(harness.report(handle).pid, pid);
    assert_eq!(harness.report(handle).ppid, std::process::id() as i32);
    assert_eq!(harness.ppid(handle).unwrap(), std::process::id() as i32);
    assert_eq!(
        harness.identity(handle).evidence.exe_path.canonicalize().unwrap(),
        Path::new(FIXTURE).canonicalize().unwrap()
    );
    assert_eq!(harness.revalidate(handle), Revalidation::Match);
    assert!(harness.signal_log().is_empty());
}

#[test]
fn the_variable_a_process_was_asked_to_echo_is_in_its_report() {
    let mut harness = harness();
    let spec = long().env("AGENTDUST_TAG", "tag-9").echo_env("AGENTDUST_TAG");
    let handle = harness.spawn(&spec).unwrap();
    assert_eq!(harness.report(handle).env.as_deref(), Some(&b"tag-9"[..]));
    let plain = harness.spawn(&long()).unwrap();
    assert_eq!(harness.report(plain).env, None);
    assert_eq!(harness.report(plain).sid, harness.report(handle).sid);
}

#[test]
fn a_cooperative_process_dies_on_sigterm() {
    let mut harness = harness();
    let handle = harness.spawn(&long()).unwrap();
    harness.signal(handle, Signal::Term).unwrap();
    assert!(wait_until(
        || harness.revalidate(handle) == Revalidation::Gone,
        HANG_GUARD
    ));
}

#[test]
fn a_sigterm_ignoring_process_survives_and_the_log_shows_one_sigterm_to_that_pid() {
    let mut harness = harness();
    let bystander = harness.spawn(&long()).unwrap();
    let handle = harness.spawn(&long().ignore_term()).unwrap();
    harness.signal(handle, Signal::Term).unwrap();
    assert!(!wait_until(
        || harness.revalidate(handle) != Revalidation::Match,
        SETTLE
    ));
    let log = harness.signal_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0].pid, harness.pid(handle));
    assert_eq!(log[0].signal, Signal::Term);
    assert_ne!(log[0].pid, harness.pid(bystander));
    assert_eq!(harness.revalidate(bystander), Revalidation::Match);
}

#[test]
fn a_sigterm_ignoring_process_is_never_caught_by_a_signal_sent_right_after_spawn() {
    let mut harness = harness();
    for round in 0..5 {
        let handle = harness.spawn(&long().ignore_term()).unwrap();
        harness.signal(handle, Signal::Term).unwrap();
        assert!(
            !wait_until(|| harness.revalidate(handle) != Revalidation::Match, QUICK_SETTLE),
            "round {round}"
        );
    }
}

#[test]
fn a_sigterm_ignoring_process_dies_on_sigkill_and_the_log_says_so() {
    let mut harness = harness();
    let handle = harness.spawn(&long().ignore_term()).unwrap();
    harness.signal(handle, Signal::Term).unwrap();
    harness.signal(handle, Signal::Kill).unwrap();
    assert!(wait_until(
        || harness.revalidate(handle) == Revalidation::Gone,
        HANG_GUARD
    ));
    let sent: Vec<_> = harness.signal_log().iter().map(|e| (e.pid, e.signal)).collect();
    let pid = harness.pid(handle);
    assert_eq!(sent, vec![(pid, Signal::Term), (pid, Signal::Kill)]);
}

#[test]
fn every_signal_is_logged_in_order_with_its_pid_and_time() {
    let mut harness = harness();
    let first = harness.spawn(&long()).unwrap();
    let second = harness.spawn(&long().ignore_term()).unwrap();
    let before = Instant::now();
    harness.signal(first, Signal::Term).unwrap();
    harness.signal(second, Signal::Kill).unwrap();
    let after = Instant::now();
    let log = harness.signal_log();
    assert_eq!(log.len(), 2);
    assert_eq!((log[0].pid, log[0].signal), (harness.pid(first), Signal::Term));
    assert_eq!((log[1].pid, log[1].signal), (harness.pid(second), Signal::Kill));
    assert!(before <= log[0].at && log[0].at <= log[1].at && log[1].at <= after);
}

#[test]
fn a_process_that_exits_during_the_approval_window_reads_gone_through_revalidate() {
    let mut harness = harness();
    let handle = harness.spawn(&long().exit_after_ms(EXIT_AFTER_MS)).unwrap();
    let approved = harness.identity(handle).clone();
    assert_eq!(revalidate(&approved, harness.provider()), Revalidation::Match);
    assert!(wait_until(
        || revalidate(&approved, harness.provider()) != Revalidation::Match,
        HANG_GUARD
    ));
    assert_eq!(revalidate(&approved, harness.provider()), Revalidation::Gone);
    assert_eq!(harness.revalidate(handle), Revalidation::Gone);
}

#[test]
fn a_process_that_has_exited_cannot_be_signalled_and_nothing_is_logged() {
    let mut harness = harness();
    let handle = harness.spawn(&long().exit_after_ms(EXIT_AFTER_MS)).unwrap();
    assert!(wait_until(
        || harness.revalidate(handle) == Revalidation::Gone,
        HANG_GUARD
    ));
    let pid = harness.pid(handle);
    for signal in [Signal::Term, Signal::Kill] {
        let result = harness.signal(handle, signal);
        assert!(
            matches!(result, Err(Error::NotRunning(p)) if p == pid),
            "{result:?}"
        );
    }
    assert!(harness.signal_log().is_empty());
}

#[test]
fn a_handle_from_another_harness_is_refused_and_nothing_is_signalled() {
    let mut owner = harness();
    let mut other = harness();
    let handle = owner.spawn(&long()).unwrap();
    let bystander = other.spawn(&long()).unwrap();
    let result = other.signal(handle, Signal::Kill);
    assert!(matches!(result, Err(Error::ForeignHandle)), "{result:?}");
    assert!(other.signal_log().is_empty());
    assert!(owner.signal_log().is_empty());
    assert!(!wait_until(
        || owner.revalidate(handle) != Revalidation::Match
            || other.revalidate(bystander) != Revalidation::Match,
        SETTLE
    ));
}

#[test]
fn a_spec_that_asks_for_children_is_refused_by_spawn() {
    let mut harness = harness();
    let result = harness.spawn(&long().spawn(2));
    assert!(matches!(result, Err(Error::TreeRequested)), "{result:?}");
}

#[test]
fn a_fixture_that_exits_before_it_reports_is_an_error_with_its_stderr() {
    let mut harness = Harness::new("/bin/sleep").unwrap();
    let result = harness.spawn(&ProcSpec::new());
    let Err(Error::ExitedBeforeReady { status, stderr }) = result else {
        panic!("{result:?}");
    };
    assert!(!status.success());
    assert!(!stderr.is_empty());
}

#[test]
fn a_fixture_that_does_not_exist_is_an_error() {
    assert!(matches!(Harness::new("/nonexistent/fixture"), Err(Error::Io(_))));
}

#[test]
fn no_fixture_process_survives_the_drop() {
    let mut harness = harness();
    let specs = [long(), long().ignore_term(), long().ignore_term().setsid()];
    let identities: Vec<ProcessIdentity> = specs
        .iter()
        .map(|spec| {
            let handle = harness.spawn(spec).unwrap();
            harness.identity(handle).clone()
        })
        .collect();
    drop(harness);
    for identity in &identities {
        assert!(gone(identity), "{identity:?}");
    }
}

#[test]
fn dropping_after_a_process_has_already_exited_is_clean() {
    let mut harness = harness();
    let handle = harness.spawn(&long().exit_after_ms(EXIT_AFTER_MS)).unwrap();
    let identity = harness.identity(handle).clone();
    assert!(wait_until(
        || harness.revalidate(handle) == Revalidation::Gone,
        HANG_GUARD
    ));
    drop(harness);
    assert!(gone(&identity));
}

#[test]
fn no_fixture_process_survives_a_panic() {
    let mut identities = Vec::new();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut harness = harness();
        for spec in [long(), long().ignore_term().setsid()] {
            let handle = harness.spawn(&spec).unwrap();
            identities.push(harness.identity(handle).clone());
        }
        panic!("a test failed while its fixtures were running");
    }));
    assert!(result.is_err());
    assert_eq!(identities.len(), 2);
    for identity in &identities {
        assert!(gone(identity), "{identity:?}");
    }
}

#[test]
fn shutdown_kills_everything_and_reports_no_survivor() {
    let mut harness = harness();
    let handle = harness.spawn(&long().ignore_term()).unwrap();
    let identity = harness.identity(handle).clone();
    harness.shutdown().unwrap();
    assert!(gone(&identity));
}

#[test]
fn the_scratch_directory_is_removed_on_drop() {
    let harness = harness();
    let dir = harness.scratch_dir().to_path_buf();
    assert!(dir.is_dir());
    drop(harness);
    assert!(!dir.exists());
}

#[test]
fn the_scratch_directory_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let harness = harness();
    let mode = harness.scratch_dir().metadata().unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
}

#[test]
fn two_harnesses_never_share_a_scratch_directory() {
    let first = harness();
    let second = harness();
    assert_ne!(first.scratch_dir(), second.scratch_dir());
}
