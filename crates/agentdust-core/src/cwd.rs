use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::digest::{Domain, keyed_digest};
use crate::journal::CwdKey;
use crate::secret::Secret;

pub fn normalize(path: &str) -> Option<String> {
    if !path.starts_with('/') || path.contains('\0') {
        return None;
    }
    let mut kept: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                kept.pop();
            }
            name => kept.push(name),
        }
    }
    if kept.is_empty() {
        return Some("/".to_owned());
    }
    let mut normalised = String::with_capacity(path.len());
    for name in kept {
        normalised.push('/');
        normalised.push_str(name);
    }
    Some(normalised)
}

pub fn canonical_cwd(path: &str) -> Option<String> {
    let lexical = normalize(path)?;
    let resolved = fs::canonicalize(&lexical)
        .ok()
        .and_then(|real| real.into_os_string().into_string().ok());
    Some(resolved.unwrap_or(lexical))
}

pub fn cwd_key(secret: &Secret, cwd: &str) -> Option<CwdKey> {
    let canonical = canonical_cwd(cwd)?;
    CwdKey::try_from(keyed_digest(secret.as_bytes(), Domain::Cwd, canonical.as_bytes())).ok()
}

const REPO_SEARCH_LIMIT: usize = 64;
const TEMP_ROOTS: [&str; 4] = ["/tmp", "/private/tmp", "/var/folders", "/private/var/folders"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CwdRelation {
    SameRepo,
    OtherRepo,
    Home,
    Temp,
    Other,
}

impl CwdRelation {
    pub const fn as_str(self) -> &'static str {
        match self {
            CwdRelation::SameRepo => "same_repo",
            CwdRelation::OtherRepo => "other_repo",
            CwdRelation::Home => "home",
            CwdRelation::Temp => "temp",
            CwdRelation::Other => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationContext {
    pub reference: Option<PathBuf>,
    pub home: Option<PathBuf>,
    pub temp_roots: Vec<PathBuf>,
}

impl RelationContext {
    pub fn system() -> Self {
        let mut temp_roots: Vec<PathBuf> = TEMP_ROOTS.iter().map(PathBuf::from).collect();
        if let Ok(dir) = fs::canonicalize(std::env::temp_dir())
            && !temp_roots.contains(&dir)
        {
            temp_roots.push(dir);
        }
        Self {
            reference: std::env::current_dir().ok(),
            home: std::env::var_os("HOME")
                .filter(|home| !home.is_empty())
                .map(PathBuf::from),
            temp_roots,
        }
    }
}

pub fn repo_root(path: &Path, stop_at: Option<&Path>) -> Option<PathBuf> {
    for dir in path.ancestors().take(REPO_SEARCH_LIMIT) {
        if stop_at == Some(dir) {
            return None;
        }
        if fs::symlink_metadata(dir.join(".git")).is_ok() {
            return Some(dir.to_path_buf());
        }
    }
    None
}

pub fn relate(cwd: &Path, context: &RelationContext) -> CwdRelation {
    let home = context.home.as_deref();
    if let Some(root) = repo_root(cwd, home) {
        let same = context
            .reference
            .as_deref()
            .and_then(|reference| repo_root(reference, home))
            .is_some_and(|reference_root| reference_root == root);
        return if same {
            CwdRelation::SameRepo
        } else {
            CwdRelation::OtherRepo
        };
    }
    if home == Some(cwd) {
        return CwdRelation::Home;
    }
    if context.temp_roots.iter().any(|root| cwd.starts_with(root)) {
        return CwdRelation::Temp;
    }
    CwdRelation::Other
}
