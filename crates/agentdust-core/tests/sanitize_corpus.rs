use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use agentdust_core::inventory::parse_launchctl_list;
use agentdust_core::sanitize::{escape, redact_command, terminal_command, terminal_path};

fn unescape(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next().expect("a backslash is never last") {
            '\\' => out.push('\\'),
            'x' => {
                let hex: String = chars.by_ref().take(2).collect();
                out.push(char::from(u8::from_str_radix(&hex, 16).unwrap()));
            }
            'u' => {
                assert_eq!(chars.next(), Some('{'));
                let hex: String = chars.by_ref().take_while(|c| *c != '}').collect();
                out.push(char::from_u32(u32::from_str_radix(&hex, 16).unwrap()).unwrap());
            }
            other => panic!("unknown escape \\{other}"),
        }
    }
    out
}

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

#[test]
fn every_seed_is_sanitised_without_a_panic_and_without_a_control_character() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/sanitize");
    let (mut seen, mut escaped_seeds, mut redacted_seeds, mut listed, mut refused) = (0, 0, 0, 0, 0);
    for entry in fs::read_dir(&dir).unwrap() {
        let data = fs::read(entry.unwrap().path()).unwrap();
        let text = String::from_utf8_lossy(&data);
        let escaped = escape(&text);
        assert!(!forbidden(&escaped));
        assert_eq!(unescape(&escaped), text);
        if escaped != text {
            escaped_seeds += 1;
        }
        let args: Vec<&[u8]> = data.split(|byte| *byte == 0).collect();
        assert!(!forbidden(&terminal_command(&args)));
        let plain: Vec<String> = args
            .iter()
            .map(|arg| String::from_utf8_lossy(arg).into_owned())
            .collect();
        if redact_command(&args) != plain.join(" ") {
            redacted_seeds += 1;
        }
        assert!(!forbidden(&terminal_path(Path::new(
            std::ffi::OsStr::from_bytes(&data)
        ))));
        match parse_launchctl_list(&text) {
            Ok(_) => listed += 1,
            Err(_) => refused += 1,
        }
        seen += 1;
    }
    assert!(seen >= 9, "expected the seeds in {}", dir.display());
    assert!(escaped_seeds >= 3, "{escaped_seeds} seeds change under escaping");
    assert!(
        redacted_seeds >= 3,
        "{redacted_seeds} seeds change under redaction"
    );
    assert!(
        listed >= 1 && refused >= 1,
        "{listed} lists parse, {refused} do not"
    );
}
