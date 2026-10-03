use agentdust_core::clock::{monotonic_ns, wall_ms};

#[test]
fn monotonic_clock_never_goes_backwards() {
    let first = monotonic_ns();
    let second = monotonic_ns();
    assert!(first > 0);
    assert!(second >= first);
}

#[test]
fn wall_clock_is_after_2026() {
    assert!(wall_ms() > 1_767_225_600_000);
}
