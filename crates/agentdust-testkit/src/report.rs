use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub pid: i32,
    pub ppid: i32,
    pub sid: i32,
    pub env: Option<Vec<u8>>,
}

#[derive(Debug, Error)]
pub enum ReportError {
    #[error("report file: {0}")]
    Io(#[from] io::Error),
    #[error("malformed report: {0}")]
    Malformed(&'static str),
}

impl Report {
    pub fn render(&self) -> Vec<u8> {
        let mut out = format!("pid={}\nppid={}\nsid={}\n", self.pid, self.ppid, self.sid).into_bytes();
        if let Some(env) = &self.env {
            out.extend_from_slice(b"env=");
            out.extend_from_slice(env);
        }
        out
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, ReportError> {
        let (pid, rest) = line(bytes, b"pid=", 1)?;
        let (ppid, rest) = line(rest, b"ppid=", 0)?;
        let (sid, rest) = line(rest, b"sid=", 1)?;
        let env = match rest {
            [] => None,
            [b'e', b'n', b'v', b'=', value @ ..] => Some(value.to_vec()),
            _ => return Err(ReportError::Malformed("unexpected bytes after the session line")),
        };
        Ok(Self { pid, ppid, sid, env })
    }
}

fn line<'a>(bytes: &'a [u8], key: &'static [u8], min: i32) -> Result<(i32, &'a [u8]), ReportError> {
    let rest = bytes
        .strip_prefix(key)
        .ok_or(ReportError::Malformed("a line is missing or out of order"))?;
    let end = rest
        .iter()
        .position(|&b| b == b'\n')
        .ok_or(ReportError::Malformed("a line is not terminated"))?;
    let digits = &rest[..end];
    let number = std::str::from_utf8(digits)
        .ok()
        .filter(|text| text.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|text| text.parse::<i32>().ok().map(|n| (n, text)))
        .filter(|(n, text)| *n >= min && n.to_string() == *text)
        .map(|(n, _)| n)
        .ok_or(ReportError::Malformed(
            "a number is out of range or not canonical",
        ))?;
    Ok((number, &rest[end + 1..]))
}

pub fn write(path: &Path, report: &Report) -> io::Result<()> {
    let mut partial = OsString::from(path);
    partial.push(".partial");
    let mut file = File::create(&partial)?;
    file.write_all(&report.render())?;
    drop(file);
    fs::rename(&partial, path)
}

pub fn read(path: &Path) -> Result<Option<Report>, ReportError> {
    match fs::read(path) {
        Ok(bytes) => Report::parse(&bytes).map(Some),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
