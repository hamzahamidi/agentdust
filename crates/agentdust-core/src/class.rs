use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Class {
    Managed,
    OwnedLive,
    OwnedEnded,
    LikelyOwned,
    Suspect,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Actionability {
    Never,
    BatchCode,
    ReportOnly,
    PerItemCode,
}

impl Class {
    pub const ALL: [Class; 6] = [
        Class::Managed,
        Class::OwnedLive,
        Class::OwnedEnded,
        Class::LikelyOwned,
        Class::Suspect,
        Class::Unknown,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Class::Managed => "managed",
            Class::OwnedLive => "owned-live",
            Class::OwnedEnded => "owned-ended",
            Class::LikelyOwned => "likely-owned",
            Class::Suspect => "suspect",
            Class::Unknown => "unknown",
        }
    }
}

impl fmt::Display for Class {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub const fn actionable_as(class: Class) -> Actionability {
    match class {
        Class::Managed | Class::OwnedLive | Class::Unknown => Actionability::Never,
        Class::OwnedEnded => Actionability::BatchCode,
        Class::LikelyOwned => Actionability::ReportOnly,
        Class::Suspect => Actionability::PerItemCode,
    }
}
