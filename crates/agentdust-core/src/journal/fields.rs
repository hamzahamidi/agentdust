use std::ffi::OsStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::identity::{KernelIdentity, ProcessIdentity};

pub const MAX_EXE_BASE_LEN: usize = 64;
pub const MAX_CWD_KEY_LEN: usize = 64;
pub const SESSION_TAG_KEY_LEN: usize = 64;
pub const MAX_AGENT_IDENTITY_LEN: usize = 160;

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
    #[error("has {len} bytes and must have {expected}")]
    WrongLength { len: usize, expected: usize },
    #[error("is not a positive number")]
    NotPositive,
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct SessionTagKey(String);

impl SessionTagKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SessionTagKey {
    type Error = FieldError;

    fn try_from(value: String) -> Result<Self, FieldError> {
        if value.len() != SESSION_TAG_KEY_LEN {
            return Err(FieldError::WrongLength {
                len: value.len(),
                expected: SESSION_TAG_KEY_LEN,
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

impl TryFrom<&str> for SessionTagKey {
    type Error = FieldError;

    fn try_from(value: &str) -> Result<Self, FieldError> {
        Self::try_from(value.to_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "AgentIdentityWire")]
pub struct AgentIdentity {
    pid: i32,
    start_time_us: u64,
    uid: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    exe_base: Option<ExeBase>,
}

#[derive(Deserialize)]
struct AgentIdentityWire {
    pid: i32,
    start_time_us: u64,
    uid: u32,
    #[serde(default)]
    exe_base: Option<ExeBase>,
}

impl TryFrom<AgentIdentityWire> for AgentIdentity {
    type Error = FieldError;

    fn try_from(wire: AgentIdentityWire) -> Result<Self, FieldError> {
        Self::new(wire.pid, wire.start_time_us, wire.uid, wire.exe_base)
    }
}

impl AgentIdentity {
    pub fn new(
        pid: i32,
        start_time_us: u64,
        uid: u32,
        exe_base: Option<ExeBase>,
    ) -> Result<Self, FieldError> {
        if pid <= 0 {
            return Err(FieldError::NotPositive);
        }
        Ok(Self {
            pid,
            start_time_us,
            uid,
            exe_base,
        })
    }

    pub fn from_process(process: &ProcessIdentity) -> Result<Self, FieldError> {
        let base = process
            .evidence
            .exe_path
            .file_name()
            .and_then(OsStr::to_str)
            .and_then(|name| ExeBase::try_from(name).ok());
        let kernel = &process.kernel;
        Self::new(kernel.pid, kernel.start_time_us, kernel.uid, base)
    }

    pub fn pid(&self) -> i32 {
        self.pid
    }

    pub fn start_time_us(&self) -> u64 {
        self.start_time_us
    }

    pub fn uid(&self) -> u32 {
        self.uid
    }

    pub fn exe_base(&self) -> Option<&ExeBase> {
        self.exe_base.as_ref()
    }

    pub fn kernel(&self, boot: &str) -> KernelIdentity {
        KernelIdentity {
            boot_session_uuid: boot.to_owned(),
            pid: self.pid,
            start_time_us: self.start_time_us,
            uid: self.uid,
        }
    }
}

fn is_control_or_bidi(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
        )
}
