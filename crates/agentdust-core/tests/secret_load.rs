mod scratch;
mod secret_support;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

use agentdust_core::safe_open::SafeOpenError;
use agentdust_core::secret::{SECRET_FILE, SecretError, load_existing};
use scratch::{private_dir, scratch_dir};
use secret_support::{load_or_create, names, only_the_secret, place};

fn io_kind(result: Result<agentdust_core::secret::Secret, SecretError>) -> std::io::ErrorKind {
    match result {
        Err(SecretError::Io(err)) => err.kind(),
        other => panic!("{other:?}"),
    }
}

fn refusal(result: Result<agentdust_core::secret::Secret, SecretError>) -> SafeOpenError {
    match result {
        Err(SecretError::Refused(err)) => err,
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_existing_secret_loads_with_the_bytes_that_were_installed() {
    let dir = private_dir("load-existing");
    let created = load_or_create(&dir).unwrap();
    let loaded = load_existing(&dir).unwrap();
    assert_eq!(loaded.as_bytes(), created.as_bytes());
    assert_eq!(names(&dir), only_the_secret());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_missing_secret_is_not_created() {
    let dir = private_dir("load-missing");
    assert_eq!(io_kind(load_existing(&dir)), std::io::ErrorKind::NotFound);
    assert!(names(&dir).is_empty());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_missing_directory_is_not_created() {
    let dir = scratch_dir("load-no-dir");
    assert_eq!(io_kind(load_existing(&dir)), std::io::ErrorKind::NotFound);
    assert!(!dir.exists());
}

#[test]
fn a_secret_of_the_wrong_size_is_refused_and_left_alone() {
    let dir = private_dir("load-size");
    let path = place(&dir, &[1; 31], 0o600);
    match load_existing(&dir) {
        Err(SecretError::WrongSize { found: 31 }) => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read(path).unwrap(), vec![1; 31]);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_loose_mode_on_the_secret_is_refused_and_not_corrected() {
    let dir = private_dir("load-mode");
    let path = place(&dir, &[1; 32], 0o644);
    assert!(matches!(
        refusal(load_existing(&dir)),
        SafeOpenError::LooseMode { .. }
    ));
    assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o7777, 0o644);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_loose_mode_on_the_directory_is_refused_and_not_corrected() {
    let dir = private_dir("load-dir-mode");
    place(&dir, &[1; 32], 0o600);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        refusal(load_existing(&dir)),
        SafeOpenError::LooseMode { .. }
    ));
    assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o7777, 0o755);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_symlinked_secret_is_refused() {
    let dir = private_dir("load-symlink");
    let target = dir.join("elsewhere");
    fs::write(&target, [1u8; 32]).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&target, dir.join(SECRET_FILE)).unwrap();
    assert!(matches!(refusal(load_existing(&dir)), SafeOpenError::Symlink));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_hard_linked_secret_is_refused() {
    let dir = private_dir("load-hardlink");
    let path = place(&dir, &[1; 32], 0o600);
    fs::hard_link(&path, dir.join("copy")).unwrap();
    assert!(matches!(
        refusal(load_existing(&dir)),
        SafeOpenError::HardLinked { .. }
    ));
    fs::remove_dir_all(&dir).unwrap();
}
