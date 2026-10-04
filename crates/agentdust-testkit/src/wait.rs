use std::thread;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(10);

pub trait Timer {
    fn now(&self) -> Instant;
    fn sleep(&self, duration: Duration);
}

pub struct SystemTimer;

impl Timer for SystemTimer {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, duration: Duration) {
        thread::sleep(duration);
    }
}

pub fn wait_until(predicate: impl FnMut() -> bool, timeout: Duration) -> bool {
    wait_until_on(&SystemTimer, predicate, timeout)
}

pub fn wait_until_on(timer: &impl Timer, mut predicate: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = timer.now() + timeout;
    loop {
        if predicate() {
            return true;
        }
        let now = timer.now();
        if now >= deadline {
            return false;
        }
        timer.sleep(POLL.min(deadline - now));
    }
}
