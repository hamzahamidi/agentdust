use std::cell::{Cell, RefCell};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use agentdust_testkit::wait::{Timer, wait_until_on};
use agentdust_testkit::wait_until;

const HANG_GUARD: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(10);

struct Simulated {
    start: Instant,
    passed: Cell<Duration>,
    sleeps: RefCell<Vec<Duration>>,
}

impl Simulated {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            passed: Cell::new(Duration::ZERO),
            sleeps: RefCell::new(Vec::new()),
        }
    }

    fn sleeps(&self) -> Vec<Duration> {
        self.sleeps.borrow().clone()
    }
}

impl Timer for Simulated {
    fn now(&self) -> Instant {
        self.start + self.passed.get()
    }

    fn sleep(&self, duration: Duration) {
        self.passed.set(self.passed.get() + duration);
        self.sleeps.borrow_mut().push(duration);
    }
}

fn counting(calls: &Cell<u32>, holds_from: u32) -> impl FnMut() -> bool + '_ {
    move || {
        calls.set(calls.get() + 1);
        calls.get() >= holds_from
    }
}

#[test]
fn a_predicate_that_is_already_true_is_evaluated_once_and_nothing_sleeps() {
    let timer = Simulated::new();
    let calls = Cell::new(0);
    assert!(wait_until_on(
        &timer,
        counting(&calls, 1),
        Duration::from_secs(10)
    ));
    assert_eq!(calls.get(), 1);
    assert_eq!(timer.sleeps(), Vec::<Duration>::new());
}

#[test]
fn a_predicate_that_never_holds_returns_false_when_the_timeout_has_passed() {
    let timer = Simulated::new();
    let calls = Cell::new(0);
    assert!(!wait_until_on(
        &timer,
        counting(&calls, u32::MAX),
        Duration::from_millis(300)
    ));
    assert_eq!(calls.get(), 31);
    assert_eq!(timer.sleeps(), vec![POLL; 30]);
    assert_eq!(timer.passed.get(), Duration::from_millis(300));
}

#[test]
fn the_last_sleep_is_cut_to_the_time_that_remains() {
    let timer = Simulated::new();
    let calls = Cell::new(0);
    assert!(!wait_until_on(
        &timer,
        counting(&calls, u32::MAX),
        Duration::from_millis(25)
    ));
    assert_eq!(calls.get(), 4);
    assert_eq!(
        timer.sleeps(),
        vec![POLL, POLL, Duration::from_millis(5)],
        "the wait must not run past its deadline"
    );
}

#[test]
fn a_timeout_shorter_than_one_poll_sleeps_only_that_long() {
    let timer = Simulated::new();
    let calls = Cell::new(0);
    assert!(!wait_until_on(
        &timer,
        counting(&calls, u32::MAX),
        Duration::from_millis(3)
    ));
    assert_eq!(calls.get(), 2);
    assert_eq!(timer.sleeps(), vec![Duration::from_millis(3)]);
}

#[test]
fn a_zero_timeout_still_evaluates_the_predicate_once() {
    let timer = Simulated::new();
    let calls = Cell::new(0);
    assert!(!wait_until_on(&timer, counting(&calls, u32::MAX), Duration::ZERO));
    assert_eq!(calls.get(), 1);
    assert_eq!(timer.sleeps(), Vec::<Duration>::new());
    assert!(wait_until_on(&timer, || true, Duration::ZERO));
}

#[test]
fn a_predicate_that_becomes_true_later_returns_true_without_using_up_the_timeout() {
    let timer = Simulated::new();
    let calls = Cell::new(0);
    assert!(wait_until_on(&timer, counting(&calls, 4), HANG_GUARD));
    assert_eq!(calls.get(), 4);
    assert_eq!(timer.sleeps(), vec![POLL; 3]);
}

#[test]
fn the_predicate_is_evaluated_after_every_sleep_and_never_between() {
    let timer = Simulated::new();
    let seen = RefCell::new(Vec::new());
    let holds = wait_until_on(
        &timer,
        || {
            seen.borrow_mut().push(timer.sleeps().len());
            false
        },
        Duration::from_millis(50),
    );
    assert!(!holds);
    assert_eq!(*seen.borrow(), vec![0, 1, 2, 3, 4, 5]);
}

#[test]
fn the_system_clock_returns_true_for_a_condition_another_thread_sets() {
    let flag = Arc::new(AtomicBool::new(false));
    let setter = {
        let flag = Arc::clone(&flag);
        thread::spawn(move || flag.store(true, Ordering::SeqCst))
    };
    assert!(wait_until(|| flag.load(Ordering::SeqCst), HANG_GUARD));
    setter.join().unwrap();
}

#[test]
fn the_system_clock_returns_false_for_a_condition_that_never_holds() {
    assert!(!wait_until(|| false, Duration::from_millis(50)));
    assert!(wait_until(|| true, Duration::ZERO));
}
