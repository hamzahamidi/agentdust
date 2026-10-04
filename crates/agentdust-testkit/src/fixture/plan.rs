use thiserror::Error;

use super::{FIXTURE_SECONDS, Fixture, FixtureProcess, Label};
use crate::spec::{MAX_SPAWN, ProcSpec};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParentExpectation {
    Harness,
    Role(String),
    Launchd,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Structure {
    pub parent: ParentExpectation,
    pub own_session: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub role: String,
    pub label: Label,
    pub spec: ProcSpec,
    pub structure: Structure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub parent: Member,
    pub children: Vec<Member>,
    pub ends: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub groups: Vec<Group>,
    pub record_only: Vec<String>,
    pub end_session: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PlanProblem {
    #[error("role {role}: only a root can have children that are started, and its parent is not a root")]
    TooDeep { role: String },
    #[error("role {role}: its flags differ from those of its parent {parent}, and children inherit them")]
    FlagsDiffer { role: String, parent: String },
    #[error("role {parent}: {count} children is above the limit of {max}")]
    TooManyChildren {
        parent: String,
        count: usize,
        max: usize,
    },
    #[error("role {role}: it is started but its parent {parent} is record only")]
    LiveChildOfRecordOnly { role: String, parent: String },
    #[error("role {role}: {label} needs a parent that ended with the session")]
    MustBeOrphaned { role: String, label: Label },
    #[error("role {role}: true_detached needs the --setsid flag")]
    MustHaveOwnSession { role: String },
    #[error("role {role}: true_live_owned needs a parent that is still alive")]
    MustHaveLiveParent { role: String },
    #[error("role {role}: flags do not parse: {reason}")]
    BadFlags { role: String, reason: String },
}

impl Fixture {
    pub fn process(&self, role: &str) -> Option<&FixtureProcess> {
        self.processes.iter().find(|process| process.role == role)
    }

    pub fn children_of(&self, role: &str) -> Vec<&FixtureProcess> {
        self.processes
            .iter()
            .filter(|process| process.parent_role.as_deref() == Some(role))
            .collect()
    }

    pub fn ends_with_session(&self, process: &FixtureProcess) -> bool {
        self.session_ended
            && !process.record_only
            && self
                .children_of(&process.role)
                .iter()
                .any(|child| !child.record_only)
    }

    pub fn observed(&self) -> Vec<&FixtureProcess> {
        self.processes
            .iter()
            .filter(|process| !self.ends_with_session(process))
            .collect()
    }

    pub fn plan(&self) -> Result<Plan, Vec<PlanProblem>> {
        let mut problems = Vec::new();
        let mut groups = Vec::new();
        for process in self.processes.iter().filter(|process| !process.record_only) {
            match process.parent_role.as_deref().and_then(|role| self.process(role)) {
                None => groups.push(self.group(process, &mut problems)),
                Some(parent) if parent.record_only => {
                    problems.push(PlanProblem::LiveChildOfRecordOnly {
                        role: process.role.clone(),
                        parent: parent.role.clone(),
                    });
                }
                Some(parent) if parent.parent_role.is_some() => {
                    problems.push(PlanProblem::TooDeep {
                        role: process.role.clone(),
                    });
                }
                Some(_) => {}
            }
        }
        if !problems.is_empty() {
            return Err(problems);
        }
        Ok(Plan {
            groups,
            record_only: self
                .processes
                .iter()
                .filter(|process| process.record_only)
                .map(|process| process.role.clone())
                .collect(),
            end_session: self.session_ended,
        })
    }

    fn group(&self, root: &FixtureProcess, problems: &mut Vec<PlanProblem>) -> Group {
        let live_children: Vec<&FixtureProcess> = self
            .children_of(&root.role)
            .into_iter()
            .filter(|child| !child.record_only)
            .collect();
        if live_children.len() > MAX_SPAWN {
            problems.push(PlanProblem::TooManyChildren {
                parent: root.role.clone(),
                count: live_children.len(),
                max: MAX_SPAWN,
            });
        }
        let ends = self.session_ended && !live_children.is_empty();
        let spec = spec_of(root, problems);
        let parent = member(root, spec.clone().unwrap_or_default(), ParentExpectation::Harness);
        let mut children = Vec::with_capacity(live_children.len());
        for child in live_children {
            let child_spec = spec_of(child, problems);
            if let (Some(own), Some(inherited)) = (&child_spec, &spec)
                && own != inherited
            {
                problems.push(PlanProblem::FlagsDiffer {
                    role: child.role.clone(),
                    parent: root.role.clone(),
                });
            }
            let expectation = if ends {
                ParentExpectation::Launchd
            } else {
                ParentExpectation::Role(root.role.clone())
            };
            children.push(member(child, child_spec.unwrap_or_default(), expectation));
        }
        if !ends {
            check_structure(&parent, problems);
        }
        for child in &children {
            check_structure(child, problems);
        }
        Group {
            parent,
            children,
            ends,
        }
    }
}

fn spec_of(process: &FixtureProcess, problems: &mut Vec<PlanProblem>) -> Option<ProcSpec> {
    match ProcSpec::parse(process.flags.iter().map(String::as_str)) {
        Ok(spec) if spec.seconds.is_some() => Some(spec),
        Ok(spec) => Some(spec.seconds(FIXTURE_SECONDS)),
        Err(err) => {
            problems.push(PlanProblem::BadFlags {
                role: process.role.clone(),
                reason: err.to_string(),
            });
            None
        }
    }
}

fn member(process: &FixtureProcess, spec: ProcSpec, parent: ParentExpectation) -> Member {
    let own_session = spec.setsid;
    Member {
        role: process.role.clone(),
        label: process.expected.label,
        spec,
        structure: Structure { parent, own_session },
    }
}

fn check_structure(member: &Member, problems: &mut Vec<PlanProblem>) {
    let role = || member.role.clone();
    let orphaned = member.structure.parent == ParentExpectation::Launchd;
    match member.label {
        Label::TrueOwnedEnded if !orphaned => problems.push(PlanProblem::MustBeOrphaned {
            role: role(),
            label: member.label,
        }),
        Label::TrueDetached => {
            if !orphaned {
                problems.push(PlanProblem::MustBeOrphaned {
                    role: role(),
                    label: member.label,
                });
            }
            if !member.structure.own_session {
                problems.push(PlanProblem::MustHaveOwnSession { role: role() });
            }
        }
        Label::TrueLiveOwned if !matches!(member.structure.parent, ParentExpectation::Role(_)) => {
            problems.push(PlanProblem::MustHaveLiveParent { role: role() });
        }
        _ => {}
    }
}
