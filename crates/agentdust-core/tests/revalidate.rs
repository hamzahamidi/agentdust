use std::io::ErrorKind;
use std::path::PathBuf;

use agentdust_core::identity::ProcessIdentity;
use agentdust_core::provider::ProcessRead;
use agentdust_core::revalidate::{Field, Revalidation, revalidate};

mod common;
use common::{ScriptedProvider, changed, identity, kernel};

const PID: i32 = 4242;

type Edit = fn(&mut ProcessIdentity);

fn one_read(read: ProcessRead) -> ScriptedProvider {
    ScriptedProvider::new().then(PID, read)
}

#[test]
fn an_identical_fresh_read_matches() {
    let expected = identity(PID);
    let provider = one_read(ProcessRead::Present(expected.clone()));
    assert_eq!(revalidate(&expected, &provider), Revalidation::Match);
}

#[test]
fn a_process_that_is_gone_reads_gone() {
    let provider = one_read(ProcessRead::Gone);
    assert_eq!(revalidate(&identity(PID), &provider), Revalidation::Gone);
}

#[test]
fn an_unscripted_pid_reads_gone() {
    assert_eq!(
        revalidate(&identity(PID), &ScriptedProvider::new()),
        Revalidation::Gone
    );
}

#[test]
fn an_unreadable_path_with_the_same_kernel_identity_is_unreadable() {
    let provider = one_read(ProcessRead::PathUnreadable(kernel(PID)));
    assert_eq!(revalidate(&identity(PID), &provider), Revalidation::Unreadable);
}

#[test]
fn an_unreadable_path_never_hides_a_changed_kernel_identity() {
    let mut reused = kernel(PID);
    reused.start_time_us += 1;
    let provider = one_read(ProcessRead::PathUnreadable(reused));
    assert_eq!(
        revalidate(&identity(PID), &provider),
        Revalidation::Changed(Field::StartTime)
    );
}

#[test]
fn a_failed_read_is_unreadable() {
    for kind in [
        ErrorKind::PermissionDenied,
        ErrorKind::NotFound,
        ErrorKind::InvalidData,
    ] {
        let provider = ScriptedProvider::new().then_fail(PID, kind);
        assert_eq!(
            revalidate(&identity(PID), &provider),
            Revalidation::Unreadable,
            "{kind:?}"
        );
    }
}

#[test]
fn a_new_start_time_on_the_same_pid_is_changed() {
    let expected = identity(PID);
    let reused = changed(&expected, |id| id.kernel.start_time_us += 1);
    let provider = one_read(ProcessRead::Present(reused));
    assert_eq!(
        revalidate(&expected, &provider),
        Revalidation::Changed(Field::StartTime)
    );
}

#[test]
fn a_new_uid_on_the_same_pid_is_changed() {
    let expected = identity(PID);
    let reused = changed(&expected, |id| id.kernel.uid = 0);
    let provider = one_read(ProcessRead::Present(reused));
    assert_eq!(
        revalidate(&expected, &provider),
        Revalidation::Changed(Field::Uid)
    );
}

#[test]
fn a_new_executable_path_on_the_same_pid_is_changed() {
    let expected = identity(PID);
    let reused = changed(&expected, |id| id.evidence.exe_path = PathBuf::from("/bin/sh"));
    let provider = one_read(ProcessRead::Present(reused));
    assert_eq!(
        revalidate(&expected, &provider),
        Revalidation::Changed(Field::ExePath)
    );
}

#[test]
fn a_path_differing_only_by_a_trailing_slash_is_changed() {
    let expected = identity(PID);
    let reused = changed(&expected, |id| {
        id.evidence.exe_path = PathBuf::from("/usr/local/bin/node/")
    });
    let provider = one_read(ProcessRead::Present(reused));
    assert_eq!(
        revalidate(&expected, &provider),
        Revalidation::Changed(Field::ExePath)
    );
}

#[test]
fn a_new_boot_session_is_changed_even_when_every_other_field_is_equal() {
    let expected = identity(PID);
    let rebooted = changed(&expected, |id| {
        id.kernel.boot_session_uuid = "99999999-2222-3333-4444-555555555555".to_owned()
    });
    let provider = one_read(ProcessRead::Present(rebooted));
    assert_eq!(
        revalidate(&expected, &provider),
        Revalidation::Changed(Field::BootSession)
    );
}

#[test]
fn a_new_boot_session_wins_over_every_other_difference() {
    let expected = identity(PID);
    let other = changed(&expected, |id| {
        id.kernel.boot_session_uuid = String::new();
        id.kernel.pid += 1;
        id.kernel.start_time_us += 1;
        id.kernel.uid += 1;
        id.evidence.exe_path = PathBuf::from("/bin/sh");
    });
    let provider = one_read(ProcessRead::Present(other.clone()));
    assert_eq!(
        revalidate(&expected, &provider),
        Revalidation::Changed(Field::BootSession)
    );
    let unreadable = one_read(ProcessRead::PathUnreadable(other.kernel));
    assert_eq!(
        revalidate(&expected, &unreadable),
        Revalidation::Changed(Field::BootSession)
    );
}

