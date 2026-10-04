use crate::class::{Actionability, actionable_as};
use crate::classifier::Finding;
use crate::finding::ModelFinding;
use crate::identity::{IdentityEvidence, ProcessIdentity};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanItem {
    pub model: ModelFinding,
    pub identity: ProcessIdentity,
}

impl PlanItem {
    pub fn from_finding(finding: &Finding, model: ModelFinding) -> Option<PlanItem> {
        let actionable = matches!(
            actionable_as(finding.class),
            Actionability::BatchCode | Actionability::PerItemCode
        );
        let kernel = finding.identity.kernel.clone();
        let exe_path = finding.identity.exe_path.clone()?;
        (actionable && kernel.pid > 1 && model.class == finding.class).then_some(PlanItem {
            model,
            identity: ProcessIdentity {
                kernel,
                evidence: IdentityEvidence { exe_path },
            },
        })
    }
}
