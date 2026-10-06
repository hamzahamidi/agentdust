use std::fmt;
use std::io::Read;

use agentdust_core::journal::Kind;
use serde::Deserialize;
use serde::de::{DeserializeSeed, Deserializer, Error as _, IgnoredAny, MapAccess, Visitor};
use thiserror::Error;

use crate::budget::{Budget, Metered};

pub const ENV_FILE_VAR: &str = "CLAUDE_ENV_FILE";
pub const MAX_ID_LEN: usize = 256;
pub const MAX_CWD_LEN: usize = 4096;
const ESCAPED_BYTE_LEN: usize = 6;
const FRAMING_LEN: usize = 64;
const NAME_BUDGET: usize = budget_for(MAX_ID_LEN);
const SHELL_TOOL: &str = "Bash";

#[derive(Debug, PartialEq, Eq)]
pub struct HookEvent {
    pub session_id: String,
    pub hook_event_name: String,
    pub tool_name: Option<String>,
    pub tool_use_id: Option<String>,
    pub agent_id: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Error)]
pub enum EventError {
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("a field name or identifier is longer than {MAX_ID_LEN} bytes")]
    FieldTooLong,
}

pub fn parse_event(reader: impl Read) -> Result<HookEvent, EventError> {
    let budget = Budget::default();
    let mut deserializer = serde_json::Deserializer::from_reader(Metered::new(reader, &budget));
    let event = match (EventSeed { budget: &budget }).deserialize(&mut deserializer) {
        Ok(event) => event,
        Err(_) if budget.tripped() => return Err(EventError::FieldTooLong),
        Err(err) => return Err(err.into()),
    };
    let strings = [
        Some(&event.session_id),
        Some(&event.hook_event_name),
        event.tool_name.as_ref(),
        event.tool_use_id.as_ref(),
        event.agent_id.as_ref(),
    ];
    let cwd_too_long = event.cwd.as_ref().is_some_and(|cwd| cwd.len() > MAX_CWD_LEN);
    if cwd_too_long || strings.into_iter().flatten().any(|text| text.len() > MAX_ID_LEN) {
        return Err(EventError::FieldTooLong);
    }
    Ok(event)
}

pub fn journal_kind(event: &HookEvent) -> Option<Kind> {
    let shell = event.tool_name.as_deref() == Some(SHELL_TOOL);
    match event.hook_event_name.as_str() {
        "SessionStart" => Some(Kind::SessionStart),
        "SessionEnd" => Some(Kind::SessionEnd),
        "SubagentStart" => Some(Kind::SubagentStart),
        "SubagentStop" => Some(Kind::SubagentStop),
        "PreToolUse" if shell => Some(Kind::ShellStart),
        "PostToolUse" if shell => Some(Kind::ShellEnd),
        _ => None,
    }
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "snake_case")]
enum Field {
    SessionId,
    HookEventName,
    ToolName,
    ToolUseId,
    AgentId,
    Cwd,
    #[serde(other)]
    Other,
}

struct EventSeed<'a> {
    budget: &'a Budget,
}

impl EventSeed<'_> {
    fn value<'de, A: MapAccess<'de>, T: Deserialize<'de>>(
        &self,
        map: &mut A,
        max_len: usize,
    ) -> Result<T, A::Error> {
        self.budget.arm(budget_for(max_len));
        let value = map.next_value();
        self.budget.disarm();
        value
    }
}

impl<'de> DeserializeSeed<'de> for EventSeed<'_> {
    type Value = HookEvent;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<HookEvent, D::Error> {
        self.budget.arm(NAME_BUDGET);
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for EventSeed<'_> {
    type Value = HookEvent;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a hook event object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<HookEvent, A::Error> {
        let mut session_id = None;
        let mut hook_event_name = None;
        let mut tool_name = None;
        let mut tool_use_id = None;
        let mut agent_id = None;
        let mut cwd = None;
        loop {
            self.budget.arm(NAME_BUDGET);
            let field = map.next_key::<Field>()?;
            self.budget.disarm();
            match field {
                None => break,
                Some(Field::SessionId) => {
                    once(&mut session_id, "session_id", self.value(&mut map, MAX_ID_LEN)?)?;
                }
                Some(Field::HookEventName) => {
                    once(
                        &mut hook_event_name,
                        "hook_event_name",
                        self.value(&mut map, MAX_ID_LEN)?,
                    )?;
                }
                Some(Field::ToolName) => {
                    once(&mut tool_name, "tool_name", self.value(&mut map, MAX_ID_LEN)?)?;
                }
                Some(Field::ToolUseId) => {
                    once(&mut tool_use_id, "tool_use_id", self.value(&mut map, MAX_ID_LEN)?)?;
                }
                Some(Field::AgentId) => once(&mut agent_id, "agent_id", self.value(&mut map, MAX_ID_LEN)?)?,
                Some(Field::Cwd) => once(&mut cwd, "cwd", self.value(&mut map, MAX_CWD_LEN)?)?,
                Some(Field::Other) => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(HookEvent {
            session_id: session_id.ok_or_else(|| A::Error::missing_field("session_id"))?,
            hook_event_name: hook_event_name.ok_or_else(|| A::Error::missing_field("hook_event_name"))?,
            tool_name: tool_name.flatten(),
            tool_use_id: tool_use_id.flatten(),
            agent_id: agent_id.flatten(),
            cwd: cwd.flatten(),
        })
    }
}

const fn budget_for(max_len: usize) -> usize {
    max_len * ESCAPED_BYTE_LEN + FRAMING_LEN
}

fn once<T, E: serde::de::Error>(slot: &mut Option<T>, name: &'static str, value: T) -> Result<(), E> {
    match slot.replace(value) {
        None => Ok(()),
        Some(_) => Err(E::duplicate_field(name)),
    }
}
