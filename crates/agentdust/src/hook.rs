use std::error::Error;
use std::io::{self, BufReader};
use std::panic;

use agentdust_agents::claude::{self, HookEvent};
use agentdust_core::journal::{self, Agent, Record, SCHEMA_VERSION};
use agentdust_core::{clock, darwin, paths};

pub fn run() {
    panic::set_hook(Box::new(|_| {}));
    let _ = panic::catch_unwind(dispatch);
    let _ = panic::catch_unwind(drain_stdin);
}

fn dispatch() {
    let args: Vec<String> = std::env::args().skip(2).collect();
    if args == ["claude"] {
        run_claude();
    }
}

fn run_claude() {
    let mut input = BufReader::new(io::stdin().lock());
    let event = claude::parse_event(&mut input);
    let _ = io::copy(&mut input, &mut io::sink());
    if let Ok(event) = event {
        let _ = record(&event);
    }
}

fn record(event: &HookEvent) -> Result<(), Box<dyn Error>> {
    let Some(kind) = claude::journal_kind(event) else {
        return Ok(());
    };
    let record = Record {
        v: SCHEMA_VERSION,
        kind,
        agent: Agent::Claude,
        session_id: event.session_id.clone(),
        subagent_id: event.agent_id.clone(),
        tool_use_id: event.tool_use_id.clone(),
        wall_ts: clock::wall_ms(),
        mono_ts: clock::monotonic_ns(),
        boot: darwin::boot_session_uuid()?,
        cwd_key: None,
        exe_base: None,
    };
    journal::append(&paths::data_dir()?, &record)?;
    Ok(())
}

fn drain_stdin() {
    let _ = io::copy(&mut io::stdin().lock(), &mut io::sink());
}
