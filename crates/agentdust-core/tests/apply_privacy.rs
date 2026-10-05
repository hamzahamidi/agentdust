mod apply_support;
mod scratch;

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use agentdust_core::apply::audit::AUDIT_KEYS;
use agentdust_core::apply::server::{Response, Step};
use agentdust_core::class::Class;
use apply_support::{ServerRig, accept, call, finding_at};

const USER: &str = "user-sentinel-wN8d";
const REPO: &str = "repo-sentinel-hJ5t";
const NAME: &str = "name-sentinel-qC3z";
const SENTINELS: [&str; 3] = [USER, REPO, NAME];

fn everything_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        for entry in fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            found.push(path.clone());
            if fs::symlink_metadata(&path).unwrap().is_dir() {
                stack.push(path);
            }
        }
    }
    found.sort();
    found
}

fn rig() -> ServerRig {
    let at = |name: &str| format!("/Users/{USER}/work/{REPO}/{name}");
    ServerRig::new(vec![
        finding_at(10, Class::OwnedEnded, &at("node")),
        finding_at(11, Class::OwnedEnded, &at("bin/python3")),
        finding_at(20, Class::Suspect, &at(&format!("{NAME} with space\n\u{1b}[31m"))),
        finding_at(21, Class::Suspect, &at("tunnel")),
        finding_at(22, Class::Suspect, &at("watcher")),
    ])
}

#[test]
fn nothing_a_process_chose_reaches_a_file_the_model_or_a_prompt() {
    let rig = rig();
    let created = rig.server.plan().unwrap();
    let refs: Vec<_> = created.items.iter().collect();
    let the_call = call(&created.plan_id, &refs[..5]);
    let mut prompts = Vec::new();
    let mut turn = 0;
    let report = rig
        .server
        .run(&the_call, &mut |challenge| {
            turn += 1;
            prompts.push(challenge.message.clone());
            match turn {
                1 | 2 => accept(challenge),
                3 => Response::Decline,
                _ => Response::Accept(Some("WRONG".to_owned())),
            }
        })
        .unwrap();

    let mut visible = prompts;
    visible.push(serde_json::to_string(&report).unwrap());
    visible.push(serde_json::to_string(&created.items).unwrap());
    visible.push(created.report.clone());
    for text in &visible {
        for sentinel in SENTINELS {
            assert!(!text.contains(sentinel), "{sentinel} in {text}");
        }
    }

    for path in everything_under(rig.dir.path()) {
        if fs::symlink_metadata(&path).unwrap().is_file() {
            let bytes = fs::read(&path).unwrap();
            let text = String::from_utf8_lossy(&bytes);
            for sentinel in SENTINELS {
                assert!(!text.contains(sentinel), "{sentinel} in {}", path.display());
            }
            assert!(!text.contains("/Users"), "{}", path.display());
        }
    }
}

#[test]
fn every_file_and_directory_the_apply_path_writes_is_private_and_known() {
    let rig = rig();
    let created = rig.server.plan().unwrap();
    let refs: Vec<_> = created.items.iter().collect();
    rig.server
        .run(&call(&created.plan_id, &refs[..2]), &mut |challenge| {
            accept(challenge)
        })
        .unwrap();
    let known: BTreeSet<&str> = ["audit.log", "audit.lock", "inspection", "locks"]
        .into_iter()
        .collect();
    let top: BTreeSet<String> = fs::read_dir(rig.dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    for name in &top {
        assert!(known.contains(name.as_str()), "unexpected entry {name}");
    }
    for path in everything_under(rig.dir.path()) {
        let meta = fs::symlink_metadata(&path).unwrap();
        let mode = meta.permissions().mode() & 0o7777;
        if meta.is_dir() {
            assert_eq!(mode, 0o700, "{}", path.display());
        } else {
            assert!(meta.is_file(), "{}", path.display());
            assert_eq!(mode, 0o600, "{}", path.display());
            assert_eq!(meta.nlink(), 1, "{}", path.display());
        }
    }
}

#[test]
fn the_audit_log_holds_only_the_named_keys() {
    let rig = rig();
    let created = rig.server.plan().unwrap();
    let refs: Vec<_> = created.items.iter().collect();
    rig.server
        .run(&call(&created.plan_id, &refs[..3]), &mut |challenge| {
            accept(challenge)
        })
        .unwrap();
    let allowed: BTreeSet<&str> = AUDIT_KEYS.iter().copied().collect();
    let lines = rig.audit();
    assert!(!lines.is_empty());
    for line in &lines {
        let keys: BTreeSet<&str> = line.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, allowed);
        let exe = &line["exe_base"];
        assert!(
            exe.is_null() || matches!(exe.as_str(), Some("node" | "python3")),
            "{exe}"
        );
    }
}

#[test]
fn a_stopped_server_leaves_no_report_and_no_lock() {
    let rig = rig();
    let created = rig.server.plan().unwrap();
    let refs: Vec<_> = created.items.iter().collect();
    rig.server
        .run(&call(&created.plan_id, &refs[..2]), &mut |challenge| {
            accept(challenge)
        })
        .unwrap();
    let Step::Ask(_) = rig.server.begin(&call(&created.plan_id, &refs[2..3])).unwrap() else {
        panic!("a prompt is expected");
    };
    let dir = rig.dir.path().to_path_buf();
    let ServerRig { server, .. } = rig;
    drop(server);
    for sub in ["inspection", "locks"] {
        let left = fs::read_dir(dir.join(sub)).map(|read| read.count()).unwrap_or(0);
        assert_eq!(left, 0, "{sub}");
    }
}
