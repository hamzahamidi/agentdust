mod scratch;

use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use agentdust_core::safe_open::{SafeOpenError, open_user_file, open_user_file_as};
use agentdust_core::user_file::{Loaded, Snapshot, UserFileError, delete_if_unchanged, load, replace};
use scratch::{make_fifo, private_dir, returns_promptly};

fn file_with(dir: &Path, name: &str, mode: u32, content: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, content).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    path
}

fn mode_of(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

fn present(bytes: &[u8], mode: u32) -> Loaded {
    Loaded::Present(Snapshot {
        bytes: bytes.to_vec(),
        mode,
    })
}

fn is_root() -> bool {
    // SAFETY: geteuid takes no arguments and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

fn leftovers(dir: &Path, expected: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| !expected.contains(&name.as_str()))
        .collect();
    names.sort();
    names
}

#[test]
fn open_user_file_accepts_any_permission_bits() {
    let dir = private_dir("uf-open-modes");
    for mode in [0o400, 0o600, 0o640, 0o644, 0o664, 0o666, 0o755] {
        let path = file_with(&dir, &format!("settings-{mode:o}"), mode, b"{}");
        assert!(open_user_file(&path).is_ok(), "{mode:o}");
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn open_user_file_refuses_a_symlink_a_hard_link_and_a_foreign_owner() {
    let dir = private_dir("uf-open-refusals");
    let target = file_with(&dir, "target", 0o644, b"{}");
    symlink(&target, dir.join("link")).unwrap();
    assert!(matches!(
        open_user_file(&dir.join("link")),
        Err(SafeOpenError::Symlink)
    ));
    fs::hard_link(&target, dir.join("second")).unwrap();
    assert!(matches!(
        open_user_file(&target),
        Err(SafeOpenError::HardLinked { links: 2 })
    ));
    let alone = file_with(&dir, "alone", 0o644, b"{}");
    let mine = fs::metadata(&alone).unwrap().uid();
    assert!(open_user_file_as(&alone, mine).is_ok());
    assert!(matches!(
        open_user_file_as(&alone, mine + 1),
        Err(SafeOpenError::ForeignOwner { .. })
    ));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn load_reads_the_bytes_and_the_mode_of_a_regular_file() {
    let dir = private_dir("uf-load");
    let path = file_with(&dir, "settings.json", 0o644, b"{\"a\": 1}\n");
    assert_eq!(load(&path).unwrap(), present(b"{\"a\": 1}\n", 0o644));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    assert_eq!(load(&path).unwrap(), present(b"{\"a\": 1}\n", 0o640));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn load_of_a_missing_file_is_missing() {
    let dir = private_dir("uf-load-missing");
    assert_eq!(load(&dir.join("settings.json")).unwrap(), Loaded::Missing);
    assert_eq!(load(&dir.join("no-dir/settings.json")).unwrap(), Loaded::Missing);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn load_refuses_a_symlink_even_when_it_points_at_a_regular_file() {
    let dir = private_dir("uf-load-symlink");
    let target = file_with(&dir, "dotfiles-settings.json", 0o644, b"{}");
    let link = dir.join("settings.json");
    symlink(&target, &link).unwrap();
    assert!(matches!(
        load(&link),
        Err(UserFileError::Refused(SafeOpenError::Symlink))
    ));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn load_refuses_a_dangling_symlink_instead_of_calling_it_missing() {
    let dir = private_dir("uf-load-dangling");
    let link = dir.join("settings.json");
    symlink(dir.join("nowhere"), &link).unwrap();
    assert!(matches!(
        load(&link),
        Err(UserFileError::Refused(SafeOpenError::Symlink))
    ));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn load_refuses_a_fifo_without_blocking_and_a_directory() {
    let dir = private_dir("uf-load-nonregular");
    let pipe = dir.join("pipe");
    make_fifo(&pipe);
    let result = returns_promptly(move || load(&pipe));
    assert!(
        matches!(result, Err(UserFileError::Refused(SafeOpenError::NotRegular))),
        "{result:?}"
    );
    assert!(matches!(
        load(&dir),
        Err(UserFileError::Refused(SafeOpenError::NotRegular))
    ));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn load_refuses_a_hard_linked_file() {
    let dir = private_dir("uf-load-hardlink");
    let path = file_with(&dir, "settings.json", 0o644, b"{}");
    fs::hard_link(&path, dir.join("elsewhere")).unwrap();
    assert!(matches!(
        load(&path),
        Err(UserFileError::Refused(SafeOpenError::HardLinked { links: 2 }))
    ));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn load_reports_an_unreadable_file_as_an_io_error() {
    if is_root() {
        return;
    }
    let dir = private_dir("uf-load-unreadable");
    let path = file_with(&dir, "settings.json", 0o000, b"{}");
    match load(&path) {
        Err(UserFileError::Io(err)) => assert_eq!(err.kind(), ErrorKind::PermissionDenied),
        other => panic!("{other:?}"),
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn replace_writes_the_new_bytes_and_keeps_the_mode() {
    let dir = private_dir("uf-replace-mode");
    for mode in [0o600, 0o640, 0o644, 0o664] {
        let path = file_with(&dir, &format!("settings-{mode:o}"), mode, b"old");
        replace(&path, &present(b"old", mode), b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new", "{mode:o}");
        assert_eq!(mode_of(&path), mode, "{mode:o}");
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn replace_swaps_the_file_by_rename_and_leaves_no_temporary_file() {
    let dir = private_dir("uf-replace-rename");
    let path = file_with(&dir, "settings.json", 0o644, b"old");
    let before = fs::metadata(&path).unwrap().ino();
    replace(&path, &present(b"old", 0o644), b"new").unwrap();
    assert_ne!(fs::metadata(&path).unwrap().ino(), before);
    assert_eq!(leftovers(&dir, &["settings.json"]), Vec::<String>::new());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn replace_creates_a_missing_file_with_mode_0600() {
    let dir = private_dir("uf-replace-create");
    let path = dir.join("settings.json");
    replace(&path, &Loaded::Missing, b"{}").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"{}");
    assert_eq!(mode_of(&path), 0o600);
    assert_eq!(leftovers(&dir, &["settings.json"]), Vec::<String>::new());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn replace_creates_a_missing_parent_directory_with_mode_0700() {
    let dir = private_dir("uf-replace-parent");
    let path = dir.join("claude/settings.json");
    replace(&path, &Loaded::Missing, b"{}").unwrap();
    assert_eq!(mode_of(&dir.join("claude")), 0o700);
    assert_eq!(fs::read(&path).unwrap(), b"{}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn replace_refuses_when_the_file_changed_after_it_was_read() {
    let dir = private_dir("uf-replace-changed");
    let path = file_with(&dir, "settings.json", 0o644, b"old");
    let read_earlier = present(b"old", 0o644);
    fs::write(&path, b"edited by the user in the meantime").unwrap();
    assert!(matches!(
        replace(&path, &read_earlier, b"new"),
        Err(UserFileError::Changed)
    ));
    assert_eq!(fs::read(&path).unwrap(), b"edited by the user in the meantime");
    assert_eq!(leftovers(&dir, &["settings.json"]), Vec::<String>::new());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn replace_refuses_when_a_file_appeared_that_was_missing_when_read() {
    let dir = private_dir("uf-replace-appeared");
    let path = file_with(&dir, "settings.json", 0o644, b"someone else wrote this");
    assert!(matches!(
        replace(&path, &Loaded::Missing, b"{}"),
        Err(UserFileError::Changed)
    ));
    assert_eq!(fs::read(&path).unwrap(), b"someone else wrote this");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn replace_refuses_a_symlink_that_took_the_place_of_the_file() {
    let dir = private_dir("uf-replace-symlink");
    let target = file_with(&dir, "target", 0o644, b"keep");
    let link = dir.join("settings.json");
    symlink(&target, &link).unwrap();
    assert!(matches!(
        replace(&link, &present(b"keep", 0o644), b"new"),
        Err(UserFileError::Refused(SafeOpenError::Symlink))
    ));
    assert!(matches!(
        replace(&link, &Loaded::Missing, b"new"),
        Err(UserFileError::Refused(SafeOpenError::Symlink))
    ));
    assert_eq!(fs::read(&target).unwrap(), b"keep");
    assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn replace_never_creates_the_target_of_a_dangling_symlink() {
    let dir = private_dir("uf-replace-dangling");
    let missing = dir.join("nowhere");
    let link = dir.join("settings.json");
    symlink(&missing, &link).unwrap();
    assert!(replace(&link, &Loaded::Missing, b"new").is_err());
    assert!(!missing.exists());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn replace_fails_and_changes_nothing_when_the_directory_is_not_writable() {
    if is_root() {
        return;
    }
    let dir = private_dir("uf-replace-readonly");
    let sub = dir.join("claude");
    fs::DirBuilder::new().mode(0o755).create(&sub).unwrap();
    let path = file_with(&sub, "settings.json", 0o644, b"old");
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o555)).unwrap();
    let result = replace(&path, &present(b"old", 0o644), b"new");
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(result, Err(UserFileError::Io(_))), "{result:?}");
    assert_eq!(fs::read(&path).unwrap(), b"old");
    assert_eq!(leftovers(&sub, &["settings.json"]), Vec::<String>::new());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn delete_if_unchanged_removes_only_the_file_it_was_given() {
    let dir = private_dir("uf-delete");
    let path = file_with(&dir, "settings.json", 0o600, b"ours");
    assert!(matches!(
        delete_if_unchanged(&path, &present(b"theirs", 0o600)),
        Err(UserFileError::Changed)
    ));
    assert!(path.exists());
    delete_if_unchanged(&path, &present(b"ours", 0o600)).unwrap();
    assert!(!path.exists());
    delete_if_unchanged(&path, &Loaded::Missing).unwrap();
    fs::remove_dir_all(&dir).unwrap();
}
