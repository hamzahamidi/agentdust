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

const VALUE_OPTIONS: [&[u8]; 7] = [
    b"-r",
    b"--require",
    b"--import",
    b"--loader",
    b"--experimental-loader",
    b"-C",
    b"--conditions",
];
const CODE_OPTIONS: [&[u8]; 4] = [b"-e", b"--eval", b"-p", b"--print"];

struct Arguments<'a> {
    exec_path: &'a [u8],
    args: Vec<&'a [u8]>,
    rest: &'a [u8],
}

fn read_args(buf: &[u8]) -> Result<Arguments<'_>, ParseError> {
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
    Ok(Arguments {
        exec_path,
        args,
        rest,
    })
}

pub fn parse(buf: &[u8]) -> Result<ProcArgs<'_>, ParseError> {
    let Arguments {
        exec_path,
        args,
        mut rest,
    } = read_args(buf)?;
    let mut env = Vec::new();
    while let Some(entry) = take_cstr(&mut rest) {
        if entry.is_empty() {
            break;
        }
        env.push(entry);
    }
    Ok(ProcArgs { exec_path, args, env })
}

pub fn script_argument(buf: &[u8]) -> Result<Option<&[u8]>, ParseError> {
    let mut rest = read_args(buf)?.args.into_iter().skip(1);
    while let Some(arg) = rest.next() {
        if arg == b"--" {
            return Ok(rest.next());
        }
        if CODE_OPTIONS.contains(&arg) {
            return Ok(None);
        }
        if VALUE_OPTIONS.contains(&arg) {
            rest.next();
            continue;
        }
        if !arg.starts_with(b"-") {
            return Ok(Some(arg));
        }
    }
    Ok(None)
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
