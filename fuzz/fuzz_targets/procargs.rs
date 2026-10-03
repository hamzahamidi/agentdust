#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(parsed) = agentdust_core::procargs::parse(data) {
        let _ = agentdust_core::procargs::env_value(&parsed, "AGENTDUST_SESSION");
    }
});
