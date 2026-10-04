use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

const HEX: &[u8; 16] = b"0123456789abcdef";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    Session,
    Cwd,
}

impl Domain {
    pub fn label(self) -> &'static str {
        match self {
            Self::Session => "AGENTDUST-SESSION-v1",
            Self::Cwd => "AGENTDUST-CWD-v1",
        }
    }
}

pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut mac = start(key);
    mac.update(message);
    mac.finalize().into_bytes().into()
}

pub fn keyed_digest(key: &[u8], domain: Domain, data: &[u8]) -> String {
    let mut mac = start(key);
    mac.update(domain.label().as_bytes());
    mac.update(&[0]);
    mac.update(data);
    to_hex(&mac.finalize().into_bytes())
}

fn start(key: &[u8]) -> HmacSha256 {
    HmacSha256::new_from_slice(key).expect("HMAC accepts keys of any length")
}

pub(crate) fn to_hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from(HEX[usize::from(byte >> 4)]));
        text.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    text
}
