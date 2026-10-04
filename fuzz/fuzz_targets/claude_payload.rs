#![no_main]

use agentdust_agents::claude::{self, MAX_CWD_LEN, MAX_ID_LEN};
use agentdust_fuzz::bounds::{PAYLOAD_PEAK_BYTES, stretch};
use agentdust_fuzz::meter::{self, Meter};
use libfuzzer_sys::fuzz_target;

#[global_allocator]
static ALLOCATOR: Meter = Meter;

const STRETCHED_ONE_IN: usize = 16;

fuzz_target!(|data: &[u8]| {
    let (parsed, peak) = meter::measure(|| claude::parse_event(data));
    assert!(
        peak <= PAYLOAD_PEAK_BYTES,
        "peak {peak} for {} input bytes",
        data.len()
    );
    if data.len().is_multiple_of(STRETCHED_ONE_IN)
        && let Some(long) = stretch(data)
    {
        let (_, peak) = meter::measure(|| claude::parse_event(&long[..]));
        assert!(
            peak <= PAYLOAD_PEAK_BYTES,
            "peak {peak} for a string stretched to {} input bytes",
            long.len()
        );
    }
    let Ok(event) = parsed else {
        return;
    };

    for text in [
        Some(&event.session_id),
        Some(&event.hook_event_name),
        event.tool_name.as_ref(),
        event.tool_use_id.as_ref(),
        event.agent_id.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        assert!(text.len() <= MAX_ID_LEN);
    }
    assert!(event.cwd.as_ref().is_none_or(|cwd| cwd.len() <= MAX_CWD_LEN));
    let _ = claude::journal_kind(&event);

    let again = serde_json::json!({
        "session_id": &event.session_id,
        "hook_event_name": &event.hook_event_name,
        "tool_name": &event.tool_name,
        "tool_use_id": &event.tool_use_id,
        "agent_id": &event.agent_id,
        "cwd": &event.cwd,
    });
    let reparsed = claude::parse_event(again.to_string().as_bytes()).expect("a parsed event parses again");
    assert_eq!(reparsed, event);
});
