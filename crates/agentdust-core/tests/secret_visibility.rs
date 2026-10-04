mod scratch;
mod secret_support;

use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use agentdust_core::secret::SECRET_FILE;
use scratch::{HANG_GUARD, private_dir};
use secret_support::load_or_create;

const MIN_CYCLES: usize = 150;
const MIN_SIGHTINGS: usize = 20;

#[test]
fn a_poller_never_sees_the_secret_half_written() {
    let dir = private_dir("secret-visibility");
    let path = dir.join(SECRET_FILE);
    let stop = Arc::new(AtomicBool::new(false));
    let partial = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(AtomicUsize::new(0));
    let poller = {
        let (path, stop, partial, seen) = (
            path.clone(),
            Arc::clone(&stop),
            Arc::clone(&partial),
            Arc::clone(&seen),
        );
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if let Ok(metadata) = fs::metadata(&path) {
                    seen.fetch_add(1, Ordering::Relaxed);
                    if metadata.len() != 32 {
                        partial.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        })
    };
    let guard = Instant::now() + HANG_GUARD;
    let mut cycles = 0;
    while (cycles < MIN_CYCLES || seen.load(Ordering::Relaxed) < MIN_SIGHTINGS) && Instant::now() < guard {
        let _ = fs::remove_file(&path);
        load_or_create(&dir).unwrap();
        thread::sleep(Duration::from_millis(1));
        cycles += 1;
    }
    stop.store(true, Ordering::Relaxed);
    poller.join().unwrap();
    assert!(
        seen.load(Ordering::Relaxed) >= MIN_SIGHTINGS,
        "the poller saw the file {} times",
        seen.load(Ordering::Relaxed)
    );
    assert_eq!(partial.load(Ordering::Relaxed), 0);
    fs::remove_dir_all(&dir).unwrap();
}
