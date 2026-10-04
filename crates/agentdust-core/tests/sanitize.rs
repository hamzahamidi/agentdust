use std::path::Path;

use agentdust_core::sanitize::{
    COMMAND_MAX, PATH_MAX, escape, redact_command, terminal_command, terminal_path, truncate_head,
    truncate_tail,
};
use proptest::collection::vec;
use proptest::prelude::*;

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

fn has_forbidden(text: &str) -> bool {
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
fn plain_text_is_unchanged() {
    for text in [
        "",
        "node server.js",
        "caf\u{e9} \u{65e5}\u{672c}\u{8a9e} \u{1f600}",
        "a/b c-d_e.f",
    ] {
        assert_eq!(escape(text), text);
    }
}

#[test]
fn c0_controls_become_hex_escapes() {
    assert_eq!(escape("a\nb"), "a\\x0ab");
    assert_eq!(escape("a\tb"), "a\\x09b");
    assert_eq!(escape("a\rb"), "a\\x0db");
    assert_eq!(escape("\0"), "\\x00");
    assert_eq!(escape("\u{7f}"), "\\x7f");
}

#[test]
fn the_escape_character_is_escaped_so_a_terminal_never_sees_it() {
    assert_eq!(escape("\u{1b}[31mred\u{1b}[0m"), "\\x1b[31mred\\x1b[0m");
    assert_eq!(escape("\u{1b}]0;title\u{7}"), "\\x1b]0;title\\x07");
}

#[test]
fn c1_controls_become_hex_escapes() {
    assert_eq!(escape("\u{9b}31m"), "\\x9b31m");
    assert_eq!(escape("\u{80}\u{85}\u{9f}"), "\\x80\\x85\\x9f");
}

#[test]
fn bidirectional_controls_become_unicode_escapes() {
    assert_eq!(escape("a\u{202e}b"), "a\\u{202e}b");
    assert_eq!(
        escape("\u{2066}\u{2067}\u{2068}\u{2069}"),
        "\\u{2066}\\u{2067}\\u{2068}\\u{2069}"
    );
    assert_eq!(
        escape("\u{202a}\u{202b}\u{202c}\u{202d}"),
        "\\u{202a}\\u{202b}\\u{202c}\\u{202d}"
    );
    assert_eq!(escape("\u{200e}\u{200f}\u{61c}"), "\\u{200e}\\u{200f}\\u{61c}");
}

#[test]
fn line_separators_and_invisible_marks_are_escaped() {
    assert_eq!(escape("\u{2028}\u{2029}"), "\\u{2028}\\u{2029}");
    assert_eq!(
        escape("\u{200b}\u{200c}\u{200d}\u{2060}\u{feff}"),
        "\\u{200b}\\u{200c}\\u{200d}\\u{2060}\\u{feff}"
    );
}

#[test]
fn a_backslash_is_doubled_so_text_cannot_forge_an_escape() {
    assert_eq!(escape("\\x1b"), "\\\\x1b");
    assert_ne!(escape("\\x1b"), escape("\u{1b}"));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn escaping_leaves_no_control_and_can_be_undone(
        chars in vec(prop_oneof![
            any::<char>(),
            Just('\u{1b}'),
            Just('\\'),
            Just('\u{202e}'),
            Just('\u{9b}'),
            Just('\n'),
            Just('\u{2028}'),
        ], 0..60)
    ) {
        let text: String = chars.into_iter().collect();
        let escaped = escape(&text);
        prop_assert!(!has_forbidden(&escaped));
        prop_assert_eq!(unescape(&escaped), text);
    }
}

#[test]
fn head_truncation_keeps_short_text_and_marks_the_cut() {
    assert_eq!(truncate_head("abc", 10), "abc");
    assert_eq!(truncate_head("abcdefghij", 10), "abcdefghij");
    assert_eq!(truncate_head("abcdefghijk", 10), "abcdefg...");
    assert_eq!(truncate_head("abcdefghijk", 10).chars().count(), 10);
}

#[test]
fn tail_truncation_keeps_the_end() {
    assert_eq!(truncate_tail("abc", 10), "abc");
    assert_eq!(truncate_tail("abcdefghij", 10), "abcdefghij");
    assert_eq!(truncate_tail("abcdefghijk", 10), "...efghijk");
    assert_eq!(truncate_tail("abcdefghijk", 10).chars().count(), 10);
}

#[test]
fn truncation_counts_characters_and_never_splits_one() {
    let text = "\u{65e5}".repeat(30);
    let cut = truncate_head(&text, 10);
    assert_eq!(cut.chars().count(), 10);
    assert!(cut.starts_with("\u{65e5}\u{65e5}\u{65e5}"));
    let cut = truncate_tail(&text, 10);
    assert_eq!(cut.chars().count(), 10);
    assert!(cut.ends_with("\u{65e5}\u{65e5}\u{65e5}"));
}

#[test]
fn a_limit_below_the_marker_keeps_the_limit() {
    assert_eq!(truncate_head("abcdef", 2).chars().count(), 2);
    assert_eq!(truncate_tail("abcdef", 2).chars().count(), 2);
    assert_eq!(truncate_head("abcdef", 0), "");
}

#[test]
fn the_limits_are_the_spec_values() {
    assert_eq!(COMMAND_MAX, 120);
    assert_eq!(PATH_MAX, 80);
}

fn redact(args: &[&str]) -> String {
    redact_command(args.iter().map(|arg| arg.as_bytes()))
}

#[test]
fn a_command_without_secrets_is_joined_with_spaces() {
    assert_eq!(
        redact(&["node", "server.js", "--port", "3000"]),
        "node server.js --port 3000"
    );
}

#[test]
fn assignments_of_sensitive_names_lose_their_value() {
    assert_eq!(redact(&["env", "API_KEY=abc123", "run"]), "env API_KEY=*** run");
    assert_eq!(redact(&["x", "github_token=zzz"]), "x github_token=***");
    assert_eq!(redact(&["x", "DB_PASSWORD=p@ss"]), "x DB_PASSWORD=***");
    assert_eq!(
        redact(&["x", "AWS_SECRET_ACCESS_KEY=s"]),
        "x AWS_SECRET_ACCESS_KEY=***"
    );
    assert_eq!(redact(&["x", "PORT=3000"]), "x PORT=3000");
}

#[test]
fn flags_with_sensitive_names_lose_their_value() {
    assert_eq!(redact(&["cli", "--token", "abc"]), "cli --token ***");
    assert_eq!(redact(&["cli", "--token=abc"]), "cli --token=***");
    assert_eq!(
        redact(&["cli", "--password", "hunter2", "--user", "bob"]),
        "cli --password *** --user bob"
    );
    assert_eq!(redact(&["cli", "--api-key=k"]), "cli --api-key=***");
    assert_eq!(redact(&["cli", "--verbose", "abc"]), "cli --verbose abc");
}

#[test]
fn authorization_headers_lose_their_credential() {
    assert_eq!(
        redact(&[
            "curl",
            "-H",
            "Authorization: Bearer abcdef",
            "https://example.test"
        ]),
        "curl -H Authorization: *** https://example.test"
    );
    assert_eq!(
        redact(&["curl", "-H", "authorization:token abc"]),
        "curl -H authorization:***"
    );
    assert_eq!(redact(&["x", "Bearer abc"]), "x Bearer ***");
}

#[test]
fn url_credentials_are_removed() {
    assert_eq!(
        redact(&["git", "clone", "https://bob:hunter2@example.test/repo.git"]),
        "git clone https://***@example.test/repo.git"
    );
    assert_eq!(
        redact(&["x", "postgres://user@host/db"]),
        "x postgres://***@host/db"
    );
    assert_eq!(
        redact(&["x", "https://example.test/a@b"]),
        "x https://example.test/a@b"
    );
}

#[test]
fn well_known_token_shapes_are_removed_wherever_they_appear() {
    for secret in [
        "sk-abcdefghijklmnopqrstuvwx",
        "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
        "github_pat_11ABCDEFG0123456789_abcdef",
        "xoxb-1234-5678-abcdefgh",
        "AKIAIOSFODNN7EXAMPLE",
        "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghijk",
        "glpat-abcdefghij0123456789",
        "npm_abcdefghijklmnopqrstuvwxyz0123456789",
    ] {
        let found = redact(&["tool", &format!("--flag={secret}"), secret]);
        assert!(!found.contains(secret), "{found}");
        assert!(found.contains("***"), "{found}");
    }
}

#[test]
fn long_mixed_tokens_are_removed_and_long_words_and_paths_are_kept() {
    let hex = "0123456789abcdef0123456789abcdef";
    assert!(!redact(&["x", hex]).contains(hex));
    let word = "agentdust-testkit-fixture-sleeper-long-name";
    assert_eq!(redact(&["x", word]), format!("x {word}"));
    let path = "/Users/dev/projects/service-with-a-long-name/node_modules/.bin/vite";
    assert_eq!(redact(&["x", path]), format!("x {path}"));
}

#[test]
fn non_utf8_arguments_are_shown_with_replacement_characters() {
    let found = redact_command([b"a".as_slice(), b"\xff\xfe".as_slice()]);
    assert_eq!(found, "a \u{fffd}\u{fffd}");
}

#[test]
fn no_arguments_give_an_empty_command() {
    assert_eq!(redact_command(Vec::<&[u8]>::new()), "");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn a_sensitive_value_never_survives(value in "[A-Za-z0-9]{6,20}", name in "[A-Z]{3,8}") {
        let value = format!("v{value}");
        let tail = format!("--token={value}");
        let assignment = format!("{name}_SECRET={value}");
        let lines = [
            redact_command(["x".as_bytes(), tail.as_bytes()]),
            redact_command(["x".as_bytes(), b"--password".as_slice(), value.as_bytes()]),
            redact_command(["x".as_bytes(), assignment.as_bytes()]),
        ];
        for line in lines {
            prop_assert!(!line.contains(&value), "{line}");
        }
    }
}

#[test]
fn the_terminal_command_is_redacted_then_cut_then_escaped() {
    let long = "x".repeat(400);
    let shown = terminal_command(["tool".as_bytes(), b"--token=abc".as_slice(), long.as_bytes()]);
    assert!(!shown.contains("abc"));
    assert_eq!(shown.chars().count(), COMMAND_MAX);
    assert!(shown.ends_with("..."));
}

#[test]
fn the_terminal_command_escapes_controls_after_the_cut() {
    let shown = terminal_command(["echo".as_bytes(), b"\x1b[31m\n\xe2\x80\xae".as_slice()]);
    assert_eq!(shown, "echo \\x1b[31m\\x0a\\u{202e}");
    assert!(!has_forbidden(&shown));
}

#[test]
fn the_terminal_path_keeps_the_tail_and_escapes() {
    let long = format!("/Users/dev/{}/project", "d".repeat(200));
    let shown = terminal_path(Path::new(&long));
    assert_eq!(shown.chars().count(), PATH_MAX);
    assert!(shown.starts_with("..."));
    assert!(shown.ends_with("/project"));
    assert_eq!(terminal_path(Path::new("/tmp/a\nb")), "/tmp/a\\x0ab");
    assert_eq!(terminal_path(Path::new("/tmp/short")), "/tmp/short");
}
