use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;

pub const DEFAULT_SECONDS: u64 = 30;
pub const MAX_SPAWN: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProcSpec {
    pub seconds: Option<u64>,
    pub ignore_term: bool,
    pub exit_after_ms: Option<u64>,
    pub spawn: usize,
    pub setsid: bool,
    pub echo_env: Option<String>,
    pub report_file: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SpecError {
    #[error("unknown argument {0:?}")]
    UnknownFlag(String),
    #[error("{0} needs a value")]
    MissingValue(&'static str),
    #[error("{flag} needs a whole number, found {value:?}")]
    BadNumber { flag: &'static str, value: String },
    #[error("--spawn {requested} is above the limit of {max}")]
    TooManyChildren { requested: usize, max: usize },
    #[error("{0} was given more than once")]
    Duplicate(&'static str),
    #[error("--echo-env needs --report-file")]
    EchoWithoutReport,
    #[error("arguments must be valid Unicode")]
    NotUnicode,
}

impl ProcSpec {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn seconds(mut self, seconds: u64) -> Self {
        self.seconds = Some(seconds);
        self
    }

    pub fn ignore_term(mut self) -> Self {
        self.ignore_term = true;
        self
    }

    pub fn exit_after_ms(mut self, millis: u64) -> Self {
        self.exit_after_ms = Some(millis);
        self
    }

    pub fn spawn(mut self, children: usize) -> Self {
        self.spawn = children;
        self
    }

    pub fn setsid(mut self) -> Self {
        self.setsid = true;
        self
    }

    pub fn echo_env(mut self, name: &str) -> Self {
        self.echo_env = Some(name.to_owned());
        self
    }

    pub fn report_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.report_file = Some(path.into());
        self
    }

    pub fn env(mut self, name: &str, value: &str) -> Self {
        self.env.push((name.to_owned(), value.to_owned()));
        self
    }

    pub fn args(&self) -> Vec<OsString> {
        let mut args = Vec::new();
        if let Some(seconds) = self.seconds {
            args.push(seconds.to_string().into());
        }
        if self.ignore_term {
            args.push("--ignore-term".into());
        }
        if let Some(millis) = self.exit_after_ms {
            args.push("--exit-after-ms".into());
            args.push(millis.to_string().into());
        }
        if self.spawn > 0 {
            args.push("--spawn".into());
            args.push(self.spawn.to_string().into());
        }
        if self.setsid {
            args.push("--setsid".into());
        }
        if let Some(name) = &self.echo_env {
            args.push("--echo-env".into());
            args.push(name.into());
        }
        if let Some(path) = &self.report_file {
            args.push("--report-file".into());
            args.push(path.into());
        }
        args
    }

    pub fn parse<I, S>(args: I) -> Result<Self, SpecError>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let mut spec = Self::new();
        let mut spawned = false;
        let mut args = args.into_iter().map(|arg| text(arg.into()));
        while let Some(arg) = args.next() {
            let arg = arg?;
            match arg.as_str() {
                "--ignore-term" => flag(&mut spec.ignore_term, "--ignore-term")?,
                "--setsid" => flag(&mut spec.setsid, "--setsid")?,
                "--exit-after-ms" => {
                    let millis = number(&value(&mut args, "--exit-after-ms")?, "--exit-after-ms")?;
                    once(&mut spec.exit_after_ms, millis, "--exit-after-ms")?;
                }
                "--spawn" => {
                    let requested = number(&value(&mut args, "--spawn")?, "--spawn")?;
                    if requested > MAX_SPAWN as u64 {
                        return Err(SpecError::TooManyChildren {
                            requested: usize::try_from(requested).unwrap_or(usize::MAX),
                            max: MAX_SPAWN,
                        });
                    }
                    if spawned {
                        return Err(SpecError::Duplicate("--spawn"));
                    }
                    spawned = true;
                    spec.spawn = requested as usize;
                }
                "--echo-env" => {
                    let name = value(&mut args, "--echo-env")?;
                    once(&mut spec.echo_env, name, "--echo-env")?;
                }
                "--report-file" => {
                    let path = value(&mut args, "--report-file")?;
                    once(&mut spec.report_file, PathBuf::from(path), "--report-file")?;
                }
                other if other.starts_with('-') => return Err(SpecError::UnknownFlag(arg)),
                _ => once(&mut spec.seconds, number(&arg, "seconds")?, "seconds")?,
            }
        }
        if spec.echo_env.is_some() && spec.report_file.is_none() {
            return Err(SpecError::EchoWithoutReport);
        }
        Ok(spec)
    }

    pub fn child(&self, index: usize) -> ProcSpec {
        ProcSpec {
            spawn: 0,
            report_file: self.report_file.as_ref().map(|path| {
                let mut name = path.clone().into_os_string();
                name.push(format!(".{index}"));
                PathBuf::from(name)
            }),
            ..self.clone()
        }
    }

    pub fn lifetime(&self) -> Duration {
        let seconds = self.seconds.unwrap_or(DEFAULT_SECONDS).saturating_mul(1000);
        Duration::from_millis(seconds.min(self.exit_after_ms.unwrap_or(u64::MAX)))
    }
}

fn text(arg: OsString) -> Result<String, SpecError> {
    arg.into_string().map_err(|_| SpecError::NotUnicode)
}

fn value(
    args: &mut impl Iterator<Item = Result<String, SpecError>>,
    flag: &'static str,
) -> Result<String, SpecError> {
    args.next().ok_or(SpecError::MissingValue(flag))?
}

fn number(value: &str, flag: &'static str) -> Result<u64, SpecError> {
    let bad = || SpecError::BadNumber {
        flag,
        value: value.to_owned(),
    };
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    value.parse().map_err(|_| bad())
}

fn flag(slot: &mut bool, name: &'static str) -> Result<(), SpecError> {
    if std::mem::replace(slot, true) {
        return Err(SpecError::Duplicate(name));
    }
    Ok(())
}

fn once<T>(slot: &mut Option<T>, new: T, name: &'static str) -> Result<(), SpecError> {
    if slot.replace(new).is_some() {
        return Err(SpecError::Duplicate(name));
    }
    Ok(())
}
