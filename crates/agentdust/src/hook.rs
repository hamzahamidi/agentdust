use std::cell::OnceCell;
use std::error::Error;
use std::fs;
use std::io::{self, BufReader};
use std::os::unix::process::parent_id;
use std::panic;
use std::path::{Path, PathBuf};

use agentdust_agents::claude::{self, ENV_FILE_VAR, HookEvent};
use agentdust_core::darwin::DarwinProvider;
use agentdust_core::journal::{
    self, Agent, AgentIdentity, CwdKey, Kind, Record, SCHEMA_VERSION, SessionTagKey,
};
use agentdust_core::secret::Secret;
use agentdust_core::tag::{self, SessionTag};
use agentdust_core::{ancestry, clock, cwd, darwin, paths, secret};

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
    let dir = paths::data_dir()?;
    if let Some(parent) = dir.parent() {
        match fs::symlink_metadata(parent) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(io::Error::other("the parent of the data directory is not a directory").into());
            }
            Err(err) => return Err(err.into()),
        }
    }
    let secret = LazySecret::new(&dir);
    let handed_out = (kind == Kind::SessionStart)
        .then(|| session_tag(&secret))
        .flatten();
    let record = Record {
        v: SCHEMA_VERSION,
        kind,
        agent: Agent::Claude,
        session_id: event.session_id.clone(),
        subagent_id: event.agent_id.clone(),
        agent_identity: agent_identity(),
        tool_use_id: event.tool_use_id.clone(),
        wall_ts: clock::wall_ms(),
        mono_ts: clock::monotonic_ns(),
        boot: darwin::boot_session_uuid()?,
        session_tag_key: handed_out.as_ref().map(|(_, key)| key.clone()),
        cwd_key: event.cwd.as_deref().and_then(|cwd| cwd_key(&secret, cwd)),
        exe_base: None,
    };
    journal::append(&dir, &record)?;
    if let Some((tag, _)) = handed_out {
        hand_over(&tag);
    }
    Ok(())
}

struct LazySecret<'a> {
    dir: &'a Path,
    loaded: OnceCell<Option<Secret>>,
}

impl<'a> LazySecret<'a> {
    fn new(dir: &'a Path) -> Self {
        Self {
            dir,
            loaded: OnceCell::new(),
        }
    }

    fn get(&self) -> Option<&Secret> {
        self.loaded
            .get_or_init(|| secret::load_or_create(self.dir).ok())
            .as_ref()
    }
}

fn cwd_key(secret: &LazySecret, working_dir: &str) -> Option<CwdKey> {
    cwd::normalize(working_dir)?;
    cwd::cwd_key(secret.get()?, working_dir)
}

fn session_tag(secret: &LazySecret) -> Option<(SessionTag, SessionTagKey)> {
    let tag = SessionTag::generate().ok()?;
    let key = tag.key(secret.get()?);
    Some((tag, key))
}

fn hand_over(tag: &SessionTag) {
    let Some(path) = std::env::var_os(ENV_FILE_VAR).filter(|path| !path.is_empty()) else {
        return;
    };
    let _ = tag::append_export(&PathBuf::from(path), tag);
}

fn agent_identity() -> Option<AgentIdentity> {
    let provider = DarwinProvider::new().ok()?;
    let found = ancestry::find_agent(&provider, parent_id() as i32)?;
    AgentIdentity::from_process(&found.identity).ok()
}

fn drain_stdin() {
    let _ = io::copy(&mut io::stdin().lock(), &mut io::sink());
}
