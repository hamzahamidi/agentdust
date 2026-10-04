use std::io::Read;
use std::path::Path;

use crate::safe_open::{self, Access, SafeOpenError};

pub const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplySwitch {
    Enabled,
    Disabled,
    Unreadable(String),
}

impl ApplySwitch {
    pub fn allows_apply(&self) -> bool {
        matches!(self, Self::Enabled)
    }
}

pub fn apply_switch(dir: &Path) -> ApplySwitch {
    let mut file = match safe_open::open_file(&dir.join(CONFIG_FILE), Access::Read) {
        Ok(file) => file,
        Err(SafeOpenError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
            return ApplySwitch::Enabled;
        }
        Err(err) => return ApplySwitch::Unreadable(err.to_string()),
    };
    let mut bytes = Vec::new();
    if let Err(err) = file.read_to_end(&mut bytes) {
        return ApplySwitch::Unreadable(err.to_string());
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => return ApplySwitch::Unreadable("the file is not valid UTF-8".to_owned()),
    };
    match parse_apply(&text) {
        Ok(Some(false)) => ApplySwitch::Disabled,
        Ok(_) => ApplySwitch::Enabled,
        Err(reason) => ApplySwitch::Unreadable(reason),
    }
}

pub fn parse_apply(text: &str) -> Result<Option<bool>, String> {
    let mut apply = None;
    let mut in_table = false;
    for (index, raw) in text.lines().enumerate() {
        let number = index + 1;
        let line = raw.trim();
        if line.chars().any(|ch| ch.is_control() && ch != '\t') {
            return Err(format!("line {number}: control character"));
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            check_table_header(line).map_err(|reason| format!("line {number}: {reason}"))?;
            in_table = true;
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {number}: expected key = value"))?;
        let key = key.trim();
        if key.is_empty()
            || !key
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        {
            return Err(format!("line {number}: unsupported key"));
        }
        let scalar = parse_scalar(value.trim()).map_err(|reason| format!("line {number}: {reason}"))?;
        if key == "apply" && !in_table {
            let Scalar::Bool(setting) = scalar else {
                return Err(format!("line {number}: apply must be true or false"));
            };
            if apply.replace(setting).is_some() {
                return Err(format!("line {number}: apply is set twice"));
            }
        }
    }
    Ok(apply)
}

enum Scalar {
    Bool(bool),
    Other,
}

fn check_table_header(line: &str) -> Result<(), &'static str> {
    let rest = line.trim_start_matches('[');
    let opened = line.len() - rest.len();
    if opened > 2 {
        return Err("unsupported table header");
    }
    let Some((name, tail)) = rest.split_once(']') else {
        return Err("unterminated table header");
    };
    let tail = tail.strip_prefix(']').filter(|_| opened == 2).unwrap_or(tail);
    let name_ok = !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'));
    if !name_ok || !is_blank_or_comment(tail) {
        return Err("unsupported table header");
    }
    Ok(())
}

fn parse_scalar(value: &str) -> Result<Scalar, &'static str> {
    for (word, setting) in [("true", true), ("false", false)] {
        if let Some(tail) = value.strip_prefix(word) {
            return if is_blank_or_comment(tail) {
                Ok(Scalar::Bool(setting))
            } else {
                Err("unsupported value")
            };
        }
    }
    if let Some(body) = value.strip_prefix('"') {
        if body.starts_with("\"\"") {
            return Err("multi-line strings are not supported");
        }
        return basic_string_tail(body).map(|_| Scalar::Other);
    }
    if let Some(body) = value.strip_prefix('\'') {
        if body.starts_with("''") {
            return Err("multi-line strings are not supported");
        }
        return match body.split_once('\'') {
            Some((_, tail)) if is_blank_or_comment(tail) => Ok(Scalar::Other),
            _ => Err("unsupported value"),
        };
    }
    let digits = value.strip_prefix(['+', '-']).unwrap_or(value);
    let end = digits
        .find(|ch: char| !(ch.is_ascii_digit() || ch == '_'))
        .unwrap_or(digits.len());
    if end > 0 && digits.starts_with(|ch: char| ch.is_ascii_digit()) && is_blank_or_comment(&digits[end..]) {
        return Ok(Scalar::Other);
    }
    Err("unsupported value")
}

fn basic_string_tail(body: &str) -> Result<(), &'static str> {
    let mut chars = body.char_indices();
    while let Some((position, ch)) = chars.next() {
        match ch {
            '\\' => {
                chars.next();
            }
            '"' if is_blank_or_comment(&body[position + 1..]) => return Ok(()),
            '"' => return Err("unsupported value"),
            _ => {}
        }
    }
    Err("unterminated string")
}

fn is_blank_or_comment(tail: &str) -> bool {
    let tail = tail.trim();
    tail.is_empty() || tail.starts_with('#')
}
