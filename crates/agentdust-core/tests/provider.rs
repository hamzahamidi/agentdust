use std::io::ErrorKind;
use std::path::PathBuf;

use agentdust_core::provider::{ProcessProvider, ProcessRead};

mod common;
use common::{ScriptedProvider, identity, kernel};

fn node() -> Option<PathBuf> {
    Some(PathBuf::from("/usr/local/bin/node"))
}

#[test]
fn a_pid_that_was_never_found_reads_gone() {
    assert_eq!(ProcessRead::from_samples(None, node(), None), ProcessRead::Gone);
    assert_eq!(
        ProcessRead::from_samples(None, None, Some(kernel(7))),
        ProcessRead::Gone
    );
}

#[test]
fn a_process_that_exits_during_the_read_reads_gone() {
    assert_eq!(
        ProcessRead::from_samples(Some(kernel(7)), node(), None),
        ProcessRead::Gone
    );
    assert_eq!(
        ProcessRead::from_samples(Some(kernel(7)), None, None),
        ProcessRead::Gone
    );
}

#[test]
fn two_matching_samples_and_a_path_make_a_present_identity() {
    assert_eq!(
        ProcessRead::from_samples(Some(kernel(7)), node(), Some(kernel(7))),
        ProcessRead::Present(identity(7))
    );
}

#[test]
fn a_missing_path_leaves_the_kernel_identity_readable() {
    assert_eq!(
        ProcessRead::from_samples(Some(kernel(7)), None, Some(kernel(7))),
        ProcessRead::PathUnreadable(kernel(7))
    );
}

#[test]
fn a_pid_reused_during_the_read_is_never_present_even_with_the_same_path() {
    let mut reused = kernel(7);
    reused.start_time_us += 1;
    assert_eq!(
        ProcessRead::from_samples(Some(kernel(7)), node(), Some(reused.clone())),
        ProcessRead::PathUnreadable(reused)
    );
}

#[test]
fn a_script_plays_in_order_and_then_repeats_its_last_read() {
    let provider = ScriptedProvider::new()
        .then(7, ProcessRead::Present(identity(7)))
        .then(7, ProcessRead::Gone);
    assert_eq!(provider.read(7).unwrap(), ProcessRead::Present(identity(7)));
    assert_eq!(provider.read(7).unwrap(), ProcessRead::Gone);
    assert_eq!(provider.read(7).unwrap(), ProcessRead::Gone);
    assert_eq!(provider.reads(7), 3);
}

#[test]
fn scripts_are_independent_per_pid_and_an_unscripted_pid_is_gone() {
    let provider = ScriptedProvider::new().then(7, ProcessRead::Present(identity(7)));
    assert_eq!(provider.read(8).unwrap(), ProcessRead::Gone);
    assert_eq!(provider.read(7).unwrap(), ProcessRead::Present(identity(7)));
    assert_eq!(provider.reads(7), 1);
    assert_eq!(provider.reads(8), 1);
    assert_eq!(provider.reads(9), 0);
}

#[test]
fn a_failing_step_surfaces_its_error_kind() {
    let provider = ScriptedProvider::new().then_fail(7, ErrorKind::PermissionDenied);
    assert_eq!(provider.read(7).unwrap_err().kind(), ErrorKind::PermissionDenied);
}
