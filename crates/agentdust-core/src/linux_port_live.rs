use std::io;
use std::path::Path;
use std::sync::Arc;

use crate::apply::exec::{Deps, Executor, Settings};
use crate::apply::timer::SystemTimer;
use crate::identity::ProcessIdentity;
use crate::linux::{LinuxPidfdSignaller, LinuxProvider, tcp_listeners};
use crate::live::LiveSurveyor;
use crate::port::{self, ListenerSource, PortSurveyor, Report};

pub struct LinuxListeners;

impl ListenerSource for LinuxListeners {
    fn listeners(&self, port: u16) -> io::Result<Vec<ProcessIdentity>> {
        tcp_listeners(port)
    }
}

pub fn run(data_dir: &Path, port: u16, resolve: bool) -> io::Result<Report> {
    if resolve {
        use crate::journal::volume::{self, SystemVolume};

        match volume::locate(&SystemVolume, data_dir) {
            Ok(facts) if facts.supported => {}
            Ok(facts) => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("port recovery requires writable ext4; found {}", facts.describe()),
                ));
            }
            Err(error) => {
                return Err(io::Error::other(format!(
                    "cannot verify port recovery data volume: {error}"
                )));
            }
        }
    }
    let surveyor = Arc::new(PortSurveyor {
        port,
        listeners: Arc::new(LinuxListeners),
        processes: Arc::new(LiveSurveyor::new(data_dir)),
    });
    let executor = Executor::new(
        Deps {
            data_dir: data_dir.to_path_buf(),
            surveyor: surveyor.clone(),
            provider: Box::new(LinuxProvider::new()?),
            signaller: Box::new(LinuxPidfdSignaller::new()?),
            timer: Arc::new(SystemTimer::new()),
        },
        Settings::default(),
    );
    port::run(port, resolve, &surveyor, &executor)
}
