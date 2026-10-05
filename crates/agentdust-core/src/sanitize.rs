use std::fmt::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

pub const COMMAND_MAX: usize = 120;
pub const PATH_MAX: usize = 80;

const MARK: &str = "...";
const MASK: &str = "***";
const SENSITIVE_WORDS: [&str; 11] = [
    "token",
    "secret",
    "password",
    "passwd",
    "pwd",
    "pass",
    "key",
    "auth",
    "credential",
    "cookie",
    "bearer",
];
const TOKEN_PREFIXES: [&str; 17] = [
    "sk-",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "xoxb-",
    "xoxa-",
    "xoxp-",
    "xoxr-",
    "xoxs-",
    "AKIA",
    "ASIA",
    "eyJ",
    "glpat-",
    "npm_",
];
const PREFIXED_TAIL_MIN: usize = 8;
const LONG_TOKEN_MIN: usize = 32;
const NAME_MAX: usize = 64;

pub(crate) fn is_control_or_bidi(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
        )
}

fn needs_escape(c: char) -> bool {
    is_control_or_bidi(c)
        || matches!(
            c,
            '\u{2028}' | '\u{2029}' | '\u{200b}'..='\u{200d}' | '\u{2060}' | '\u{feff}'
        )
}

pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\\' {
            out.push_str("\\\\");
        } else if needs_escape(c) {
            let code = u32::from(c);
            let _ = if code < 0x100 {
                write!(out, "\\x{code:02x}")
            } else {
                write!(out, "\\u{{{code:x}}}")
            };
        } else {
            out.push(c);
        }
    }
    out
}

pub fn truncate_head(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    if max <= MARK.len() {
        return text.chars().take(max).collect();
    }
    let mut out: String = text.chars().take(max - MARK.len()).collect();
    out.push_str(MARK);
    out
}

pub fn truncate_tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    if max <= MARK.len() {
        return text.chars().skip(count - max).collect();
    }
    let mut out = String::from(MARK);
    out.extend(text.chars().skip(count - (max - MARK.len())));
    out
}

pub fn redact_command<I, A>(args: I) -> String
where
    I: IntoIterator<Item = A>,
    A: AsRef<[u8]>,
{
    let mut shown: Vec<String> = Vec::new();
    let mut mask_next = false;
    for arg in args {
        let text = String::from_utf8_lossy(arg.as_ref());
        if mask_next && !text.starts_with('-') {
            mask_next = false;
            shown.push(MASK.to_owned());
            continue;
        }
        mask_next = text.starts_with('-') && !text.contains('=') && sensitive(&text);
        shown.push(redact_arg(&text));
    }
    shown.join(" ")
}

pub fn terminal_command<I, A>(args: I) -> String
where
    I: IntoIterator<Item = A>,
    A: AsRef<[u8]>,
{
    escape(&truncate_head(&redact_command(args), COMMAND_MAX))
}

pub fn terminal_path(path: &Path) -> String {
    let text = String::from_utf8_lossy(path.as_os_str().as_bytes());
    escape(&truncate_tail(&text, PATH_MAX))
}

fn sensitive(name: &str) -> bool {
    let lower = name.trim_start_matches('-').to_ascii_lowercase();
    SENSITIVE_WORDS.iter().any(|word| lower.contains(word))
}

fn is_name(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= NAME_MAX
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

fn redact_arg(text: &str) -> String {
    if let Some((name, _)) = text.split_once('=')
        && is_name(name)
        && sensitive(name)
    {
        return format!("{name}={MASK}");
    }
    let lower = text.to_ascii_lowercase();
    if let Some(at) = lower.find("authorization:") {
        let end = at + "authorization:".len();
        let space = if text[end..].starts_with(' ') { " " } else { "" };
        return format!("{}{space}{MASK}", &text[..end]);
    }
    if let Some(at) = lower.find("bearer ") {
        let end = at + "bearer ".len();
        return format!("{}{MASK}", &text[..end]);
    }
    mask_tokens(&mask_userinfo(text))
}

fn mask_userinfo(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let (head, tail) = rest.split_at(at + 3);
        out.push_str(head);
        let end = tail.find(['/', '?', '#']).unwrap_or(tail.len());
        let (authority, remainder) = tail.split_at(end);
        match authority.rfind('@') {
            Some(at) => {
                out.push_str(MASK);
                out.push_str(&authority[at..]);
            }
            None => out.push_str(authority),
        }
        rest = remainder;
    }
    out.push_str(rest);
    out
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')
}

fn mask_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut start: Option<usize> = None;
    for (at, c) in text.char_indices() {
        if is_token_char(c) {
            start.get_or_insert(at);
            continue;
        }
        if let Some(from) = start.take() {
            push_run(&mut out, &text[from..at]);
        }
        out.push(c);
    }
    if let Some(from) = start {
        push_run(&mut out, &text[from..]);
    }
    out
}

fn push_run(out: &mut String, run: &str) {
    out.push_str(if secret_shaped(run) { MASK } else { run });
}

fn secret_shaped(run: &str) -> bool {
    let prefixed = TOKEN_PREFIXES
        .iter()
        .any(|prefix| run.starts_with(prefix) && run.len() >= prefix.len() + PREFIXED_TAIL_MIN);
    let long = run.len() >= LONG_TOKEN_MIN
        && run.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && run.bytes().any(|byte| byte.is_ascii_digit())
        && run.bytes().any(|byte| byte.is_ascii_alphabetic());
    prefixed || long
}
