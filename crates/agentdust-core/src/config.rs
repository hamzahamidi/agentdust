use std::io::{self, Read};
use std::path::Path;

use serde::Deserialize;

use crate::safe_open::{self, Access, SafeOpenError};

pub const CONFIG_FILE: &str = "config.toml";
const MAX_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disabled {
    SwitchedOff,
    Unsafe(String),
    Unreadable,
    Invalid,
}

impl Disabled {
    pub fn code(&self) -> &'static str {
        match self {
            Disabled::SwitchedOff => "switched_off",
            Disabled::Unsafe(_) => "unsafe",
            Disabled::Unreadable => "unreadable",
            Disabled::Invalid => "invalid",
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Disabled::SwitchedOff => "apply = false in config.toml switches apply off".to_owned(),
            Disabled::Unsafe(detail) => format!("config.toml is not a private regular file: {detail}"),
            Disabled::Unreadable => "config.toml exists and cannot be read".to_owned(),
            Disabled::Invalid => {
                "config.toml cannot be parsed, or its apply setting is not true or false".to_owned()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplySwitch {
    Enabled,
    Disabled(Disabled),
}

#[derive(Deserialize)]
struct Settings {
    apply: Option<bool>,
}

pub fn apply_switch(dir: &Path) -> ApplySwitch {
    match read_setting(dir) {
        Ok(Some(false)) => ApplySwitch::Disabled(Disabled::SwitchedOff),
        Ok(_) => ApplySwitch::Enabled,
        Err(reason) => ApplySwitch::Disabled(reason),
    }
}

fn read_setting(dir: &Path) -> Result<Option<bool>, Disabled> {
    let file = match safe_open::open_file(&dir.join(CONFIG_FILE), Access::Read) {
        Ok(file) => file,
        Err(SafeOpenError::Io(err)) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(SafeOpenError::Io(_)) => return Err(Disabled::Unreadable),
        Err(refused) => return Err(Disabled::Unsafe(refused.to_string())),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Disabled::Unreadable)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Disabled::Invalid);
    }
    let text = String::from_utf8(bytes).map_err(|_| Disabled::Invalid)?;
    toml::from_str::<Settings>(&text)
        .map(|settings| settings.apply)
        .map_err(|_| Disabled::Invalid)
}
