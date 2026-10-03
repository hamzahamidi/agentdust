use thiserror::Error;

#[derive(Debug, PartialEq, Eq)]
pub struct ProcArgs<'a> {
    pub exec_path: &'a [u8],
    pub args: Vec<&'a [u8]>,
    pub env: Vec<&'a [u8]>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("buffer is shorter than the argc header")]
    TooShort,
    #[error("argc is negative")]
    NegativeArgc,
    #[error("buffer ends before all arguments were read")]
    Truncated,
}

pub fn parse(buf: &[u8]) -> Result<ProcArgs<'_>, ParseError> {
    let header: [u8; 4] = buf
        .get(..4)
        .and_then(|h| h.try_into().ok())
        .ok_or(ParseError::TooShort)?;
    let argc = usize::try_from(i32::from_ne_bytes(header)).map_err(|_| ParseError::NegativeArgc)?;
    let mut rest = &buf[4..];
    let exec_path = take_cstr(&mut rest).ok_or(ParseError::Truncated)?;
    while let [0, tail @ ..] = rest {
        rest = tail;
    }
    let mut args = Vec::with_capacity(argc.min(256));
    for _ in 0..argc {
        args.push(take_cstr(&mut rest).ok_or(ParseError::Truncated)?);
    }
    let mut env = Vec::new();
    while let Some(entry) = take_cstr(&mut rest) {
        if entry.is_empty() {
            break;
        }
        env.push(entry);
    }
    Ok(ProcArgs { exec_path, args, env })
}

pub fn env_value<'a>(parsed: &ProcArgs<'a>, name: &str) -> Option<&'a [u8]> {
    parsed
        .env
        .iter()
        .find_map(|entry| entry.strip_prefix(name.as_bytes())?.strip_prefix(b"="))
}

fn take_cstr<'a>(rest: &mut &'a [u8]) -> Option<&'a [u8]> {
    let end = rest.iter().position(|&b| b == 0)?;
    let (value, tail) = rest.split_at(end);
    *rest = &tail[1..];
    Some(value)
}
