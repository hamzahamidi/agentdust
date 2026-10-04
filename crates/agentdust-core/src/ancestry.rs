use std::collections::HashSet;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::identity::{KernelIdentity, ProcessIdentity};
use crate::provider::{ProcessProvider, ProcessRead};

pub const MAX_DEPTH: usize = 64;
const AGENT_EXE: &[u8] = b"claude";
const NODE_EXE: &[u8] = b"node";
const SCRIPT_MARK: &[u8] = b"claude";
const NATIVE_DIR: &[u8] = b"claude";
const NATIVE_VERSIONS: &[u8] = b"versions";

pub trait AncestryProvider: ProcessProvider {
    fn parent(&self, identity: &KernelIdentity) -> io::Result<Option<i32>>;

    fn script_argument(&self, pid: i32) -> io::Result<Option<Vec<u8>>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Executable,
    NodeScript,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub identity: ProcessIdentity,
    pub via: Anchor,
}

pub fn find_agent<P: AncestryProvider + ?Sized>(provider: &P, start: i32) -> Option<Found> {
    let mut seen = HashSet::new();
    let mut pid = start;
    for _ in 0..MAX_DEPTH {
        if pid <= 1 || !seen.insert(pid) {
            return None;
        }
        let kernel = match provider.read(pid) {
            Ok(ProcessRead::Present(identity)) => {
                if let Some(via) = anchor(provider, &identity) {
                    return Some(Found { identity, via });
                }
                identity.kernel
            }
            Ok(ProcessRead::PathUnreadable(kernel)) => kernel,
            Ok(ProcessRead::Gone) | Err(_) => return None,
        };
        pid = match provider.parent(&kernel) {
            Ok(Some(parent)) => parent,
            _ => return None,
        };
    }
    None
}

pub fn agent_exe(path: &Path) -> bool {
    let mut parts = path.as_os_str().as_bytes().rsplit(|byte| *byte == b'/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(AGENT_EXE), _, _) => true,
        (Some(version), Some(NATIVE_VERSIONS), Some(NATIVE_DIR)) => !version.is_empty(),
        _ => false,
    }
}

pub fn is_node(path: &Path) -> bool {
    basename(path) == NODE_EXE
}

pub fn script_names_agent(script: &[u8]) -> bool {
    script
        .windows(SCRIPT_MARK.len())
        .any(|window| window == SCRIPT_MARK)
}

fn anchor<P: AncestryProvider + ?Sized>(provider: &P, identity: &ProcessIdentity) -> Option<Anchor> {
    let path = &identity.evidence.exe_path;
    if agent_exe(path) {
        return Some(Anchor::Executable);
    }
    if !is_node(path) {
        return None;
    }
    let script = provider.script_argument(identity.kernel.pid).ok().flatten()?;
    script_names_agent(&script).then_some(Anchor::NodeScript)
}

fn basename(path: &Path) -> &[u8] {
    path.as_os_str()
        .as_bytes()
        .rsplit(|byte| *byte == b'/')
        .next()
        .unwrap_or_default()
}
