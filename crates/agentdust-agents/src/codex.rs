use agentdust_core::tag;

pub use crate::claude::{EventError, HookEvent, journal_kind, parse_event};

pub fn valid_session(event: &HookEvent) -> bool {
    tag::valid_codex_session(event.session_id.as_bytes())
}
