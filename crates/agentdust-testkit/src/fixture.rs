use std::fmt;

use agentdust_core::class::Class;
use agentdust_core::journal::{Agent, ExeBase, Kind};
use serde::{Deserialize, Serialize};

mod load;
#[cfg(target_os = "macos")]
mod materialise;
mod plan;

pub use load::{Loaded, Problem, ProblemKind, load, load_dir, parse_str};
#[cfg(target_os = "macos")]
pub use materialise::{MaterialiseError, Materialised, Violation, materialise, materialise_plan};
pub use plan::{Group, Member, ParentExpectation, Plan, PlanProblem, Structure};

pub const SCHEMA_VERSION: u32 = 1;
pub const FIXTURE_SECONDS: u64 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Label {
    TrueOwnedEnded,
    TrueLiveOwned,
    TrueDetached,
    TrueManaged,
    TrueUnknown,
}

impl Label {
    pub const ALL: [Label; 5] = [
        Label::TrueOwnedEnded,
        Label::TrueLiveOwned,
        Label::TrueDetached,
        Label::TrueManaged,
        Label::TrueUnknown,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Label::TrueOwnedEnded => "true_owned_ended",
            Label::TrueLiveOwned => "true_live_owned",
            Label::TrueDetached => "true_detached",
            Label::TrueManaged => "true_managed",
            Label::TrueUnknown => "true_unknown",
        }
    }

    pub const fn class(self) -> Class {
        match self {
            Label::TrueOwnedEnded => Class::OwnedEnded,
            Label::TrueLiveOwned => Class::OwnedLive,
            Label::TrueDetached => Class::Suspect,
            Label::TrueManaged => Class::Managed,
            Label::TrueUnknown => Class::Unknown,
        }
    }

    pub const fn must_never_signal(self) -> bool {
        !matches!(self, Label::TrueOwnedEnded | Label::TrueDetached)
    }
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroundTruth {
    pub label: Label,
    pub class: Class,
    pub must_never_signal: bool,
}

impl GroundTruth {
    pub const fn canonical(label: Label) -> Self {
        Self {
            label,
            class: label.class(),
            must_never_signal: label.must_never_signal(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureProcess {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_role: Option<String>,
    pub flags: Vec<String>,
    pub expected: GroundTruth,
    #[serde(default, skip_serializing_if = "is_false")]
    pub record_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureEvent {
    pub kind: Kind,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exe_base: Option<ExeBase>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub schema_version: u32,
    pub name: String,
    pub description: String,
    pub agent: Agent,
    pub processes: Vec<FixtureProcess>,
    pub journal: Vec<FixtureEvent>,
    pub session_ended: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}
