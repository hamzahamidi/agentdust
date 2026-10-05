use std::io;
use std::process::ExitCode;

use agentdust_core::classifier::Policy;
use agentdust_core::cwd::RelationContext;
use agentdust_core::darwin::{self, DarwinLive, DarwinProvider, DarwinSource};
use agentdust_core::doctor::{Components, run as diagnose};
use agentdust_core::inventory::{LaunchctlList, SystemClock};
use agentdust_core::journal::volume::SystemVolume;
use agentdust_core::paths;
use agentdust_core::secret;
use agentdust_core::session::ProviderLiveness;

pub fn run(json: bool) -> ExitCode {
    match report(json) {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("agentdust doctor: {err}");
            ExitCode::FAILURE
        }
    }
}

fn report(json: bool) -> io::Result<String> {
    let dir = paths::data_dir()?;
    let secret = secret::load_existing(&dir).ok();
    let source = DarwinSource::new(secret.as_ref())?;
    let provider = DarwinProvider::new()?;
    let live = DarwinLive::new()?;
    let relation = RelationContext::system();
    let diagnosis = diagnose(&Components {
        data_dir: &dir,
        volume: &SystemVolume,
        secret_available: secret.is_some(),
        processes: &source,
        launchd: &LaunchctlList,
        clock: &SystemClock,
        liveness: &ProviderLiveness(&provider),
        live: &live,
        relation: &relation,
        policy: Policy::new(darwin::current_uid(), std::process::id() as i32),
    })?;
    Ok(if json {
        diagnosis.to_json()
    } else {
        diagnosis.render(&live)
    })
}
