use std::io;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::process::parent_id;
use std::process::{Child, Command, ExitCode, Stdio};
use std::thread;
use std::time::Duration;

use agentdust_testkit::report::{self, Report};
use agentdust_testkit::spec::ProcSpec;
use agentdust_testkit::wait_until;

const CHILD_READY: Duration = Duration::from_secs(60);
const BAD_ARGUMENTS: u8 = 2;
const NO_SESSION: u8 = 3;
const NO_CHILD: u8 = 4;
const CHILD_NOT_READY: u8 = 5;
const NO_REPORT: u8 = 6;

fn main() -> ExitCode {
    let spec = match ProcSpec::parse(std::env::args_os().skip(1)) {
        Ok(spec) => spec,
        Err(e) => {
            eprintln!("fixture-sleeper: {e}");
            return ExitCode::from(BAD_ARGUMENTS);
        }
    };
    set_sigterm(spec.ignore_term);
    if spec.setsid && become_session_leader().is_err() {
        eprintln!("fixture-sleeper: setsid failed");
        return ExitCode::from(NO_SESSION);
    }
    let mut children = Vec::new();
    for index in 1..=spec.spawn {
        match spawn_child(&spec, index) {
            Ok(child) => children.push(child),
            Err(e) => {
                eprintln!("fixture-sleeper: cannot start child {index}: {e}");
                return give_up(children, NO_CHILD);
            }
        }
    }
    if let Some(path) = &spec.report_file {
        for (offset, child) in children.iter_mut().enumerate() {
            if !child_is_ready(&spec, offset + 1, child) {
                eprintln!("fixture-sleeper: child {} did not report", offset + 1);
                return give_up(children, CHILD_NOT_READY);
            }
        }
        if let Err(e) = write_report(&spec, path) {
            eprintln!("fixture-sleeper: cannot write the report: {e}");
            return give_up(children, NO_REPORT);
        }
    }
    thread::sleep(spec.lifetime());
    ExitCode::SUCCESS
}

fn set_sigterm(ignore: bool) {
    let handler = if ignore { libc::SIG_IGN } else { libc::SIG_DFL };
    // SAFETY: SIG_IGN and SIG_DFL are valid dispositions and no handler code is installed.
    unsafe { libc::signal(libc::SIGTERM, handler) };
}

fn become_session_leader() -> io::Result<()> {
    // SAFETY: setsid takes no arguments and only changes this process's session.
    if unsafe { libc::setsid() } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn session_id() -> io::Result<i32> {
    // SAFETY: getsid(0) reads the session of the calling process.
    let sid = unsafe { libc::getsid(0) };
    if sid < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(sid)
}

fn spawn_child(spec: &ProcSpec, index: usize) -> io::Result<Child> {
    Command::new(std::env::current_exe()?)
        .args(spec.child(index).args())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
}

fn child_is_ready(spec: &ProcSpec, index: usize, child: &mut Child) -> bool {
    let Some(path) = spec.child(index).report_file else {
        return true;
    };
    wait_until(
        || matches!(report::read(&path), Ok(Some(_))) || matches!(child.try_wait(), Ok(Some(_))),
        CHILD_READY,
    ) && matches!(report::read(&path), Ok(Some(_)))
}

fn write_report(spec: &ProcSpec, path: &std::path::Path) -> io::Result<()> {
    let report = Report {
        pid: std::process::id() as i32,
        ppid: parent_id() as i32,
        sid: session_id()?,
        env: spec
            .echo_env
            .as_ref()
            .and_then(std::env::var_os)
            .map(OsStringExt::into_vec),
    };
    report::write(path, &report)
}

fn give_up(children: Vec<Child>, code: u8) -> ExitCode {
    for mut child in children {
        let _ = child.kill();
        let _ = child.wait();
    }
    ExitCode::from(code)
}
