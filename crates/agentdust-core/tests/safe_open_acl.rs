#![cfg(target_os = "macos")]

mod journal_support;
mod maintenance_support;
mod scratch;
mod secret_support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use agentdust_core::journal::{JournalError, MaintenanceError};
use agentdust_core::safe_open::{Access, SafeOpenError, check_dir, ensure_dir, open_dir, open_file};
use agentdust_core::secret::SecretError;
use journal_support::{frame, frames, journal, named, names_in, plant};
use maintenance_support::{BOOT, T, keep_everything, numbered, refused_by, snapshot, without_lock};
use scratch::TempDir;
use secret_support::{load_or_create, place};

const ALLOW_READ: &str = "everyone allow read";
const ALLOW_LIST: &str = "everyone allow list";
const ALLOW_INHERIT_FILES: &str = "everyone allow read,file_inherit";
const ALLOW_INHERIT_DIRS: &str = "everyone allow list,directory_inherit";
const ALLOW_INHERIT_BOTH: &str = "everyone allow list,file_inherit,directory_inherit";
const ALLOW_ONLY_INHERIT: &str = "everyone allow read,file_inherit,only_inherit";
const DENY_DELETE: &str = "everyone deny delete";

struct Acl(PathBuf);

impl Drop for Acl {
    fn drop(&mut self) {
        let _ = Command::new("/bin/chmod").arg("-N").arg(&self.0).status();
    }
}

fn with_acl(path: &Path, entry: &str) -> Acl {
    let status = Command::new("/bin/chmod")
        .args(["+a", entry])
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success(), "chmod +a {entry:?} {path:?}");
    Acl(path.to_path_buf())
}

fn keep(dir: &TempDir, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, b"keep").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    path
}

fn is_acl(result: &Result<impl std::fmt::Debug, SafeOpenError>) -> bool {
    matches!(result, Err(SafeOpenError::ExtendedAcl))
}

#[test]
fn a_file_with_an_allow_entry_is_refused_for_every_access() {
    let dir = TempDir::private("acl-file-allow");
    let path = keep(&dir, "state");
    let _acl = with_acl(&path, ALLOW_READ);
    for access in [Access::Read, Access::Append] {
        let result = open_file(&path, access);
        assert!(is_acl(&result), "{access:?}: {result:?}");
    }
    assert_eq!(fs::read(&path).unwrap(), b"keep");
}

#[test]
fn a_directory_with_an_allow_entry_is_refused_by_every_check() {
    let dir = TempDir::private("acl-dir-allow");
    let _acl = with_acl(&dir, ALLOW_LIST);
    assert!(is_acl(&check_dir(&dir)));
    assert!(is_acl(&open_dir(&dir)));
    assert!(is_acl(&ensure_dir(&dir)));
}

#[test]
fn an_acl_with_only_deny_entries_is_accepted() {
    let dir = TempDir::private("acl-deny-only");
    let path = keep(&dir, "state");
    let _file_acl = with_acl(&path, DENY_DELETE);
    let _dir_acl = with_acl(&dir, DENY_DELETE);
    for access in [Access::Read, Access::Append] {
        let result = open_file(&path, access);
        assert!(result.is_ok(), "{access:?}: {result:?}");
    }
    assert!(check_dir(&dir).is_ok());
    assert!(open_dir(&dir).is_ok());
}

#[test]
fn one_allow_entry_beside_a_deny_entry_is_refused() {
    let dir = TempDir::private("acl-deny-and-allow");
    let path = keep(&dir, "state");
    let _deny = with_acl(&path, DENY_DELETE);
    let _allow = with_acl(&path, ALLOW_READ);
    let result = open_file(&path, Access::Read);
    assert!(is_acl(&result), "{result:?}");
}

#[test]
fn a_file_created_below_an_inheritable_allow_entry_is_refused_and_removed() {
    let dir = TempDir::private("acl-inherit-file");
    let _acl = with_acl(&dir, ALLOW_INHERIT_FILES);
    let child = dir.join("created");
    let result = open_file(&child, Access::Create);
    assert!(is_acl(&result), "{result:?}");
    assert!(!child.exists());
}

#[test]
fn a_directory_created_below_an_inheritable_allow_entry_is_refused_and_removed() {
    let parent = TempDir::private("acl-inherit-dir");
    let _acl = with_acl(&parent, ALLOW_INHERIT_DIRS);
    let leaf = parent.join("data");
    let result = ensure_dir(&leaf);
    assert!(is_acl(&result), "{result:?}");
    assert!(!leaf.exists());
}

#[test]
fn a_directory_whose_allow_entry_only_applies_to_new_children_gets_no_file() {
    let dir = TempDir::private("acl-only-inherit");
    let _acl = with_acl(&dir, ALLOW_ONLY_INHERIT);
    assert!(is_acl(&check_dir(&dir)));
    match journal(&dir).append(&named("a")) {
        Err(JournalError::Refused(SafeOpenError::ExtendedAcl)) => {}
        other => panic!("{other:?}"),
    }
    match load_or_create(&dir) {
        Err(SecretError::Refused(SafeOpenError::ExtendedAcl)) => {}
        other => panic!("{other:?}"),
    }
    assert!(names_in(&dir).is_empty());
}

