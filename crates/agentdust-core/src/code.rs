use std::io;
use std::time::Duration;

use crate::entropy;

pub const ALPHABET: &[u8; 25] = b"ACDEFGHJKMNPQRTUVWXY34679";
pub const LEN: usize = 4;
pub const TTL: Duration = Duration::from_secs(120);
pub const DRAW_BYTES: usize = 8;
const UNBIASED_BELOW: u8 = 250;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Approved,
    Wrong,
    Empty,
}

pub fn generate() -> io::Result<String> {
    generate_with(&mut || entropy::bytes::<DRAW_BYTES>())
}

pub fn generate_with(draw: &mut dyn FnMut() -> io::Result<[u8; DRAW_BYTES]>) -> io::Result<String> {
    let mut code = String::with_capacity(LEN);
    while code.len() < LEN {
        for byte in draw()? {
            if code.len() < LEN && byte < UNBIASED_BELOW {
                code.push(char::from(ALPHABET[usize::from(byte) % ALPHABET.len()]));
            }
        }
    }
    Ok(code)
}

pub fn check(answer: Option<&str>, expected: &str) -> Check {
    match answer {
        None | Some("") => Check::Empty,
        Some(answer) if !expected.is_empty() && answer == expected => Check::Approved,
        Some(_) => Check::Wrong,
    }
}
