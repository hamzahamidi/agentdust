mod scratch;

use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt, symlink};
use std::path::Path;

use agentdust_core::safe_open::{
    Access, SafeOpenError, check_dir, check_dir_as, ensure_dir, open_file, open_file_as,
};
use scratch::{make_fifo, private_dir, returns_promptly, scratch_dir};

fn file_with(dir: &Path, name: &str, mode: u32, content: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    fs::write(&path, content).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    path
}

fn mode_of(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

fn owner_of(path: &Path) -> u32 {
    fs::metadata(path).unwrap().uid()
}

#[test]
fn a_private_regular_file_opens_for_reading() {
    let dir = private_dir("so-read");
    let path = file_with(&dir, "state", 0o600, b"hello");
    let mut text = String::new();
    open_file(&path, Access::Read)
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "hello");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_read_only_file_still_opens_for_reading() {
    let dir = private_dir("so-read-only");
    let path = file_with(&dir, "state", 0o400, b"x");
    assert!(open_file(&path, Access::Read).is_ok());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn append_adds_to_the_end_of_an_existing_file() {
    let dir = private_dir("so-append");
    let path = file_with(&dir, "state", 0o600, b"a");
    open_file(&path, Access::Append).unwrap().write_all(b"b").unwrap();
    open_file(&path, Access::Append).unwrap().write_all(b"c").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"abc");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn append_does_not_create_a_missing_file() {
    let dir = private_dir("so-append-missing");
    let path = dir.join("state");
    match open_file(&path, Access::Append) {
        Err(SafeOpenError::Io(err)) => assert_eq!(err.kind(), ErrorKind::NotFound),
        other => panic!("{other:?}"),
    }
    assert!(!path.exists());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn reading_a_missing_file_is_not_found() {
    let dir = private_dir("so-read-missing");
    match open_file(&dir.join("state"), Access::Read) {
        Err(SafeOpenError::Io(err)) => assert_eq!(err.kind(), ErrorKind::NotFound),
        other => panic!("{other:?}"),
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn create_makes_a_new_file_with_mode_0600() {
    let dir = private_dir("so-create");
    let path = dir.join("secret");
    let mut file = open_file(&path, Access::Create).unwrap();
    file.write_all(b"data").unwrap();
    assert_eq!(mode_of(&path), 0o600);
    assert_eq!(fs::read(&path).unwrap(), b"data");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn create_refuses_an_existing_file_and_leaves_it_alone() {
    let dir = private_dir("so-create-exists");
    let path = file_with(&dir, "secret", 0o600, b"keep");
    match open_file(&path, Access::Create) {
        Err(SafeOpenError::Io(err)) => assert_eq!(err.kind(), ErrorKind::AlreadyExists),
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read(&path).unwrap(), b"keep");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn create_never_writes_through_a_symlink() {
    let dir = private_dir("so-create-symlink");
    let missing_target = dir.join("target-missing");
    symlink(&missing_target, dir.join("dangling")).unwrap();
    assert!(open_file(&dir.join("dangling"), Access::Create).is_err());
    assert!(!missing_target.exists());
    let existing = file_with(&dir, "target-existing", 0o600, b"keep");
    symlink(&existing, dir.join("pointing")).unwrap();
    assert!(open_file(&dir.join("pointing"), Access::Create).is_err());
    assert_eq!(fs::read(&existing).unwrap(), b"keep");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_symlink_is_refused_for_every_access() {
    let dir = private_dir("so-symlink");
    let target = file_with(&dir, "target", 0o600, b"keep");
    let link = dir.join("link");
    symlink(&target, &link).unwrap();
    for access in [Access::Read, Access::Append] {
        assert!(matches!(open_file(&link, access), Err(SafeOpenError::Symlink)));
    }
    assert_eq!(fs::read(&target).unwrap(), b"keep");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_dangling_symlink_is_a_symlink_and_not_a_missing_file() {
    let dir = private_dir("so-dangling");
    let link = dir.join("link");
    symlink(dir.join("nowhere"), &link).unwrap();
    assert!(matches!(
        open_file(&link, Access::Read),
        Err(SafeOpenError::Symlink)
    ));
    assert!(matches!(
        open_file(&link, Access::Append),
        Err(SafeOpenError::Symlink)
    ));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_fifo_is_refused_for_reading_without_blocking() {
    let dir = private_dir("so-fifo-read");
    let path = dir.join("pipe");
    make_fifo(&path);
    let result = returns_promptly(move || open_file(&path, Access::Read));
    assert!(matches!(result, Err(SafeOpenError::NotRegular)), "{result:?}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_fifo_with_no_reader_is_refused_for_appending_without_blocking() {
    let dir = private_dir("so-fifo-append");
    let path = dir.join("pipe");
    make_fifo(&path);
    let result = returns_promptly(move || open_file(&path, Access::Append));
    assert!(matches!(result, Err(SafeOpenError::NotRegular)), "{result:?}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_fifo_with_a_reader_is_refused_for_appending_and_receives_nothing() {
    let dir = private_dir("so-fifo-reader");
    let path = dir.join("pipe");
    make_fifo(&path);
    let mut reader = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&path)
        .unwrap();
    let target = path.clone();
    let result = returns_promptly(move || open_file(&target, Access::Append));
    assert!(matches!(result, Err(SafeOpenError::NotRegular)), "{result:?}");
    let mut received = Vec::new();
    let _ = reader.read_to_end(&mut received);
    assert!(received.is_empty());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_directory_is_not_a_regular_file() {
    let dir = private_dir("so-directory");
    for access in [Access::Read, Access::Append] {
        assert!(matches!(open_file(&dir, access), Err(SafeOpenError::NotRegular)));
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_hard_linked_file_is_refused() {
    let dir = private_dir("so-hardlink");
    let path = file_with(&dir, "state", 0o600, b"keep");
    fs::hard_link(&path, dir.join("second-name")).unwrap();
    for access in [Access::Read, Access::Append] {
        match open_file(&path, access) {
            Err(SafeOpenError::HardLinked { links }) => assert_eq!(links, 2),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(fs::read(&path).unwrap(), b"keep");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_file_owned_by_another_uid_is_refused() {
    let dir = private_dir("so-owner");
    let path = file_with(&dir, "state", 0o600, b"keep");
    let mine = owner_of(&path);
    assert!(open_file_as(&path, Access::Read, mine).is_ok());
    for access in [Access::Read, Access::Append] {
        match open_file_as(&path, access, mine + 1) {
            Err(SafeOpenError::ForeignOwner { found, expected }) => {
                assert_eq!((found, expected), (mine, mine + 1));
            }
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(fs::read(&path).unwrap(), b"keep");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_default_owner_is_the_current_user() {
    let dir = private_dir("so-default-owner");
    let path = file_with(&dir, "state", 0o600, b"x");
    assert!(open_file(&path, Access::Read).is_ok());
    assert!(open_file_as(&path, Access::Read, owner_of(&path) + 1).is_err());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_mode_looser_than_0600_is_refused() {
    let dir = private_dir("so-modes");
    for mode in [
        0o601, 0o602, 0o604, 0o610, 0o620, 0o640, 0o660, 0o644, 0o666, 0o700, 0o777,
    ] {
        let path = file_with(&dir, &format!("state-{mode:o}"), mode, b"keep");
        for access in [Access::Read, Access::Append] {
            match open_file(&path, access) {
                Err(SafeOpenError::LooseMode { mode: found, allowed }) => {
                    assert_eq!((found, allowed), (mode, 0o600), "{mode:o}");
                }
                other => panic!("{mode:o}: {other:?}"),
            }
        }
        assert_eq!(fs::read(&path).unwrap(), b"keep");
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_refused_append_writes_nothing() {
    let dir = private_dir("so-refused-append");
    let path = file_with(&dir, "state", 0o644, b"keep");
    assert!(open_file(&path, Access::Append).is_err());
    let size = fs::metadata(&path).unwrap().len();
    assert_eq!(size, 4);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_private_directory_passes() {
    let dir = private_dir("so-dir-ok");
    assert!(check_dir(&dir).is_ok());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_directory_mode_looser_than_0700_is_refused() {
    let dir = private_dir("so-dir-modes");
    for mode in [0o701, 0o710, 0o750, 0o755, 0o770, 0o777] {
        let sub = dir.join(format!("d-{mode:o}"));
        fs::create_dir(&sub).unwrap();
        fs::set_permissions(&sub, fs::Permissions::from_mode(mode)).unwrap();
        match check_dir(&sub) {
            Err(SafeOpenError::LooseMode { mode: found, allowed }) => {
                assert_eq!((found, allowed), (mode, 0o700), "{mode:o}");
            }
            other => panic!("{mode:o}: {other:?}"),
        }
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_symlink_to_a_directory_is_refused() {
    let dir = private_dir("so-dir-symlink");
    let real = dir.join("real");
    fs::DirBuilder::new().mode(0o700).create(&real).unwrap();
    symlink(&real, dir.join("link")).unwrap();
    assert!(matches!(
        check_dir(&dir.join("link")),
        Err(SafeOpenError::Symlink)
    ));
    assert!(matches!(
        ensure_dir(&dir.join("link")),
        Err(SafeOpenError::Symlink)
    ));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_regular_file_is_not_a_directory() {
    let dir = private_dir("so-dir-file");
    let path = file_with(&dir, "state", 0o600, b"x");
    assert!(matches!(check_dir(&path), Err(SafeOpenError::NotDirectory)));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_fifo_is_not_a_directory_and_does_not_block() {
    let dir = private_dir("so-dir-fifo");
    let path = dir.join("pipe");
    make_fifo(&path);
    let result = returns_promptly(move || check_dir(&path));
    assert!(matches!(result, Err(SafeOpenError::NotDirectory)), "{result:?}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_directory_owned_by_another_uid_is_refused() {
    let dir = private_dir("so-dir-owner");
    let mine = owner_of(&dir);
    assert!(check_dir_as(&dir, mine).is_ok());
    match check_dir_as(&dir, mine + 1) {
        Err(SafeOpenError::ForeignOwner { found, expected }) => {
            assert_eq!((found, expected), (mine, mine + 1))
        }
        other => panic!("{other:?}"),
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_missing_directory_is_not_found() {
    let dir = scratch_dir("so-dir-missing");
    match check_dir(&dir) {
        Err(SafeOpenError::Io(err)) => assert_eq!(err.kind(), ErrorKind::NotFound),
        other => panic!("{other:?}"),
    }
}

#[test]
fn ensure_dir_creates_a_missing_final_component_with_mode_0700() {
    let root = private_dir("so-ensure");
    let leaf = root.join("data");
    ensure_dir(&leaf).unwrap();
    assert_eq!(mode_of(&leaf), 0o700);
    ensure_dir(&leaf).unwrap();
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn ensure_dir_below_a_missing_parent_fails_and_creates_nothing() {
    let root = private_dir("so-ensure-no-parent");
    for leaf in [root.join("missing/data"), root.join("a/b/data")] {
        match ensure_dir(&leaf) {
            Err(SafeOpenError::Io(err)) => assert_eq!(err.kind(), ErrorKind::NotFound, "{leaf:?}"),
            other => panic!("{leaf:?}: {other:?}"),
        }
    }
    assert!(fs::read_dir(&root).unwrap().next().is_none());
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn ensure_dir_refuses_a_loose_directory_and_leaves_its_mode() {
    let root = private_dir("so-ensure-loose");
    let loose = root.join("loose");
    fs::create_dir(&loose).unwrap();
    fs::set_permissions(&loose, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(ensure_dir(&loose), Err(SafeOpenError::LooseMode { .. })));
    assert_eq!(mode_of(&loose), 0o755);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn ensure_dir_refuses_a_file_in_the_way() {
    let root = private_dir("so-ensure-file");
    let path = file_with(&root, "state", 0o600, b"x");
    assert!(ensure_dir(&path).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"x");
    fs::remove_dir_all(&root).unwrap();
}
