use std::process::Child;

pub mod report;
pub mod spec;
pub mod wait;

pub use wait::wait_until;

pub struct Fixture(pub Child);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
