use std::io;
use std::path::{Path, PathBuf};

use crate::classifier::{Finding, Policy, classify};
use crate::cwd::{CwdRelation, RelationContext, relate};
use crate::darwin::{self, DarwinLive, DarwinProvider, DarwinSource};
use crate::doctor::load_sessions;
use crate::finding::ModelFinding;
use crate::inventory::{IDLE_SAMPLE_GAP, LaunchctlList, LiveDetails, SystemClock, take};
use crate::journal::volume::SystemVolume;
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
        let secret = secret::load_existing(&self.data_dir).ok();
        let source = DarwinSource::new(secret.as_ref())?;
        let provider = DarwinProvider::new()?;
        let snapshot = take(&source, &LaunchctlList, &SystemClock, IDLE_SAMPLE_GAP)?;
        let sessions = load_sessions(
            &self.data_dir,
            &SystemVolume,
            secret.is_some(),
            &ProviderLiveness(&provider),
        );
        let policy = Policy::new(darwin::current_uid(), std::process::id() as i32);
        Ok(classify(&snapshot, &sessions.provenance(), &policy))
    }

    fn describe(&self, finding: &Finding) -> ModelFinding {
        let relation = DarwinLive::new()
            .ok()
            .and_then(|live| live.cwd(&finding.identity.kernel))
            .map_or(CwdRelation::Other, |cwd| relate(&cwd, &self.relation));
        ModelFinding::new(finding, relation)
    }
}
