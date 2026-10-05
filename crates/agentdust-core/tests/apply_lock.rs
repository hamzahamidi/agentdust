mod scratch;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;

use agentdust_core::apply::lock::{IdentityLock, LockError, LockPoint, LockProbe};
use scratch::TempDir;

fn locks(dir: &TempDir) -> PathBuf {
    dir.join("locks")
}

fn names(dir: &Path) -> Vec<String> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[test]
fn the_first_taker_gets_the_lock_and_a_second_is_told_it_is_held() {
    let dir = TempDir::private("lock-two");
    let first = IdentityLock::try_acquire(&locks(&dir), "p42-1a").unwrap();
    assert!(first.is_some());
    let second = IdentityLock::try_acquire(&locks(&dir), "p42-1a").unwrap();
    assert!(second.is_none());
}

#[test]
fn locks_of_different_identities_do_not_meet() {
    let dir = TempDir::private("lock-names");
    let first = IdentityLock::try_acquire(&locks(&dir), "p42-1a").unwrap();
    let second = IdentityLock::try_acquire(&locks(&dir), "p43-1a").unwrap();
    assert!(first.is_some() && second.is_some());
}

#[test]
fn the_lock_directory_and_file_are_private() {
    let dir = TempDir::private("lock-modes");
    let held = IdentityLock::try_acquire(&locks(&dir), "p42-1a")
        .unwrap()
        .unwrap();
    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode(&locks(&dir)), 0o700);
    assert_eq!(mode(held.path()), 0o600);
    assert_eq!(names(&locks(&dir)), ["p42-1a.lock"]);
}

#[test]
fn releasing_the_lock_removes_its_file_and_lets_the_next_taker_in() {
    let dir = TempDir::private("lock-release");
    let first = IdentityLock::try_acquire(&locks(&dir), "p42-1a")
        .unwrap()
        .unwrap();
    let path = first.path().to_path_buf();
    drop(first);
    assert!(!path.exists());
    assert!(names(&locks(&dir)).is_empty());
    assert!(
        IdentityLock::try_acquire(&locks(&dir), "p42-1a")
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_file_left_by_a_crashed_holder_does_not_block_anyone() {
    let dir = TempDir::private("lock-stale");
    fs::create_dir(locks(&dir)).unwrap();
    fs::set_permissions(locks(&dir), fs::Permissions::from_mode(0o700)).unwrap();
    let stale = locks(&dir).join("p42-1a.lock");
    fs::write(&stale, b"").unwrap();
    fs::set_permissions(&stale, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        IdentityLock::try_acquire(&locks(&dir), "p42-1a")
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_name_that_could_leave_the_directory_is_refused() {
    let dir = TempDir::private("lock-name");
    for name in [
        "", ".", "..", "../x", "a/b", "p42 1a", "p42-1a\n", ".hidden", "a\0b",
    ] {
        assert!(
            matches!(
                IdentityLock::try_acquire(&locks(&dir), name),
                Err(LockError::InvalidName)
            ),
            "{name:?}"
        );
    }
    let long = "a".repeat(65);
    assert!(matches!(
        IdentityLock::try_acquire(&locks(&dir), &long),
        Err(LockError::InvalidName)
    ));
    assert!(!locks(&dir).exists());
}

#[test]
fn unsafe_state_in_the_lock_directory_is_refused() {
    let dir = TempDir::private("lock-unsafe");

    let real = TempDir::private("lock-unsafe-real");
    symlink(real.path(), locks(&dir)).unwrap();
    assert!(matches!(
        IdentityLock::try_acquire(&locks(&dir), "p1-1"),
        Err(LockError::Refused(_))
    ));
    fs::remove_file(locks(&dir)).unwrap();

    fs::create_dir(locks(&dir)).unwrap();
    fs::set_permissions(locks(&dir), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        IdentityLock::try_acquire(&locks(&dir), "p1-1"),
        Err(LockError::Refused(_))
    ));
    fs::set_permissions(locks(&dir), fs::Permissions::from_mode(0o700)).unwrap();

    let target = real.join("elsewhere");
    fs::write(&target, b"").unwrap();
    symlink(&target, locks(&dir).join("p2-2.lock")).unwrap();
    assert!(matches!(
        IdentityLock::try_acquire(&locks(&dir), "p2-2"),
        Err(LockError::Refused(_))
    ));

    let first = locks(&dir).join("p3-3.lock");
    fs::write(&first, b"").unwrap();
    fs::set_permissions(&first, fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(&first, locks(&dir).join("other")).unwrap();
    assert!(matches!(
        IdentityLock::try_acquire(&locks(&dir), "p3-3"),
        Err(LockError::Refused(_))
    ));
}

struct ReplaceOnce {
    path: PathBuf,
    done: AtomicBool,
    opened: AtomicUsize,
}

impl LockProbe for ReplaceOnce {
    fn reached(&self, point: LockPoint) {
        let LockPoint::Opened = point;
        self.opened.fetch_add(1, Ordering::SeqCst);
        if !self.done.swap(true, Ordering::SeqCst) {
            fs::remove_file(&self.path).unwrap();
            fs::write(&self.path, b"").unwrap();
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
}

#[test]
fn a_file_replaced_between_open_and_lock_does_not_give_two_holders() {
    let dir = TempDir::private("lock-replace");
    let path = locks(&dir).join("p42-1a.lock");
    let probe = ReplaceOnce {
        path: path.clone(),
        done: AtomicBool::new(false),
        opened: AtomicUsize::new(0),
    };
    let first = IdentityLock::try_acquire_with(&locks(&dir), "p42-1a", &probe)
        .unwrap()
        .expect("the retry locks the file that is now at the path");
    assert_eq!(probe.opened.load(Ordering::SeqCst), 2);
    assert!(
        IdentityLock::try_acquire(&locks(&dir), "p42-1a")
            .unwrap()
            .is_none()
    );
    drop(first);
    assert!(
        IdentityLock::try_acquire(&locks(&dir), "p42-1a")
            .unwrap()
            .is_some()
    );
}

#[test]
fn taking_and_releasing_in_a_loop_never_gives_two_holders() {
    let dir = TempDir::private("lock-loop");
    let directory = locks(&dir);
    let holders = AtomicUsize::new(0);
    let acquired = AtomicUsize::new(0);
    thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                for _ in 0..150 {
                    if let Some(lock) = IdentityLock::try_acquire(&directory, "p9-9").unwrap() {
                        acquired.fetch_add(1, Ordering::SeqCst);
                        assert_eq!(holders.fetch_add(1, Ordering::SeqCst), 0);
                        thread::yield_now();
                        holders.fetch_sub(1, Ordering::SeqCst);
                        drop(lock);
                    }
                }
            });
        }
    });
    assert!(acquired.load(Ordering::SeqCst) > 0);
    assert!(names(&directory).is_empty());
}
