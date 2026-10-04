#![cfg(target_os = "macos")]

use std::fs::{self, File};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

use agentdust_core::identity::KernelIdentity;
use agentdust_core::{darwin, procargs};
use agentdust_testkit::report::{self, Report};
use agentdust_testkit::spec::ProcSpec;
use agentdust_testkit::{Fixture, wait_until};

const FIXTURE: &str = env!("CARGO_BIN_EXE_fixture-sleeper");
const HANG_GUARD: Duration = Duration::from_secs(60);
const SETTLE: Duration = Duration::from_millis(300);
const LIFE: u64 = 120;
const EXIT_AFTER_MS: u64 = 5_000;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("agentdust-flags-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, file: &str) -> PathBuf {
        self.0.join(file)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Started {
    fixture: Fixture,
    spec: ProcSpec,
    report: Report,
    children: Vec<KernelIdentity>,
}

impl Started {
    fn pid(&self) -> i32 {
        self.fixture.0.id() as i32
    }

    fn child(&self, index: usize) -> Report {
        read_child(&self.spec, index).unwrap_or_else(|| panic!("child {index} has no report"))
    }

    fn exit(&mut self) -> ExitStatus {
        let mut status = None;
        let exited = wait_until(
            || {
                status = self.fixture.0.try_wait().unwrap();
                status.is_some()
            },
            HANG_GUARD,
        );
        assert!(exited, "the fixture was still running after {HANG_GUARD:?}");
        status.unwrap()
    }
}

impl Drop for Started {
    fn drop(&mut self) {
        let Ok(boot) = darwin::boot_session_uuid() else {
            return;
        };
        for identity in &self.children {
            let unchanged = matches!(
                darwin::process_info(identity.pid, &boot),
                Ok(Some(info)) if info.identity == *identity
            );
            if unchanged {
                // SAFETY: the kernel identity was just confirmed to be the child this fixture started.
                unsafe { libc::kill(identity.pid, libc::SIGKILL) };
            }
        }
    }
}

fn read_child(spec: &ProcSpec, index: usize) -> Option<Report> {
    let path = spec.child(index).report_file?;
    report::read(&path).unwrap()
}

fn command(spec: &ProcSpec, stderr: File) -> Command {
    let mut command = Command::new(FIXTURE);
    command
        .args(spec.args())
        .envs(spec.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr);
    command
}

fn start_with(dir: &Scratch, spec: ProcSpec, mut command: Command) -> Started {
    let spec = spec.report_file(dir.path("report"));
    let path = spec.report_file.clone().unwrap();
    let mut fixture = Fixture(command.spawn().unwrap());
    let mut found = None;
    wait_until(
        || {
            found = report::read(&path).unwrap();
            found.is_some() || fixture.0.try_wait().unwrap().is_some()
        },
        HANG_GUARD,
    );
    let report = found.unwrap_or_else(|| {
        let status = fixture.0.try_wait().unwrap();
        let stderr = fs::read_to_string(dir.path("stderr")).unwrap_or_default();
        panic!("no report within {HANG_GUARD:?} (exit {status:?}): {stderr}")
    });
    let boot = darwin::boot_session_uuid().unwrap();
    let children = (1..=spec.spawn)
        .filter_map(|index| read_child(&spec, index))
        .filter_map(|child| darwin::process_info(child.pid, &boot).ok().flatten())
        .map(|info| info.identity)
        .collect();
    Started {
        fixture,
        spec,
        report,
        children,
    }
}

fn start(dir: &Scratch, spec: ProcSpec) -> Started {
    let spec = spec.report_file(dir.path("report"));
    let stderr = File::create(dir.path("stderr")).unwrap();
    let command = command(&spec, stderr);
    start_with(dir, spec, command)
}

fn alive(pid: i32) -> bool {
    let boot = darwin::boot_session_uuid().unwrap();
    darwin::process_info(pid, &boot).unwrap().is_some()
}

fn term(pid: i32) {
    // SAFETY: the pid belongs to a fixture process this test started a moment ago.
    let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
    assert_eq!(rc, 0);
}

fn survives(pid: i32) -> bool {
    !wait_until(|| !alive(pid), SETTLE)
}

fn long() -> ProcSpec {
    ProcSpec::new().seconds(LIFE)
}

#[test]
fn the_report_names_the_process_its_parent_and_its_session() {
    let dir = Scratch::new("report");
    let started = start(&dir, long());
    assert_eq!(started.report.pid, started.pid());
    assert_eq!(started.report.ppid, std::process::id() as i32);
    assert_ne!(started.report.sid, started.report.pid);
    assert_eq!(started.report.env, None);
}

#[test]
fn processes_started_together_share_the_session_of_their_parent() {
    let dir = Scratch::new("shared-session");
    let first = start(&dir, long());
    let other = Scratch::new("shared-session-2");
    let second = start(&other, long());
    assert_eq!(first.report.sid, second.report.sid);
}

#[test]
fn a_process_without_flags_dies_on_sigterm() {
    let dir = Scratch::new("cooperative");
    let mut started = start(&dir, long());
    term(started.pid());
    assert_eq!(started.exit().signal(), Some(libc::SIGTERM));
}

#[test]
fn a_process_without_flags_dies_on_sigterm_even_if_it_started_with_sigterm_ignored() {
    let dir = Scratch::new("inherited-ignore");
    let spec = long().report_file(dir.path("report"));
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg("trap '' TERM; exec \"$0\" \"$@\"")
        .arg(FIXTURE)
        .args(spec.args())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(File::create(dir.path("stderr")).unwrap());
    let mut started = start_with(&dir, spec, command);
    term(started.pid());
    assert_eq!(started.exit().signal(), Some(libc::SIGTERM));
}

#[test]
fn a_process_with_ignore_term_survives_sigterm_but_not_sigkill() {
    let dir = Scratch::new("ignore-term");
    let mut started = start(&dir, long().ignore_term());
    term(started.pid());
    assert!(survives(started.pid()));
    assert_eq!(started.fixture.0.try_wait().unwrap(), None);
    started.fixture.0.kill().unwrap();
    assert_eq!(started.exit().signal(), Some(libc::SIGKILL));
}

#[test]
fn the_process_is_ready_to_ignore_sigterm_by_the_time_it_reports() {
    for round in 0..5 {
        let dir = Scratch::new(&format!("ignore-race-{round}"));
        let started = start(&dir, long().ignore_term());
        term(started.pid());
        assert!(survives(started.pid()), "round {round}");
    }
}

#[test]
fn exit_after_ms_ends_the_process_by_itself_after_it_reported() {
    let dir = Scratch::new("exit-after");
    let mut started = start(&dir, long().exit_after_ms(EXIT_AFTER_MS));
    assert_eq!(started.fixture.0.try_wait().unwrap(), None);
    let status = started.exit();
    assert!(status.success(), "{status}");
}

#[test]
fn the_seconds_still_end_a_process_that_has_a_longer_exit_time() {
    let dir = Scratch::new("seconds-win");
    let mut started = start(&dir, ProcSpec::new().seconds(1).exit_after_ms(3_600_000));
    let status = started.exit();
    assert!(status.success(), "{status}");
}

#[test]
fn setsid_makes_the_process_the_leader_of_a_new_session() {
    let dir = Scratch::new("setsid");
    let started = start(&dir, long().setsid());
    assert_eq!(started.report.sid, started.report.pid);
    assert_eq!(started.report.pid, started.pid());
}

#[test]
fn echo_env_reports_the_value_of_the_variable() {
    for (value, expected) in [
        ("value-1", &b"value-1"[..]),
        ("", b""),
        ("two\nlines", b"two\nlines"),
    ] {
        let dir = Scratch::new("echo");
        let started = start(&dir, long().env("AGENTDUST_TAG", value).echo_env("AGENTDUST_TAG"));
        assert_eq!(started.report.env.as_deref(), Some(expected), "{value:?}");
    }
}

#[test]
fn echo_env_reports_nothing_for_a_variable_that_is_not_set() {
    let dir = Scratch::new("echo-unset");
    let started = start(&dir, long().echo_env("AGENTDUST_NOT_SET_ANYWHERE"));
    assert_eq!(started.report.env, None);
}

#[test]
fn a_parent_reports_only_after_every_child_has_reported() {
    let dir = Scratch::new("spawn");
    let started = start(&dir, long().spawn(2));
    let (first, second) = (started.child(1), started.child(2));
    assert_ne!(first.pid, second.pid);
    for child in [&first, &second] {
        assert_ne!(child.pid, started.pid());
        assert_eq!(child.ppid, started.pid());
        assert!(alive(child.pid));
    }
}

#[test]
fn a_child_is_started_with_the_flags_of_its_parent_except_spawn() {
    let dir = Scratch::new("child-argv");
    let started = start(&dir, long().spawn(2).ignore_term().setsid());
    for index in 1..=2 {
        let pid = started.child(index).pid;
        let raw = darwin::procargs2(pid).unwrap().unwrap();
        let parsed = procargs::parse(&raw).unwrap();
        let args: Vec<&[u8]> = parsed.args[1..].to_vec();
        let expected = started.spec.child(index).args();
        let expected: Vec<&[u8]> = expected.iter().map(|arg| arg.as_bytes()).collect();
        assert_eq!(args, expected, "child {index}");
        assert!(!args.contains(&&b"--spawn"[..]));
    }
}

#[test]
fn children_inherit_ignore_term() {
    let dir = Scratch::new("child-ignore");
    let started = start(&dir, long().spawn(2).ignore_term());
    for index in 1..=2 {
        let pid = started.child(index).pid;
        term(pid);
        assert!(survives(pid), "child {index}");
    }
}

#[test]
fn children_without_ignore_term_die_on_sigterm_and_leave_the_parent_alive() {
    let dir = Scratch::new("child-term");
    let mut started = start(&dir, long().spawn(2));
    let pid = started.child(1).pid;
    let other = started.child(2).pid;
    term(pid);
    assert!(wait_until(|| !alive(pid), HANG_GUARD));
    assert!(alive(other));
    assert!(alive(started.pid()));
    assert_eq!(started.fixture.0.try_wait().unwrap(), None);
}

#[test]
fn children_inherit_setsid_and_get_a_session_of_their_own() {
    let dir = Scratch::new("child-setsid");
    let started = start(&dir, long().spawn(2).setsid());
    let (first, second) = (started.child(1), started.child(2));
    assert_eq!(first.sid, first.pid);
    assert_eq!(second.sid, second.pid);
    assert_eq!(started.report.sid, started.report.pid);
    assert_ne!(first.sid, started.report.sid);
}

#[test]
fn children_without_setsid_stay_in_the_session_of_the_parent() {
    let dir = Scratch::new("child-session");
    let started = start(&dir, long().spawn(1));
    assert_eq!(started.child(1).sid, started.report.sid);
}

#[test]
fn children_inherit_the_echoed_variable() {
    let dir = Scratch::new("child-env");
    let started = start(
        &dir,
        long()
            .spawn(1)
            .env("AGENTDUST_TAG", "tree-7")
            .echo_env("AGENTDUST_TAG"),
    );
    assert_eq!(started.child(1).env.as_deref(), Some(&b"tree-7"[..]));
    assert_eq!(started.report.env.as_deref(), Some(&b"tree-7"[..]));
}

#[test]
fn children_inherit_the_exit_time() {
    let dir = Scratch::new("child-exit");
    let started = start(&dir, long().spawn(1).exit_after_ms(1_000));
    let pid = started.child(1).pid;
    assert!(wait_until(|| !alive(pid), HANG_GUARD));
}

#[test]
fn arguments_that_do_not_parse_exit_with_code_2_and_write_no_report() {
    let dir = Scratch::new("bad-args");
    let report = dir.path("report");
    let report = report.to_str().unwrap();
    for args in [
        vec!["--bogus", "--report-file", report],
        vec!["--spawn", "99", "--report-file", report],
        vec!["--spawn"],
        vec!["abc", "--report-file", report],
        vec!["--echo-env", "X"],
        vec!["--setsid", "--setsid", "--report-file", report],
    ] {
        let output = Command::new(FIXTURE).args(&args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(!dir.path("report").exists(), "{args:?}");
        assert!(!String::from_utf8_lossy(&output.stderr).is_empty(), "{args:?}");
    }
}

#[test]
fn a_process_that_cannot_become_a_session_leader_exits_with_code_3() {
    let dir = Scratch::new("group-leader");
    let output = Command::new(FIXTURE)
        .args(long().setsid().report_file(dir.path("report")).args())
        .process_group(0)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert!(!dir.path("report").exists());
}

#[test]
fn a_report_that_cannot_be_written_ends_the_process_with_code_6() {
    let dir = Scratch::new("unwritable");
    let output = Command::new(FIXTURE)
        .args(long().report_file(dir.path("missing-directory/report")).args())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(6));
}

#[test]
fn a_parent_whose_child_cannot_report_exits_with_code_5_and_leaves_no_child_behind() {
    let dir = Scratch::new("child-fails");
    let spec = long().spawn(2).report_file(dir.path("report"));
    fs::create_dir(dir.path("report.2")).unwrap();
    let status = command(&spec, File::create(dir.path("stderr")).unwrap())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(5));
    let stderr = fs::read_to_string(dir.path("stderr")).unwrap();
    assert!(stderr.contains("child 2"), "{stderr}");
    assert!(!dir.path("report").exists());
    let first = read_child(&spec, 1).expect("the first child reported before the second failed");
    assert!(!alive(first.pid));
}