#[test]
fn the_first_differing_field_is_named_in_a_fixed_order() {
    let expected = identity(PID);
    let steps: [(Field, Edit); 4] = [
        (Field::Pid, |id| id.kernel.pid += 1),
        (Field::StartTime, |id| id.kernel.start_time_us += 1),
        (Field::Uid, |id| id.kernel.uid += 1),
        (Field::ExePath, |id| {
            id.evidence.exe_path = PathBuf::from("/bin/sh")
        }),
    ];
    for skip in 0..steps.len() {
        let fresh = changed(&expected, |id| {
            steps[skip..].iter().for_each(|(_, edit)| edit(id))
        });
        let provider = one_read(ProcessRead::Present(fresh));
        assert_eq!(
            revalidate(&expected, &provider),
            Revalidation::Changed(steps[skip].0)
        );
    }
}

#[test]
fn a_read_for_another_pid_is_changed() {
    let provider = one_read(ProcessRead::Present(identity(PID + 1)));
    assert_eq!(
        revalidate(&identity(PID), &provider),
        Revalidation::Changed(Field::Pid)
    );
}

#[test]
fn only_one_fresh_read_of_the_expected_pid_is_taken() {
    let expected = identity(PID);
    let provider = one_read(ProcessRead::Present(expected.clone()));
    revalidate(&expected, &provider);
    assert_eq!(provider.reads(PID), 1);
}

#[test]
fn a_non_positive_pid_is_never_read_and_never_matches() {
    for pid in [0, -1, i32::MIN] {
        let expected = identity(pid);
        let provider = ScriptedProvider::new().then(pid, ProcessRead::Present(expected.clone()));
        assert_eq!(
            revalidate(&expected, &provider),
            Revalidation::Unreadable,
            "pid {pid}"
        );
        assert_eq!(provider.reads(pid), 0, "pid {pid}");
    }
}

#[test]
fn pid_reuse_with_a_new_start_time_is_caught_on_the_second_check() {
    let original = identity(PID);
    let reused = changed(&original, |id| id.kernel.start_time_us += 5_000_000);
    let provider = ScriptedProvider::new()
        .then(PID, ProcessRead::Present(original.clone()))
        .then(PID, ProcessRead::Present(reused));
    assert_eq!(revalidate(&original, &provider), Revalidation::Match);
    assert_eq!(
        revalidate(&original, &provider),
        Revalidation::Changed(Field::StartTime)
    );
}

#[test]
fn pid_reuse_by_another_user_is_caught_on_the_second_check() {
    let original = identity(PID);
    let reused = changed(&original, |id| id.kernel.uid = 0);
    let provider = ScriptedProvider::new()
        .then(PID, ProcessRead::Present(original.clone()))
        .then(PID, ProcessRead::Present(reused));
    assert_eq!(revalidate(&original, &provider), Revalidation::Match);
    assert_eq!(
        revalidate(&original, &provider),
        Revalidation::Changed(Field::Uid)
    );
}

#[test]
fn pid_reuse_by_another_executable_is_caught_on_the_second_check() {
    let original = identity(PID);
    let reused = changed(&original, |id| id.evidence.exe_path = PathBuf::from("/bin/sleep"));
    let provider = ScriptedProvider::new()
        .then(PID, ProcessRead::Present(original.clone()))
        .then(PID, ProcessRead::Present(reused));
    assert_eq!(revalidate(&original, &provider), Revalidation::Match);
    assert_eq!(
        revalidate(&original, &provider),
        Revalidation::Changed(Field::ExePath)
    );
}

#[test]
fn a_process_that_exits_and_restarts_with_the_same_binary_is_not_the_same_process() {
    let original = identity(PID);
    let restarted = changed(&original, |id| id.kernel.start_time_us += 3_000_000);
    let provider = ScriptedProvider::new()
        .then(PID, ProcessRead::Present(original.clone()))
        .then(PID, ProcessRead::Gone)
        .then(PID, ProcessRead::Present(restarted));
    assert_eq!(revalidate(&original, &provider), Revalidation::Match);
    assert_eq!(revalidate(&original, &provider), Revalidation::Gone);
    assert_eq!(
        revalidate(&original, &provider),
        Revalidation::Changed(Field::StartTime)
    );
}

#[test]
fn a_path_that_becomes_unreadable_between_checks_stops_matching() {
    let original = identity(PID);
    let provider = ScriptedProvider::new()
        .then(PID, ProcessRead::Present(original.clone()))
        .then(PID, ProcessRead::PathUnreadable(original.kernel.clone()));
    assert_eq!(revalidate(&original, &provider), Revalidation::Match);
    assert_eq!(revalidate(&original, &provider), Revalidation::Unreadable);
}
