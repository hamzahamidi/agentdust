use std::time::Duration;

fn main() {
    let seconds = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(30);
    std::thread::sleep(Duration::from_secs(seconds));
}
