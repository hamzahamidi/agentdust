use std::fs;

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
