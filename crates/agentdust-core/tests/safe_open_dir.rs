mod scratch;

use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt, symlink};

use agentdust_core::safe_open::{SafeOpenError, open_dir, open_dir_as};
use scratch::{make_fifo, private_dir, returns_promptly, scratch_dir};

#[test]
fn a_private_directory_opens_and_can_be_synced() {
    let dir = private_dir("od-ok");
    let handle = open_dir(&dir).unwrap();
    handle.sync_all().unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_handle_is_the_directory_that_was_checked() {
    let dir = private_dir("od-same");
    let handle = open_dir(&dir).unwrap();
    let metadata = handle.metadata().unwrap();
    assert!(metadata.is_dir());
    assert_eq!(metadata.ino(), fs::metadata(&dir).unwrap().ino());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_symlink_to_a_directory_is_refused() {
    let dir = private_dir("od-symlink");
    let real = dir.join("real");
    fs::DirBuilder::new().mode(0o700).create(&real).unwrap();
    symlink(&real, dir.join("link")).unwrap();
    assert!(matches!(open_dir(&dir.join("link")), Err(SafeOpenError::Symlink)));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_directory_looser_than_0700_is_refused() {
    let dir = private_dir("od-loose");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        open_dir(&dir),
        Err(SafeOpenError::LooseMode {
            mode: 0o755,
            allowed: 0o700
        })
    ));
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_regular_file_is_not_a_directory() {
    let dir = private_dir("od-file");
    let path = dir.join("state");
    fs::write(&path, b"x").unwrap();
    assert!(matches!(open_dir(&path), Err(SafeOpenError::NotDirectory)));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_fifo_is_not_a_directory_and_does_not_block() {
    let dir = private_dir("od-fifo");
    let path = dir.join("pipe");
    make_fifo(&path);
    let result = returns_promptly(move || open_dir(&path));
    assert!(matches!(result, Err(SafeOpenError::NotDirectory)), "{result:?}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_directory_owned_by_another_uid_is_refused() {
    let dir = private_dir("od-owner");
    let mine = fs::metadata(&dir).unwrap().uid();
    assert!(open_dir_as(&dir, mine).is_ok());
    match open_dir_as(&dir, mine + 1) {
        Err(SafeOpenError::ForeignOwner { found, expected }) => {
            assert_eq!((found, expected), (mine, mine + 1));
        }
        other => panic!("{other:?}"),
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_missing_directory_is_not_found() {
    let dir = scratch_dir("od-missing");
    match open_dir(&dir) {
        Err(SafeOpenError::Io(err)) => assert_eq!(err.kind(), ErrorKind::NotFound),
        other => panic!("{other:?}"),
    }
}
