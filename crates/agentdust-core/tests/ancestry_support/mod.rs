#![allow(dead_code)]

use std::collections::HashMap;
use std::io;
use std::sync::Mutex;

use agentdust_core::ancestry::AncestryProvider;
use agentdust_core::identity::KernelIdentity;
use agentdust_core::provider::{ProcessProvider, ProcessRead};

use crate::common::{changed, identity, kernel};

enum Script {
    Absent,
    Argument(Vec<u8>),
    Fails,
}

enum Parent {
    Live,
    Fails,
    Reused,
}

struct Node {
    read: ProcessRead,
    read_fails: bool,
    ppid: i32,
    script: Script,
    parent: Parent,
}

#[derive(Default)]
pub struct Tree {
    nodes: HashMap<i32, Node>,
    reads: Mutex<Vec<i32>>,
    script_reads: Mutex<Vec<i32>>,
}

impl Tree {
    pub fn new() -> Self {
        Self::default()
    }

    fn node(mut self, pid: i32, ppid: i32, read: ProcessRead) -> Self {
        self.nodes.insert(
            pid,
            Node {
                read,
                read_fails: false,
                ppid,
                script: Script::Absent,
                parent: Parent::Live,
            },
        );
        self
    }

    fn edit(mut self, pid: i32, edit: impl FnOnce(&mut Node)) -> Self {
        edit(self.nodes.get_mut(&pid).expect("the process is in the tree"));
        self
    }

    pub fn process(self, pid: i32, ppid: i32, exe_path: &str) -> Self {
        let present = changed(&identity(pid), |id| id.evidence.exe_path = exe_path.into());
        self.node(pid, ppid, ProcessRead::Present(present))
    }

    pub fn unreadable_path(self, pid: i32, ppid: i32) -> Self {
        self.node(pid, ppid, ProcessRead::PathUnreadable(kernel(pid)))
    }

    pub fn gone(self, pid: i32, ppid: i32) -> Self {
        self.node(pid, ppid, ProcessRead::Gone)
    }

    pub fn read_error(self, pid: i32, ppid: i32) -> Self {
        self.node(pid, ppid, ProcessRead::Gone)
            .edit(pid, |node| node.read_fails = true)
    }

    pub fn script(self, pid: i32, argument: &str) -> Self {
        self.edit(pid, |node| {
            node.script = Script::Argument(argument.as_bytes().to_vec())
        })
    }

    pub fn script_error(self, pid: i32) -> Self {
        self.edit(pid, |node| node.script = Script::Fails)
    }

    pub fn parent_error(self, pid: i32) -> Self {
        self.edit(pid, |node| node.parent = Parent::Fails)
    }

    pub fn reused_before_the_parent_is_read(self, pid: i32) -> Self {
        self.edit(pid, |node| node.parent = Parent::Reused)
    }

    pub fn reads_of(&self, pid: i32) -> usize {
        self.reads
            .lock()
            .unwrap()
            .iter()
            .filter(|read| **read == pid)
            .count()
    }

    pub fn all_reads(&self) -> Vec<i32> {
        self.reads.lock().unwrap().clone()
    }

    pub fn script_reads(&self) -> Vec<i32> {
        self.script_reads.lock().unwrap().clone()
    }
}

pub fn chain(length: usize, agent_at: Option<usize>) -> Tree {
    let mut tree = Tree::new();
    for index in 0..length {
        let pid = 1000 + index as i32;
        let ppid = if index + 1 == length { 1 } else { pid + 1 };
        let exe = if agent_at == Some(index) {
            "/opt/claude"
        } else {
            "/bin/sh"
        };
        tree = tree.process(pid, ppid, exe);
    }
    tree
}

impl ProcessProvider for Tree {
    fn read(&self, pid: i32) -> io::Result<ProcessRead> {
        self.reads.lock().unwrap().push(pid);
        match self.nodes.get(&pid) {
            Some(node) if node.read_fails => Err(io::Error::from(io::ErrorKind::PermissionDenied)),
            Some(node) => Ok(node.read.clone()),
            None => Ok(ProcessRead::Gone),
        }
    }
}

impl AncestryProvider for Tree {
    fn parent(&self, identity: &KernelIdentity) -> io::Result<Option<i32>> {
        let Some(node) = self.nodes.get(&identity.pid) else {
            return Ok(None);
        };
        match node.parent {
            Parent::Fails => Err(io::Error::from(io::ErrorKind::PermissionDenied)),
            Parent::Reused => Ok(None),
            Parent::Live => Ok(Some(node.ppid)),
        }
    }

    fn script_argument(&self, pid: i32) -> io::Result<Option<Vec<u8>>> {
        self.script_reads.lock().unwrap().push(pid);
        match self.nodes.get(&pid).map(|node| &node.script) {
            Some(Script::Argument(argument)) => Ok(Some(argument.clone())),
            Some(Script::Fails) => Err(io::Error::from(io::ErrorKind::PermissionDenied)),
            Some(Script::Absent) | None => Ok(None),
        }
    }
}
