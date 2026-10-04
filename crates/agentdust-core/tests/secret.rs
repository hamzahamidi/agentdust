mod scratch;
mod secret_support;

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::sync::{Arc, Barrier};
use std::thread;

use agentdust_core::safe_open::SafeOpenError;
use agentdust_core::secret::{SECRET_FILE, SECRET_LEN, Secret, SecretError};
use scratch::{make_fifo, private_dir, returns_promptly, scratch_dir};
use secret_support::{load_or_create, mode_of, names, only_the_secret, place};

fn refused(result: Result<Secret, SecretError>) -> SafeOpenError {
    match result {
        Err(SecretError::Refused(err)) => err,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn the_secret_file_is_named_install_secret_and_holds_32_bytes() {
    assert_eq!(SECRET_FILE, "install.secret");
    assert_eq!(SECRET_LEN, 32);
}

#[test]
fn a_missing_data_directory_is_created_with_the_secret() {
    let dir = scratch_dir("secret-create");
    let secret = load_or_create(&dir).unwrap();
    let path = dir.join(SECRET_FILE);
    assert_eq!(mode_of(&dir), 0o700);
    assert_eq!(mode_of(&path), 0o600);
    assert_eq!(fs::metadata(&path).unwrap().len(), 32);
    assert_eq!(fs::read(&path).unwrap(), secret.as_bytes());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_data_directory_below_a_missing_parent_gets_no_secret_and_creates_nothing() {
    let root = private_dir("secret-no-parent");
    let result = load_or_create(&root.join("missing/data"));
    match result {
        Err(SecretError::Io(err)) => assert_eq!(err.kind(), std::io::ErrorKind::NotFound),
        other => panic!("{other:?}"),
    }
    assert!(names(&root).is_empty());
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn an_existing_private_directory_gets_the_secret() {
    let dir = private_dir("secret-existing-dir");
    let secret = load_or_create(&dir).unwrap();
    assert_eq!(fs::read(dir.join(SECRET_FILE)).unwrap(), secret.as_bytes());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_secret_is_owned_by_the_current_user_with_one_link() {
    let dir = scratch_dir("secret-owner");
    load_or_create(&dir).unwrap();
    let metadata = fs::metadata(dir.join(SECRET_FILE)).unwrap();
    assert_eq!(metadata.uid(), fs::metadata(&dir).unwrap().uid());
    assert_eq!(metadata.nlink(), 1);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_second_call_returns_the_same_secret_and_does_not_touch_the_file() {
    let dir = scratch_dir("secret-again");
    let first = load_or_create(&dir).unwrap();
    let path = dir.join(SECRET_FILE);
    let before = fs::metadata(&path).unwrap();
    let second = load_or_create(&dir).unwrap();
    let after = fs::metadata(&path).unwrap();
    assert_eq!(first.as_bytes(), second.as_bytes());
    assert_eq!(before.ino(), after.ino());
    assert_eq!(before.mtime_nsec(), after.mtime_nsec());
    assert_eq!(before.mtime(), after.mtime());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn separate_installs_draw_distinct_non_constant_secrets() {
    let secrets: Vec<[u8; 32]> = (0..8)
        .map(|i| {
            let dir = scratch_dir(&format!("secret-random-{i}"));
            let secret = load_or_create(&dir).unwrap();
            fs::remove_dir_all(&dir).unwrap();
            *secret.as_bytes()
        })
        .collect();
    for (i, secret) in secrets.iter().enumerate() {
        assert!(secret.iter().any(|byte| *byte != secret[0]), "{secret:?}");
        for other in &secrets[i + 1..] {
            assert_ne!(secret, other);
        }
    }
}

#[test]
fn no_temporary_file_is_left_beside_the_secret() {
    let dir = scratch_dir("secret-no-temp");
    load_or_create(&dir).unwrap();
    load_or_create(&dir).unwrap();
    assert_eq!(names(&dir), only_the_secret());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_wrong_size_is_an_error_and_the_file_is_never_regenerated() {
    for size in [0usize, 1, 31, 33, 64, 4096] {
        let dir = private_dir(&format!("secret-size-{size}"));
        let content = vec![7u8; size];
        let path = place(&dir, &content, 0o600);
        let result = load_or_create(&dir);
        assert!(
            matches!(result, Err(SecretError::WrongSize { found }) if found == size as u64),
            "{size}: {result:?}"
        );
        assert_eq!(fs::read(&path).unwrap(), content, "{size}");
        assert_eq!(names(&dir), only_the_secret(), "{size}");
        fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn a_loose_mode_is_an_error_and_the_file_is_left_as_it_was() {
    for mode in [0o644, 0o640, 0o604, 0o660, 0o666, 0o700, 0o777, 0o4600] {
        let dir = private_dir(&format!("secret-mode-{mode:o}"));
        let content = vec![9u8; 32];
        let path = place(&dir, &content, mode);
        let err = refused(load_or_create(&dir));
        assert!(
            matches!(err, SafeOpenError::LooseMode { .. }),
            "{mode:o}: {err:?}"
        );
        assert_eq!(fs::read(&path).unwrap(), content, "{mode:o}");
        assert_eq!(mode_of(&path), mode, "{mode:o}");
        assert_eq!(names(&dir), only_the_secret(), "{mode:o}");
        fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn a_read_only_secret_is_accepted() {
    let dir = private_dir("secret-read-only");
    let content = vec![5u8; 32];
    place(&dir, &content, 0o400);
    assert_eq!(load_or_create(&dir).unwrap().as_bytes(), content.as_slice());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_symlinked_secret_is_refused_and_its_target_is_untouched() {
    let dir = private_dir("secret-symlink");
    let target = dir.join("elsewhere");
    fs::write(&target, vec![3u8; 32]).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&target, dir.join(SECRET_FILE)).unwrap();
    let err = refused(load_or_create(&dir));
    assert!(matches!(err, SafeOpenError::Symlink), "{err:?}");
    assert_eq!(fs::read(&target).unwrap(), vec![3u8; 32]);
    assert!(fs::symlink_metadata(dir.join(SECRET_FILE)).unwrap().is_symlink());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_dangling_symlink_is_refused_and_creates_nothing_at_its_target() {
    let dir = private_dir("secret-dangling");
    let target = dir.join("not-yet");
    symlink(&target, dir.join(SECRET_FILE)).unwrap();
    let err = refused(load_or_create(&dir));
    assert!(matches!(err, SafeOpenError::Symlink), "{err:?}");
    assert!(!target.exists());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_fifo_in_place_of_the_secret_is_refused_at_once() {
    let dir = private_dir("secret-fifo");
    make_fifo(&dir.join(SECRET_FILE));
    let scan = dir.clone();
    let result = returns_promptly(move || load_or_create(&scan));
    assert!(matches!(refused(result), SafeOpenError::NotRegular));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_directory_in_place_of_the_secret_is_refused() {
    let dir = private_dir("secret-dir-in-place");
    fs::create_dir(dir.join(SECRET_FILE)).unwrap();
    let err = refused(load_or_create(&dir));
    assert!(matches!(err, SafeOpenError::NotRegular), "{err:?}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_hard_linked_secret_is_refused() {
    let dir = private_dir("secret-hardlink");
    let path = place(&dir, &[1u8; 32], 0o600);
    fs::hard_link(&path, dir.join("copy")).unwrap();
    let err = refused(load_or_create(&dir));
    assert!(matches!(err, SafeOpenError::HardLinked { links: 2 }), "{err:?}");
    assert_eq!(fs::read(&path).unwrap(), vec![1u8; 32]);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_data_directory_with_a_loose_mode_gets_no_secret() {
    let dir = private_dir("secret-loose-dir");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    let err = refused(load_or_create(&dir));
    assert!(matches!(err, SafeOpenError::LooseMode { .. }), "{err:?}");
    assert!(names(&dir).is_empty());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_symlinked_data_directory_gets_no_secret() {
    let root = private_dir("secret-symlinked-dir");
    let real = root.join("real");
    fs::create_dir(&real).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
    let link = root.join("link");
    symlink(&real, &link).unwrap();
    let err = refused(load_or_create(&link));
    assert!(matches!(err, SafeOpenError::Symlink), "{err:?}");
    assert!(names(&real).is_empty());
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_data_directory_that_is_a_file_is_refused() {
    let root = private_dir("secret-dir-is-file");
    let path = root.join("data");
    fs::write(&path, b"x").unwrap();
    let err = refused(load_or_create(&path));
    assert!(matches!(err, SafeOpenError::NotDirectory), "{err:?}");
    assert_eq!(fs::read(&path).unwrap(), b"x");
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn an_unwritable_data_directory_is_an_error_and_creates_nothing() {
    let dir = private_dir("secret-unwritable");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
    let result = load_or_create(&dir);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(result, Err(SecretError::Io(_))), "{result:?}");
    assert!(names(&dir).is_empty());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn threads_creating_the_secret_together_all_use_one_secret() {
    for round in 0..10 {
        let dir = scratch_dir(&format!("secret-race-{round}"));
        let barrier = Arc::new(Barrier::new(16));
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let dir = dir.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    *load_or_create(&dir).unwrap().as_bytes()
                })
            })
            .collect();
        let secrets: Vec<[u8; 32]> = handles.into_iter().map(|handle| handle.join().unwrap()).collect();
        let stored = fs::read(dir.join(SECRET_FILE)).unwrap();
        for secret in &secrets {
            assert_eq!(secret.as_slice(), stored.as_slice(), "round {round}");
        }
        assert_eq!(names(&dir), only_the_secret(), "round {round}");
        fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn debug_output_never_shows_the_secret_bytes() {
    let secret = Secret::from_bytes([0x7a; 32]);
    let shown = format!("{secret:?}");
    assert!(!shown.contains("122"), "{shown}");
    assert!(!shown.contains("7a"), "{shown}");
    assert!(!shown.contains('z'), "{shown}");
    assert_eq!(secret.as_bytes(), &[0x7a; 32]);
}
