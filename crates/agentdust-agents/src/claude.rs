use std::io::Read;

use agentdust_core::journal::Kind;
use serde::Deserialize;
use thiserror::Error;

pub const MAX_ID_LEN: usize = 256;
const SHELL_TOOL: &str = "Bash";

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct HookEvent {
    pub session_id: String,
    pub hook_event_name: String,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Error)]
pub enum EventError {
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("an identifier is longer than {MAX_ID_LEN} bytes")]
    FieldTooLong,
}

pub fn parse_event(reader: impl Read) -> Result<HookEvent, EventError> {
    let event = HookEvent::deserialize(&mut serde_json::Deserializer::from_reader(reader))?;
    let ids = [
        Some(&event.session_id),
        event.tool_use_id.as_ref(),
        event.agent_id.as_ref(),
    ];
    if ids.into_iter().flatten().any(|id| id.len() > MAX_ID_LEN) {
        return Err(EventError::FieldTooLong);
    }
    Ok(event)
}

pub fn journal_kind(event: &HookEvent) -> Option<Kind> {
    let shell = event.tool_name.as_deref() == Some(SHELL_TOOL);
    match event.hook_event_name.as_str() {
        "SessionStart" => Some(Kind::SessionStart),
        "SessionEnd" => Some(Kind::SessionEnd),
        "PreToolUse" if shell => Some(Kind::ShellStart),
        "PostToolUse" if shell => Some(Kind::ShellEnd),
        _ => None,
    }
}
