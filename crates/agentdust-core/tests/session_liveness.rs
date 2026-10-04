mod common;

use std::io::ErrorKind;
use std::path::PathBuf;

use agentdust_core::identity::KernelIdentity;
use agentdust_core::provider::ProcessRead;
use agentdust_core::session::{Liveness, LivenessProbe, ProviderLiveness};
use common::{ScriptedProvider, changed, identity, kernel};

const PID: i32 = 4242;

fn probe(provider: &ScriptedProvider) -> Liveness {
    ProviderLiveness(provider).probe(&kernel(PID))
}

fn with(edit: impl FnOnce(&mut KernelIdentity)) -> KernelIdentity {
    let mut copy = kernel(PID);
    edit(&mut copy);
    copy
}

#[test]
fn the_same_kernel_identity_is_alive() {
    let provider = ScriptedProvider::new().then(PID, ProcessRead::Present(identity(PID)));
    assert_eq!(probe(&provider), Liveness::Alive);
}

#[test]
fn a_process_that_is_gone_is_gone() {
    assert_eq!(
        probe(&ScriptedProvider::new().then(PID, ProcessRead::Gone)),
        Liveness::Gone
    );
    assert_eq!(probe(&ScriptedProvider::new()), Liveness::Gone);
}

#[test]
fn a_pid_taken_by_another_process_is_gone() {
    let edits: [fn(&mut KernelIdentity); 4] = [
        |k| k.start_time_us += 1,
        |k| k.uid += 1,
        |k| k.boot_session_uuid = "another-boot".to_owned(),
        |k| k.start_time_us -= 1,
    ];
    for edit in edits {
        let fresh = with(edit);
        let present = changed(&identity(PID), |id| id.kernel = fresh.clone());
        assert_eq!(
            probe(&ScriptedProvider::new().then(PID, ProcessRead::Present(present))),
            Liveness::Gone,
            "{fresh:?}"
        );
        assert_eq!(
            probe(&ScriptedProvider::new().then(PID, ProcessRead::PathUnreadable(fresh.clone()))),
            Liveness::Gone,
            "{fresh:?}"
        );
    }
}

#[test]
fn a_new_executable_path_on_the_same_process_is_still_alive() {
    let exec = changed(&identity(PID), |id| {
        id.evidence.exe_path = PathBuf::from("/bin/sh")
    });
    let provider = ScriptedProvider::new().then(PID, ProcessRead::Present(exec));
    assert_eq!(probe(&provider), Liveness::Alive);
}

#[test]
fn an_unreadable_path_with_the_same_kernel_identity_is_still_alive() {
    let provider = ScriptedProvider::new().then(PID, ProcessRead::PathUnreadable(kernel(PID)));
    assert_eq!(probe(&provider), Liveness::Alive);
}

#[test]
fn a_failed_read_is_unknown_and_never_gone() {
    for kind in [
        ErrorKind::PermissionDenied,
        ErrorKind::NotFound,
        ErrorKind::InvalidData,
    ] {
        let provider = ScriptedProvider::new().then_fail(PID, kind);
        assert_eq!(probe(&provider), Liveness::Unknown, "{kind:?}");
    }
}

#[test]
fn a_pid_that_cannot_name_a_process_is_unknown_and_is_not_read() {
    let provider = ScriptedProvider::new();
    for pid in [0, -1] {
        assert_eq!(ProviderLiveness(&provider).probe(&kernel(pid)), Liveness::Unknown);
        assert_eq!(provider.reads(pid), 0);
    }
}

#[test]
fn a_probe_reads_the_process_once() {
    let provider = ScriptedProvider::new().then(PID, ProcessRead::Present(identity(PID)));
    probe(&provider);
    assert_eq!(provider.reads(PID), 1);
}
