mod scratch;
mod secret_support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Condvar, Mutex};
use std::thread;
use std::time::Duration;

use agentdust_core::safe_open::SafeOpenError;
use agentdust_core::secret::{
    InstallPoint, InstallProbe, NoProbe, SECRET_FILE, SecretError, load_or_create_with,
};
use scratch::{TempDir, private_dir};
use secret_support::{apfs, mode_of, names, only_the_secret, place};

const HANG_GUARD: Duration = Duration::from_secs(60);
const CALLERS: usize = 16;

struct At<F> {
    point: InstallPoint,
    action: F,
}

impl<F: Fn() + Sync> InstallProbe for At<F> {
    fn reached(&self, point: InstallPoint) {
        if point == self.point {
            (self.action)();
        }
    }
}

fn at<F: Fn() + Sync>(point: InstallPoint, action: F) -> At<F> {
    At { point, action }
}

fn temp_files(dir: &Path) -> Vec<String> {
    names(dir)
        .into_iter()
        .filter(|name| name.starts_with("install.secret.") && name.ends_with(".tmp"))
        .collect()
}

struct Rendezvous {
    arrived: Mutex<usize>,
    changed: Condvar,
    parties: usize,
}

impl Rendezvous {
    fn new(parties: usize) -> Self {
        Self {
            arrived: Mutex::new(0),
            changed: Condvar::new(),
            parties,
        }
    }

    fn wait(&self) {
        let mut arrived = self.arrived.lock().unwrap();
        *arrived += 1;
        self.changed.notify_all();
        let (arrived, timeout) = self
            .changed
            .wait_timeout_while(arrived, HANG_GUARD, |arrived| *arrived < self.parties)
            .unwrap();
        assert!(
            !timeout.timed_out(),
            "only {} of {} callers arrived",
            *arrived,
            self.parties
        );
    }
}

struct Pause {
    reached: Mutex<Sender<()>>,
    resume: Mutex<Receiver<()>>,
}

impl InstallProbe for Pause {
    fn reached(&self, point: InstallPoint) {
        if point == InstallPoint::TempSynced {
            self.reached.lock().unwrap().send(()).unwrap();
            self.resume.lock().unwrap().recv_timeout(HANG_GUARD).unwrap();
        }
    }
}

#[test]
fn a_file_planted_between_the_temp_file_and_the_rename_is_never_replaced() {
    let dir = TempDir::private("secret-gap-planted");
    let probe = at(InstallPoint::TempSynced, || {
        place(&dir, &[0x42; 32], 0o600);
    });
    let secret = load_or_create_with(&dir, apfs(), &probe).unwrap();
    assert_eq!(secret.as_bytes(), &[0x42; 32]);
    assert_eq!(fs::read(dir.join(SECRET_FILE)).unwrap(), vec![0x42; 32]);
    assert_eq!(names(&dir), only_the_secret());
}

#[test]
fn an_unusable_file_planted_in_that_gap_is_reported_and_left_alone() {
    for (name, content, mode) in [
        ("short", vec![0x42u8; 5], 0o600),
        ("long", vec![0x42u8; 40], 0o600),
        ("loose", vec![0x42u8; 32], 0o644),
    ] {
        let dir = TempDir::private("secret-gap-unusable");
        let probe = at(InstallPoint::TempSynced, || {
            place(&dir, &content, mode);
        });
        let result = load_or_create_with(&dir, apfs(), &probe);
        match (&result, name) {
            (Err(SecretError::WrongSize { found }), "short" | "long") => {
                assert_eq!(*found, content.len() as u64);
            }
            (Err(SecretError::Refused(SafeOpenError::LooseMode { .. })), "loose") => {}
            _ => panic!("{name}: {result:?}"),
        }
        assert_eq!(fs::read(dir.join(SECRET_FILE)).unwrap(), content, "{name}");
        assert_eq!(mode_of(&dir.join(SECRET_FILE)), mode, "{name}");
        assert_eq!(names(&dir), only_the_secret(), "{name}");
    }
}

