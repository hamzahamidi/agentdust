use std::fs::OpenOptions;
use std::io::{self, BufRead, BufReader, IsTerminal, Write};
use std::os::fd::AsRawFd;
use std::process::ExitCode;
use std::sync::Arc;

use agentdust_core::ancestry::find_agent;
use agentdust_core::apply::exec::{Deps, Settings};
use agentdust_core::apply::server::{Call, Response, Server};
use agentdust_core::apply::signal::KillSignaller;
use agentdust_core::apply::timer::SystemTimer;
use agentdust_core::darwin::DarwinProvider;
use agentdust_core::live::LiveSurveyor;
use agentdust_core::paths;

pub fn run() -> ExitCode {
    match execute() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("agentdust apply: {error}");
            ExitCode::FAILURE
        }
    }
}

fn execute() -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "requires stdin and stdout attached to a terminal",
        ));
    }
    let tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
    let mut terminal = BufReader::new(tty);
    let foreground = unsafe { libc::tcgetpgrp(terminal.get_ref().as_raw_fd()) };
    if foreground < 0 || foreground != unsafe { libc::getpgrp() } {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "requires the foreground process group of its terminal",
        ));
    }
    let data_dir = paths::data_dir()?;
    let provider = DarwinProvider::new()?;
    if find_agent(&provider, std::process::id() as i32).is_some() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "cannot run under a known agent process",
        ));
    }
    let surveyor = Arc::new(LiveSurveyor::new(&data_dir));
    let server = Server::new(
        Deps {
            data_dir,
            surveyor: surveyor.clone(),
            provider: Box::new(provider),
            signaller: Box::new(KillSignaller),
            timer: Arc::new(SystemTimer::new()),
        },
        Settings::default(),
    );
    let plan = server.plan().map_err(io::Error::other)?;
    if plan.items.is_empty() {
        writeln!(terminal.get_mut(), "No eligible processes were found.")?;
        return Ok(());
    }
    let call = Call {
        plan_id: plan.plan_id,
        item_ids: plan
            .items
            .iter()
            .take(10)
            .map(|item| item.item_id.clone())
            .collect(),
    };
    if plan.items.len() > 10 {
        writeln!(
            terminal.get_mut(),
            "This call covers 10 of {} eligible processes. Run apply again for another fresh plan.",
            plan.items.len()
        )?;
    }
    let mut ask = |challenge: &agentdust_core::apply::server::Challenge| {
        writeln!(terminal.get_mut(), "{}", challenge.message).ok();
        write!(terminal.get_mut(), "Approval code: ").ok();
        terminal.get_mut().flush().ok();
        let mut answer = String::new();
        match terminal.read_line(&mut answer) {
            Ok(0) => Response::Cancel,
            Ok(_) => Response::Accept(Some(answer.trim_end_matches(['\r', '\n']).to_owned())),
            Err(_) => Response::Cancel,
        }
    };
    let report = server.run(&call, &mut ask).map_err(io::Error::other)?;
    serde_json::to_writer_pretty(terminal.get_mut(), &report)?;
    writeln!(terminal.get_mut())?;
    Ok(())
}
