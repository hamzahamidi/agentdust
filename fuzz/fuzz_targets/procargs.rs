#![no_main]

use agentdust_core::procargs;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let script = procargs::script_argument(data);
    match procargs::parse(data) {
        Ok(parsed) => {
            let _ = procargs::env_value(&parsed, "AGENTDUST_SESSION");
            let found = script.expect("arguments that parse have a script argument result");
            if let Some(argument) = found {
                assert!(parsed.args.iter().skip(1).any(|arg| *arg == argument));
            }
        }
        Err(error) => assert_eq!(script, Err(error)),
    }
});