#[test]
fn an_append_into_a_data_directory_with_an_allow_entry_writes_nothing() {
    let dir = TempDir::private("acl-append-dir");
    let _acl = with_acl(&dir, ALLOW_LIST);
    match journal(&dir).append(&named("a")) {
        Err(JournalError::Refused(SafeOpenError::ExtendedAcl)) => {}
        other => panic!("{other:?}"),
    }
    assert!(names_in(&dir).is_empty());
}

#[test]
fn an_append_below_an_inheritable_allow_entry_leaves_neither_directory_nor_journal() {
    let parent = TempDir::private("acl-append-inherit");
    let _acl = with_acl(&parent, ALLOW_INHERIT_BOTH);
    match journal(&parent.join("data")).append(&named("a")) {
        Err(JournalError::Refused(SafeOpenError::ExtendedAcl)) => {}
        other => panic!("{other:?}"),
    }
    assert!(names_in(&parent).is_empty());
}

#[test]
fn a_journal_with_an_allow_entry_is_refused_by_append_and_read_and_left_alone() {
    let dir = TempDir::private("acl-journal");
    plant(&dir, "journal.jsonl", &frame(&named("a")));
    let _acl = with_acl(&dir.join("journal.jsonl"), ALLOW_READ);
    match journal(&dir).append(&named("b")) {
        Err(JournalError::Refused(SafeOpenError::ExtendedAcl)) => {}
        other => panic!("{other:?}"),
    }
    match journal(&dir).read() {
        Err(JournalError::Refused(SafeOpenError::ExtendedAcl)) => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read(dir.join("journal.jsonl")).unwrap(), frame(&named("a")));
}

#[test]
fn a_generation_with_an_allow_entry_is_not_followed_by_a_reader_and_is_counted() {
    let dir = TempDir::private("acl-reader-generation");
    plant(&dir, "journal.100.jsonl", &frame(&named("old")));
    plant(&dir, "journal.jsonl", &frame(&named("new")));
    let _acl = with_acl(&dir.join("journal.100.jsonl"), ALLOW_READ);
    let report = journal(&dir).read().unwrap();
    assert_eq!(report.unsafe_files, 1);
    assert_eq!(report.records.len(), 1);
}

#[test]
fn a_secret_with_an_allow_entry_is_refused_and_left_alone() {
    let dir = TempDir::private("acl-secret");
    let path = place(&dir, &[7u8; 32], 0o600);
    let _acl = with_acl(&path, ALLOW_READ);
    match load_or_create(&dir) {
        Err(SecretError::Refused(SafeOpenError::ExtendedAcl)) => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read(&path).unwrap(), vec![7u8; 32]);
}

#[test]
fn a_data_directory_with_an_allow_entry_gets_no_secret() {
    let dir = TempDir::private("acl-secret-dir");
    let _acl = with_acl(&dir, ALLOW_LIST);
    match load_or_create(&dir) {
        Err(SecretError::Refused(SafeOpenError::ExtendedAcl)) => {}
        other => panic!("{other:?}"),
    }
    assert!(names_in(&dir).is_empty());
}

#[test]
fn rotation_refuses_an_active_file_a_generation_and_a_lock_file_with_an_allow_entry() {
    for (label, victim) in [
        ("active", "journal.jsonl"),
        ("generation", "journal.100.jsonl"),
        ("lock", "journal.maint"),
    ] {
        let dir = TempDir::private(&format!("acl-rotate-{label}"));
        plant(&dir, "journal.jsonl", &frames(&[numbered("a", 1)]));
        if victim != "journal.jsonl" {
            plant(&dir, victim, b"");
        }
        let _acl = with_acl(&dir.join(victim), ALLOW_READ);
        let before = without_lock(snapshot(&dir));

        let (path, source) = refused_by(journal(&dir).rotate(T));

        assert_eq!(path, dir.join(victim), "{label}");
        assert!(
            matches!(source, SafeOpenError::ExtendedAcl),
            "{label}: {source:?}"
        );
        assert_eq!(without_lock(snapshot(&dir)), before, "{label}");
        assert!(dir.join("journal.jsonl").exists(), "{label}");
    }
}

#[test]
fn retention_refuses_a_generation_with_an_allow_entry_and_changes_nothing() {
    let dir = TempDir::private("acl-retain");
    plant(&dir, "journal.100.jsonl", &frames(&[numbered("a", 1)]));
    let _acl = with_acl(&dir.join("journal.100.jsonl"), ALLOW_READ);
    let before = without_lock(snapshot(&dir));

    let result = journal(&dir).retain(&keep_everything(), T, BOOT);

    match result {
        Err(MaintenanceError::Refused { source, .. }) => {
            assert!(matches!(source, SafeOpenError::ExtendedAcl), "{source:?}")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(without_lock(snapshot(&dir)), before);
}
