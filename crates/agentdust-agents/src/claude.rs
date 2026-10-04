use std::fmt;
use std::io::Read;

use agentdust_core::journal::Kind;
use serde::Deserialize;
use serde::de::{DeserializeSeed, Deserializer, Error as _, IgnoredAny, MapAccess, Visitor};
use thiserror::Error;

use crate::budget::{Budget, Metered};

pub const MAX_ID_LEN: usize = 256;
const ESCAPED_BYTE_LEN: usize = 6;
const FRAMING_LEN: usize = 64;
const FIELD_BUDGET: usize = MAX_ID_LEN * ESCAPED_BYTE_LEN + FRAMING_LEN;
const SHELL_TOOL: &str = "Bash";

#[derive(Debug, PartialEq, Eq)]
pub struct HookEvent {
    pub session_id: String,
    pub hook_event_name: String,
    pub tool_name: Option<String>,
    pub tool_use_id: Option<String>,
    pub agent_id: Option<String>,
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
    if strings.into_iter().flatten().any(|text| text.len() > MAX_ID_LEN) {
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

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "snake_case")]
enum Field {
    SessionId,
    HookEventName,
    ToolName,
    ToolUseId,
    AgentId,
    #[serde(other)]
    Other,
}

struct EventSeed<'a> {
    budget: &'a Budget,
}

impl EventSeed<'_> {
    fn value<'de, A: MapAccess<'de>, T: Deserialize<'de>>(&self, map: &mut A) -> Result<T, A::Error> {
        self.budget.arm(FIELD_BUDGET);
        let value = map.next_value();
        self.budget.disarm();
        value
    }
}

impl<'de> DeserializeSeed<'de> for EventSeed<'_> {
    type Value = HookEvent;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<HookEvent, D::Error> {
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
        loop {
            self.budget.arm(FIELD_BUDGET);
            let field = map.next_key::<Field>()?;
            self.budget.disarm();
            match field {
                None => break,
                Some(Field::SessionId) => once(&mut session_id, "session_id", self.value(&mut map)?)?,
                Some(Field::HookEventName) => {
                    once(&mut hook_event_name, "hook_event_name", self.value(&mut map)?)?;
                }
                Some(Field::ToolName) => once(&mut tool_name, "tool_name", self.value(&mut map)?)?,
                Some(Field::ToolUseId) => once(&mut tool_use_id, "tool_use_id", self.value(&mut map)?)?,
                Some(Field::AgentId) => once(&mut agent_id, "agent_id", self.value(&mut map)?)?,
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
        })
    }
}

fn once<T, E: serde::de::Error>(slot: &mut Option<T>, name: &'static str, value: T) -> Result<(), E> {
    match slot.replace(value) {
        None => Ok(()),
        Some(_) => Err(E::duplicate_field(name)),
    }
}
