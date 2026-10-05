use std::thread;
use std::time::{Duration, Instant};

pub trait Timer: Send + Sync {
    fn now(&self) -> Duration;

    fn sleep(&self, duration: Duration);
}

pub struct SystemTimer {
    origin: Instant,
}

impl SystemTimer {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemTimer {
    fn default() -> Self {
        Self::new()
    }
}

impl Timer for SystemTimer {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }

    fn sleep(&self, duration: Duration) {
        thread::sleep(duration);
    }
}
