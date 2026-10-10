use std::io;
use std::path::{Path, PathBuf};

use crate::classifier::{Finding, Policy, classify};
use crate::cwd::{CwdRelation, RelationContext, relate};
use crate::doctor::load_sessions;
use crate::finding::ModelFinding;
use crate::inventory::{IDLE_SAMPLE_GAP, LiveDetails, SystemClock, take};
use crate::journal;
use crate::journal::retention::Policy as RetentionPolicy;
use crate::journal::volume::SystemVolume;
use crate::platform::{self, Live, ManagedProcesses, Provider, Source};
use crate::secret;
use crate::session::ProviderLiveness;
use crate::survey::Surveyor;

pub struct LiveSurveyor {
    data_dir: PathBuf,
    relation: RelationContext,
}

impl LiveSurveyor {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
            relation: RelationContext::system(),
        }
    }
}

impl Surveyor for LiveSurveyor {
    fn survey(&self) -> io::Result<Vec<Finding>> {
        journal::maintain_if_due(
            &self.data_dir,
            &RetentionPolicy::default(),
            crate::clock::wall_ms(),
            &platform::boot_identity()?,
        )
        .map_err(io::Error::other)?;
        let secret = secret::load_existing(&self.data_dir).ok();
        let source = Source::new(secret.as_ref())?;
        let provider = Provider::new()?;
        let snapshot = take(&source, &ManagedProcesses, &SystemClock, IDLE_SAMPLE_GAP)?;
        let sessions = load_sessions(
            &self.data_dir,
            &SystemVolume,
            secret.is_some(),
            &ProviderLiveness(&provider),
        );
        let policy = Policy::new(platform::current_uid(), std::process::id() as i32);
        Ok(classify(&snapshot, &sessions.provenance(), &policy))
    }

    fn describe(&self, finding: &Finding) -> ModelFinding {
        let relation = Live::new()
            .ok()
            .and_then(|live| live.cwd(&finding.identity.kernel))
            .map_or(CwdRelation::Other, |cwd| relate(&cwd, &self.relation));
        ModelFinding::new(finding, relation)
    }
}
