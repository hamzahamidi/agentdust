use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_EXE_BASE_LEN: usize = 64;
pub const MAX_CWD_KEY_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FieldError {
    #[error("longer than {max} bytes ({len})")]
    TooLong { len: usize, max: usize },
    #[error("contains a control character")]
    Control,
    #[error("is empty")]
    Empty,
    #[error("is not lowercase hexadecimal")]
    NotHex,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct ExeBase(String);

impl ExeBase {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ExeBase {
    type Error = FieldError;

    fn try_from(value: String) -> Result<Self, FieldError> {
        if value.len() > MAX_EXE_BASE_LEN {
            return Err(FieldError::TooLong {
                len: value.len(),
                max: MAX_EXE_BASE_LEN,
            });
        }
        if value.chars().any(is_control_or_bidi) {
            return Err(FieldError::Control);
        }
        Ok(Self(value))
    }
}

impl TryFrom<&str> for ExeBase {
    type Error = FieldError;

    fn try_from(value: &str) -> Result<Self, FieldError> {
        Self::try_from(value.to_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct CwdKey(String);

impl CwdKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for CwdKey {
    type Error = FieldError;

    fn try_from(value: String) -> Result<Self, FieldError> {
        if value.is_empty() {
            return Err(FieldError::Empty);
        }
        if value.len() > MAX_CWD_KEY_LEN {
            return Err(FieldError::TooLong {
                len: value.len(),
                max: MAX_CWD_KEY_LEN,
            });
        }
        if !value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(FieldError::NotHex);
        }
        Ok(Self(value))
    }
}

impl TryFrom<&str> for CwdKey {
    type Error = FieldError;

    fn try_from(value: &str) -> Result<Self, FieldError> {
        Self::try_from(value.to_owned())
    }
}

fn is_control_or_bidi(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
        )
}