#[test]
fn the_secret_is_absent_until_a_whole_synced_file_is_renamed_into_place() {
    let dir = TempDir::private("secret-gap-visible");
    let seen = AtomicUsize::new(0);
    let probe = at(InstallPoint::TempSynced, || {
        seen.fetch_add(1, Ordering::Relaxed);
        assert!(fs::symlink_metadata(dir.join(SECRET_FILE)).is_err());
        let temps = temp_files(&dir);
        assert_eq!(temps.len(), 1, "{temps:?}");
        let temp = dir.join(&temps[0]);
        assert_eq!(fs::metadata(&temp).unwrap().len(), 32);
        assert_eq!(mode_of(&temp), 0o600);
        assert_eq!(names(&dir).len(), 1);
    });
    let secret = load_or_create_with(&dir, apfs(), &probe).unwrap();
    assert_eq!(seen.load(Ordering::Relaxed), 1);
    assert_eq!(fs::read(dir.join(SECRET_FILE)).unwrap(), secret.as_bytes());
    assert_eq!(names(&dir), only_the_secret());
}

#[test]
fn the_probe_is_not_reached_when_the_secret_exists() {
    let dir = TempDir::private("secret-gap-existing");
    place(&dir, &[7u8; 32], 0o600);
    let reached = AtomicUsize::new(0);
    struct Count<'a>(&'a AtomicUsize);
    impl InstallProbe for Count<'_> {
        fn reached(&self, _point: InstallPoint) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    load_or_create_with(&dir, apfs(), &Count(&reached)).unwrap();
    assert_eq!(reached.load(Ordering::Relaxed), 0);
}

#[test]
fn a_stray_temp_file_is_not_mistaken_for_the_secret() {
    let dir = private_dir("secret-stray");
    let stray = dir.join("install.secret.0123456789abcdef.tmp");
    fs::write(&stray, [0x55u8; 32]).unwrap();
    fs::set_permissions(&stray, fs::Permissions::from_mode(0o600)).unwrap();
    let secret = load_or_create_with(&dir, apfs(), &NoProbe).unwrap();
    assert_ne!(secret.as_bytes(), &[0x55u8; 32]);
    assert_eq!(fs::read(dir.join(SECRET_FILE)).unwrap(), secret.as_bytes());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_caller_paused_after_its_temp_file_returns_the_secret_of_the_one_that_finished() {
    let dir = TempDir::private("secret-paused");
    let (reached_tx, reached_rx) = channel();
    let (resume_tx, resume_rx) = channel();
    let pause = Pause {
        reached: Mutex::new(reached_tx),
        resume: Mutex::new(resume_rx),
    };
    thread::scope(|scope| {
        let paused = scope.spawn(|| *load_or_create_with(&dir, apfs(), &pause).unwrap().as_bytes());
        reached_rx.recv_timeout(HANG_GUARD).unwrap();
        assert_eq!(temp_files(&dir).len(), 1);
        let finished = *load_or_create_with(&dir, apfs(), &NoProbe).unwrap().as_bytes();
        assert_eq!(names(&dir).len(), 2);
        resume_tx.send(()).unwrap();
        let resumed = paused.join().unwrap();
        assert_eq!(resumed, finished);
        assert_eq!(fs::read(dir.join(SECRET_FILE)).unwrap(), finished.to_vec());
    });
    assert_eq!(names(&dir), only_the_secret());
}

fn converge(point: InstallPoint, rounds: usize) {
    for round in 0..rounds {
        let dir = TempDir::private("secret-converge");
        let first = Rendezvous::new(CALLERS);
        let second = Rendezvous::new(CALLERS);
        let temps_seen = Mutex::new(Vec::new());
        let probe = at(point, || {
            first.wait();
            temps_seen.lock().unwrap().push(temp_files(&dir).len());
            second.wait();
        });
        let secrets: Vec<[u8; 32]> = thread::scope(|scope| {
            let handles: Vec<_> = (0..CALLERS)
                .map(|_| scope.spawn(|| *load_or_create_with(&dir, apfs(), &probe).unwrap().as_bytes()))
                .collect();
            handles.into_iter().map(|handle| handle.join().unwrap()).collect()
        });
        let stored = fs::read(dir.join(SECRET_FILE)).unwrap();
        for secret in &secrets {
            assert_eq!(secret.as_slice(), stored.as_slice(), "round {round}");
        }
        assert_eq!(temps_seen.lock().unwrap().len(), CALLERS, "round {round}");
        if point == InstallPoint::TempSynced {
            assert!(
                temps_seen.lock().unwrap().iter().all(|count| *count == CALLERS),
                "{:?}",
                temps_seen.lock().unwrap()
            );
        }
        assert_eq!(names(&dir), only_the_secret(), "round {round}");
    }
}

#[test]
fn callers_that_all_found_the_secret_missing_converge_on_one_secret() {
    converge(InstallPoint::Absent, 5);
}

#[test]
fn callers_that_all_wrote_a_temp_file_converge_on_the_first_rename() {
    converge(InstallPoint::TempSynced, 5);
}
