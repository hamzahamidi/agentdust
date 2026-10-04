#![cfg(target_os = "macos")]

use std::path::Path;
use std::process::Command;

use agentdust_core::darwin::{self, DarwinProvider};
use agentdust_core::provider::{ProcessProvider, ProcessRead};
use agentdust_core::revalidate::{Revalidation, revalidate};
use agentdust_testkit::Fixture;

const SLEEPER: &str = env!("CARGO_BIN_EXE_fixture-sleeper");

fn spawn_sleeper() -> Fixture {
    Fixture(Command::new(SLEEPER).arg("30").spawn().unwrap())
}

#[test]
fn a_live_fixture_revalidates_match_twice() {
    let provider = DarwinProvider::new().unwrap();
    let fixture = spawn_sleeper();
    let pid = fixture.0.id() as i32;
    let ProcessRead::Present(identity) = provider.read(pid).unwrap() else {
        panic!("the fixture should be readable");
    };
    assert_eq!(identity.kernel.pid, pid);
    let ProcessRead::Present(own) = provider.read(std::process::id() as i32).unwrap() else {
        panic!("this test process should be readable");
    };
    assert_eq!(identity.kernel.uid, own.kernel.uid);
    assert_eq!(
        identity.kernel.boot_session_uuid,
        darwin::boot_session_uuid().unwrap()
    );
    assert_eq!(
        identity.evidence.exe_path.canonicalize().unwrap(),
        Path::new(SLEEPER).canonicalize().unwrap()
    );
    assert_eq!(revalidate(&identity, &provider), Revalidation::Match);
    assert_eq!(revalidate(&identity, &provider), Revalidation::Match);
}

#[test]
fn a_fixture_that_has_exited_reads_gone() {
    let provider = DarwinProvider::new().unwrap();
    let mut fixture = spawn_sleeper();
    let pid = fixture.0.id() as i32;
    let ProcessRead::Present(identity) = provider.read(pid).unwrap() else {
        panic!("the fixture should be readable");
    };
    fixture.0.kill().unwrap();
    fixture.0.wait().unwrap();
    assert_eq!(provider.read(pid).unwrap(), ProcessRead::Gone);
    assert_eq!(revalidate(&identity, &provider), Revalidation::Gone);
}
