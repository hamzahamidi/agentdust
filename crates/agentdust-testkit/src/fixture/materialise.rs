use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::time::{Duration, Instant};

use agentdust_core::revalidate::Revalidation;
use thiserror::Error;

use super::{Fixture, Member, ParentExpectation, Plan, PlanProblem};
use crate::harness::{self, Harness, ProcHandle};

#[derive(Debug, Error)]
pub enum MaterialiseError {
    #[error("the fixture cannot be started: {}", describe(.0))]
    Plan(Vec<PlanProblem>),
    #[error(transparent)]
    Harness(#[from] harness::Error),
}

fn describe(problems: &[PlanProblem]) -> String {
    problems
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub role: String,
    pub detail: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "role {}: {}", self.role, self.detail)
    }
}

pub struct Materialised {
    harness: Harness,
    plan: Plan,
    handles: BTreeMap<String, ProcHandle>,
    order: Vec<String>,
    started: Instant,
}

pub fn materialise(
    fixture: &Fixture,
    fixture_bin: impl AsRef<Path>,
) -> Result<Materialised, MaterialiseError> {
    let plan = fixture.plan().map_err(MaterialiseError::Plan)?;
    materialise_plan(&plan, fixture_bin)
}

pub fn materialise_plan(
    plan: &Plan,
    fixture_bin: impl AsRef<Path>,
) -> Result<Materialised, MaterialiseError> {
    let started = Instant::now();
    let mut harness = Harness::new(fixture_bin)?;
    let mut handles = BTreeMap::new();
    let mut order = Vec::new();
    for group in &plan.groups {
        let spec = group.parent.spec.clone().spawn(group.children.len());
        let tree = harness.spawn_tree(&spec)?;
        handles.insert(group.parent.role.clone(), tree.parent);
        order.push(group.parent.role.clone());
        for (member, handle) in group.children.iter().zip(tree.children) {
            handles.insert(member.role.clone(), handle);
            order.push(member.role.clone());
        }
        if group.ends {
            harness.orphan(tree.parent)?;
        }
    }
    Ok(Materialised {
        harness,
        plan: plan.clone(),
        handles,
        order,
        started,
    })
}

impl Materialised {
    pub fn harness(&self) -> &Harness {
        &self.harness
    }

    pub fn harness_mut(&mut self) -> &mut Harness {
        &mut self.harness
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    pub fn handle(&self, role: &str) -> Option<ProcHandle> {
        self.handles.get(role).copied()
    }

    pub fn roles(&self) -> Vec<&str> {
        self.order.iter().map(String::as_str).collect()
    }

    pub fn shutdown(self) -> Result<(), harness::Error> {
        self.harness.shutdown()
    }

    pub fn check_structure(&self) -> Vec<Violation> {
        self.check_structure_at(self.started.elapsed())
    }

    pub fn check_structure_at(&self, elapsed: Duration) -> Vec<Violation> {
        let mut violations = Vec::new();
        for group in &self.plan.groups {
            let parent = (!group.ends).then_some(&group.parent);
            for member in parent.into_iter().chain(&group.children) {
                if let Some(detail) = self.problem(member)
                    && !self.excused(member, elapsed)
                {
                    violations.push(Violation {
                        role: member.role.clone(),
                        detail,
                    });
                }
            }
        }
        violations
    }

    fn excused(&self, member: &Member, elapsed: Duration) -> bool {
        elapsed >= member.spec.lifetime()
            && self
                .handle(&member.role)
                .is_some_and(|handle| self.harness.revalidate(handle) == Revalidation::Gone)
    }

    fn problem(&self, member: &Member) -> Option<String> {
        let handle = self.handle(&member.role)?;
        let found = self.harness.revalidate(handle);
        if found != Revalidation::Match {
            return Some(format!("not running: {found:?}"));
        }
        let expected = match &member.structure.parent {
            ParentExpectation::Harness => std::process::id() as i32,
            ParentExpectation::Launchd => 1,
            ParentExpectation::Role(role) => self.harness.pid(self.handle(role)?),
        };
        match self.harness.ppid(handle) {
            Ok(parent) if parent == expected => {}
            Ok(parent) => return Some(format!("parent is {parent}, expected {expected}")),
            Err(err) => return Some(format!("parent cannot be read: {err}")),
        }
        let report = self.harness.report(handle);
        let own_session = report.sid == report.pid;
        (own_session != member.structure.own_session).then(|| {
            format!(
                "own session is {own_session}, expected {}",
                member.structure.own_session
            )
        })
    }
}
