use std::collections::HashMap;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use crate::apply::exec::{Deps, Executor, Settings};
use crate::apply::signal::KillSignaller;
use crate::apply::timer::SystemTimer;
use crate::darwin::{self, DarwinProvider};
use crate::identity::ProcessIdentity;
use crate::live::LiveSurveyor;
use crate::port::{self, ListenerSource, PortSurveyor, Report};
use crate::provider::{ProcessProvider, ProcessRead};

const MAX_OUTPUT: u64 = 64 * 1024;
const DEADLINE: Duration = Duration::from_secs(5);

pub struct SystemListeners;

impl ListenerSource for SystemListeners {
    fn listeners(&self, port: u16) -> io::Result<Vec<ProcessIdentity>> {
        if port == 0 {
            return Err(io::Error::other("invalid port"));
        }
        let provider = DarwinProvider::new()?;
        let mut before = HashMap::new();
        for pid in darwin::list_pids()? {
            if let Ok(ProcessRead::Present(identity)) = provider.read(pid) {
                before.insert(pid, identity);
            }
        }
        let bytes = capture(port)?;
        port::parse_lsof(&bytes, port)?
            .into_iter()
            .map(|pid| match (before.get(&pid), provider.read(pid)?) {
                (Some(before), ProcessRead::Present(after)) if before == &after => Ok(after),
                _ => Err(io::Error::other("listener identity unavailable or changed")),
            })
            .collect()
    }
}

fn capture(port: u16) -> io::Result<Vec<u8>> {
    let mut command = Command::new("/usr/sbin/lsof");
    command.args(["-nP", "-a", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-F0pn"]);
    capture_command(command, DEADLINE)
}

fn capture_command(mut command: Command, timeout: Duration) -> io::Result<Vec<u8>> {
    let mut child = command
        .env_clear()
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let (tx, rx) = mpsc::channel();
    for (index, pipe) in [
        child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>),
        child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .enumerate()
    {
        let tx = tx.clone();
        thread::spawn(move || {
            let result = pipe
                .ok_or_else(|| io::Error::other("missing inventory pipe"))
                .and_then(|pipe| {
                    let mut bytes = Vec::new();
                    pipe.take(MAX_OUTPUT + 1).read_to_end(&mut bytes)?;
                    Ok(bytes)
                });
            let _ = tx.send((index, result));
        });
    }
    drop(tx);
    let deadline = Instant::now() + timeout;
    let result = (|| {
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other("listener inventory timed out"));
            }
            thread::sleep(Duration::from_millis(10));
        };
        let mut captured = [Vec::new(), Vec::new()];
        for _ in 0..2 {
            let (index, result) = rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|_| io::Error::other("listener inventory timed out"))?;
            captured[index] = result?;
        }
        if captured.iter().any(|bytes| bytes.len() as u64 > MAX_OUTPUT) || !captured[1].is_empty() {
            return Err(io::Error::other("listener inventory incomplete"));
        }
        match status.code() {
            Some(0) if !captured[0].is_empty() => Ok(std::mem::take(&mut captured[0])),
            Some(1) if captured[0].is_empty() => Ok(Vec::new()),
            _ => Err(io::Error::other("listener inventory failed")),
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

pub fn run(dir: &Path, port: u16, resolve: bool) -> io::Result<Report> {
    let surveyor = Arc::new(PortSurveyor {
        port,
        listeners: Arc::new(SystemListeners),
        processes: Arc::new(LiveSurveyor::new(dir)),
    });
    let executor = Executor::new(
        Deps {
            data_dir: dir.to_path_buf(),
            surveyor: surveyor.clone(),
            provider: Box::new(DarwinProvider::new()?),
            signaller: Box::new(KillSignaller),
            timer: Arc::new(SystemTimer::new()),
        },
        Settings::default(),
    );
    port::run(port, resolve, &surveyor, &executor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(script: &str, timeout: Duration) -> io::Result<Vec<u8>> {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        capture_command(command, timeout)
    }

    #[test]
    fn capture_accepts_only_clean_empty_status_one() {
        assert!(shell("exit 1", Duration::from_secs(1)).unwrap().is_empty());
        for script in [
            "exit 0",
            "exit 2",
            "printf warning >&2; exit 1",
            "printf record; exit 1",
        ] {
            assert!(shell(script, Duration::from_secs(1)).is_err());
        }
        assert_eq!(shell("printf record", Duration::from_secs(1)).unwrap(), b"record");
    }

    #[test]
    fn oversized_output_and_timeouts_are_refused() {
        assert!(shell("/usr/bin/head -c 65537 /dev/zero", Duration::from_secs(1)).is_err());
        let start = Instant::now();
        assert!(shell("exec /bin/sleep 2", Duration::from_millis(40)).is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
