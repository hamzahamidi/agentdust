use std::fmt;
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

use crate::digest::{Domain, keyed_digest, to_hex};
use crate::entropy;
use crate::journal::SessionTagKey;
use crate::safe_open::{self, SafeOpenError};
use crate::secret::Secret;

pub const ENV_NAME: &str = "AGENTDUST_SESSION";
pub const CODEX_ENV_NAME: &str = "CODEX_SESSION_ID";
pub const TAG_BYTES: usize = 16;
pub const TAG_LEN: usize = TAG_BYTES * 2;

pub struct SessionTag(String);

impl SessionTag {
    pub fn from_bytes(bytes: [u8; TAG_BYTES]) -> Self {
        Self(to_hex(&bytes))
    }

    pub fn generate() -> io::Result<Self> {
        Self::generate_from(entropy::bytes::<TAG_BYTES>)
    }

    pub fn generate_from(source: impl FnOnce() -> io::Result<[u8; TAG_BYTES]>) -> io::Result<Self> {
        source().map(Self::from_bytes)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn export_line(&self) -> String {
        format!("export {ENV_NAME}={}\n", self.0)
    }

    pub fn key(&self, secret: &Secret) -> SessionTagKey {
        key_of(secret, self.0.as_bytes())
    }
}

pub fn key_of(secret: &Secret, raw: &[u8]) -> SessionTagKey {
    SessionTagKey::try_from(keyed_digest(secret.as_bytes(), Domain::Session, raw))
        .expect("a keyed digest is 64 lowercase hexadecimal characters")
}

impl fmt::Debug for SessionTag {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("SessionTag(redacted)")
    }
}

pub fn append_export(path: &Path, tag: &SessionTag) -> Result<(), SafeOpenError> {
    require_absolute(path)?;
    write_export(safe_open::open_shared_append(path)?, tag)
}

pub fn append_export_as(path: &Path, tag: &SessionTag, owner: u32) -> Result<(), SafeOpenError> {
    require_absolute(path)?;
    write_export(safe_open::open_shared_append_as(path, owner)?, tag)
}

fn require_absolute(path: &Path) -> Result<(), SafeOpenError> {
    if path.is_absolute() {
        return Ok(());
    }
    Err(SafeOpenError::Io(io::Error::new(
        io::ErrorKind::InvalidInput,
        "the environment file path must be absolute",
    )))
}

fn write_export(mut file: File, tag: &SessionTag) -> Result<(), SafeOpenError> {
    let mut text = String::new();
    if file.metadata()?.len() > 0 {
        text.push('\n');
    }
    text.push_str(&tag.export_line());
    loop {
        match file.write(text.as_bytes()) {
            Ok(written) if written == text.len() => return Ok(()),
            Ok(_) => return Err(io::Error::from(io::ErrorKind::WriteZero).into()),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err.into()),
        }
    }
}

pub fn valid_codex_session(raw: &[u8]) -> bool {
    raw.len() == 36
        && raw.iter().enumerate().all(|(i, byte)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)
            }
        })
}

pub fn codex_key_of(secret: &Secret, raw: &[u8]) -> Option<SessionTagKey> {
    valid_codex_session(raw).then(|| {
        SessionTagKey::try_from(keyed_digest(secret.as_bytes(), Domain::CodexSession, raw))
            .expect("a keyed digest is 64 lowercase hexadecimal characters")
    })
}
