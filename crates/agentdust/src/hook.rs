use std::error::Error;
use std::io::{self, BufReader};

use agentdust_agents::claude::{self, HookEvent};
use agentdust_core::journal::{self, Agent, Record, SCHEMA_VERSION};
use agentdust_core::{clock, darwin, paths};

pub fn run_claude() {
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
        wall_ts_ms: clock::wall_ms(),
        mono_ns: clock::monotonic_ns(),
        boot: darwin::boot_session_uuid()?,
    };
    journal::append(&paths::data_dir()?, &record)?;
    Ok(())
}

pub fn drain_stdin() {
    let _ = io::copy(&mut io::stdin().lock(), &mut io::sink());
}
