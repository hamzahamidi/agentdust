#![no_main]

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use agentdust_core::inventory::parse_launchctl_list;
use agentdust_core::sanitize::{escape, redact_command, terminal_command, terminal_path};
use libfuzzer_sys::fuzz_target;

fn forbidden(text: &str) -> bool {
    text.chars().any(|c| {
        c.is_control()
            || matches!(
                c,
                '\u{061c}'
                    | '\u{200b}'..='\u{200f}'
                    | '\u{2028}'..='\u{202e}'
                    | '\u{2060}'
                    | '\u{2066}'..='\u{2069}'
                    | '\u{feff}'
            )
    })
}

fn unescape(text: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            '\\' => out.push('\\'),
            'x' => {
                let hex: String = chars.by_ref().take(2).collect();
                out.push(char::from(u8::from_str_radix(&hex, 16).ok()?));
            }
            'u' => {
                if chars.next()? != '{' {
                    return None;
                }
                let hex: String = chars.by_ref().take_while(|c| *c != '}').collect();
                out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
            }
            _ => return None,
        }
    }
    Some(out)
}

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let escaped = escape(&text);
    assert!(!forbidden(&escaped));
    assert_eq!(unescape(&escaped).as_deref(), Some(text.as_ref()));
    let args: Vec<&[u8]> = data.split(|byte| *byte == 0).collect();
    let _ = redact_command(&args);
    assert!(!forbidden(&terminal_command(&args)));
    assert!(!forbidden(&terminal_path(Path::new(OsStr::from_bytes(data)))));
    let _ = parse_launchctl_list(&text);
});
