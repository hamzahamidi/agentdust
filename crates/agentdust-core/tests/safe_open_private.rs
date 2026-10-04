mod scratch;

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

use agentdust_core::safe_open::{
    SafeOpenError, open_lock_file, open_lock_file_as, open_private_append, open_private_append_as,
};
use scratch::{TempDir, make_fifo, returns_promptly};

fn mode_of(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

fn plain(path: &Path, mode: u32) {
    fs::write(path, b"x").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn a_missing_lock_file_is_created_private() {
    let dir = TempDir::private("sop-lock-new");
    let path = dir.join("a.lock");
    open_lock_file(&path).unwrap();
    assert_eq!(mode_of(&path), 0o600);
}

#[test]
fn an_existing_lock_file_is_opened_as_it_is() {
    let dir = TempDir::private("sop-lock-old");
    let path = dir.join("a.lock");
    plain(&path, 0o600);
    let mut text = String::new();
    open_lock_file(&path).unwrap().read_to_string(&mut text).unwrap();
    assert_eq!(text, "x");
}

#[test]
fn the_same_lock_file_opens_twice() {
    let dir = TempDir::private("sop-lock-twice");
    let path = dir.join("a.lock");
    let first = open_lock_file(&path).unwrap();
    let second = open_lock_file(&path).unwrap();
    drop((first, second));
}

#[test]
fn a_lock_file_that_is_a_link_or_loose_or_shared_or_foreign_is_refused() {
    let dir = TempDir::private("sop-lock-bad");

    let real = dir.join("real");
    plain(&real, 0o600);
    symlink(&real, dir.join("link.lock")).unwrap();
    assert!(matches!(
        open_lock_file(&dir.join("link.lock")),
        Err(SafeOpenError::Symlink)
    ));

    plain(&dir.join("hard.lock"), 0o600);
    fs::hard_link(dir.join("hard.lock"), dir.join("hard2")).unwrap();
    assert!(matches!(
        open_lock_file(&dir.join("hard.lock")),
        Err(SafeOpenError::HardLinked { links: 2 })
    ));

    plain(&dir.join("loose.lock"), 0o644);
    assert!(matches!(
        open_lock_file(&dir.join("loose.lock")),
        Err(SafeOpenError::LooseMode { .. })
    ));

    plain(&dir.join("mine.lock"), 0o600);
    let mine = fs::metadata(dir.join("mine.lock")).unwrap();
    let other = std::os::unix::fs::MetadataExt::uid(&mine) + 1;
    assert!(matches!(
        open_lock_file_as(&dir.join("mine.lock"), other),
        Err(SafeOpenError::ForeignOwner { .. })
    ));

    fs::create_dir(dir.join("dir.lock")).unwrap();
    assert!(open_lock_file(&dir.join("dir.lock")).is_err());
}

#[test]
fn a_fifo_in_place_of_a_lock_file_is_refused_without_blocking() {
    let dir = TempDir::private("sop-lock-fifo");
    make_fifo(&dir.join("a.lock"));
    let path = dir.join("a.lock");
    let opened = returns_promptly(move || open_lock_file(&path).map(drop));
    assert!(matches!(opened, Err(SafeOpenError::NotRegular)));
}

#[test]
fn a_missing_private_append_file_is_created_private_and_appended_to() {
    let dir = TempDir::private("sop-app-new");
    let path = dir.join("audit.log");
    open_private_append(&path).unwrap().write_all(b"a").unwrap();
    open_private_append(&path).unwrap().write_all(b"b").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"ab");
    assert_eq!(mode_of(&path), 0o600);
}

#[test]
fn an_existing_private_append_file_keeps_its_content() {
    let dir = TempDir::private("sop-app-old");
    let path = dir.join("audit.log");
    plain(&path, 0o600);
    open_private_append(&path).unwrap().write_all(b"y").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"xy");
}

#[test]
fn a_private_append_file_that_is_a_link_or_loose_or_foreign_is_refused() {
    let dir = TempDir::private("sop-app-bad");

    let real = dir.join("real");
    plain(&real, 0o600);
    symlink(&real, dir.join("link.log")).unwrap();
    assert!(matches!(
        open_private_append(&dir.join("link.log")),
        Err(SafeOpenError::Symlink)
    ));
    assert_eq!(fs::read(&real).unwrap(), b"x");

    plain(&dir.join("hard.log"), 0o600);
    fs::hard_link(dir.join("hard.log"), dir.join("hard2")).unwrap();
    assert!(matches!(
        open_private_append(&dir.join("hard.log")),
        Err(SafeOpenError::HardLinked { .. })
    ));

    plain(&dir.join("loose.log"), 0o666);
    assert!(matches!(
        open_private_append(&dir.join("loose.log")),
        Err(SafeOpenError::LooseMode { .. })
    ));

    plain(&dir.join("mine.log"), 0o600);
    let mine = fs::metadata(dir.join("mine.log")).unwrap();
    let other = std::os::unix::fs::MetadataExt::uid(&mine) + 1;
    assert!(matches!(
        open_private_append_as(&dir.join("mine.log"), other),
        Err(SafeOpenError::ForeignOwner { .. })
    ));
}

#[test]
fn a_fifo_in_place_of_a_private_append_file_is_refused_without_blocking() {
    let dir = TempDir::private("sop-app-fifo");
    make_fifo(&dir.join("a.log"));
    let path = dir.join("a.log");
    let opened = returns_promptly(move || open_private_append(&path).map(drop));
    assert!(matches!(opened, Err(SafeOpenError::NotRegular)));
}
