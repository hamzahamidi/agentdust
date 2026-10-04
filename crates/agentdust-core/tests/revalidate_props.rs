use std::ffi::OsString;
use std::io::ErrorKind;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use agentdust_core::identity::{IdentityEvidence, KernelIdentity, ProcessIdentity};
use agentdust_core::provider::ProcessRead;
use agentdust_core::revalidate::{Field, Revalidation, revalidate};
use proptest::prelude::*;

mod common;
use common::ScriptedProvider;

const FIELDS: [Field; 5] = [
    Field::BootSession,
    Field::Pid,
    Field::StartTime,
    Field::Uid,
    Field::ExePath,
];

fn exe_path() -> impl Strategy<Value = PathBuf> {
    prop::collection::vec(any::<u8>(), 0..48).prop_map(|bytes| PathBuf::from(OsString::from_vec(bytes)))
}

fn kernel(pid: impl Strategy<Value = i32>) -> impl Strategy<Value = KernelIdentity> {
    (any::<String>(), pid, any::<u64>(), any::<u32>()).prop_map(
        |(boot_session_uuid, pid, start_time_us, uid)| KernelIdentity {
            boot_session_uuid,
            pid,
            start_time_us,
            uid,
        },
    )
}

fn identity(pid: impl Strategy<Value = i32>) -> impl Strategy<Value = ProcessIdentity> {
    (kernel(pid), exe_path()).prop_map(|(kernel, exe_path)| ProcessIdentity {
        kernel,
        evidence: IdentityEvidence { exe_path },
    })
}

fn live_pid() -> impl Strategy<Value = i32> {
    1..=i32::MAX
}

fn any_pid() -> impl Strategy<Value = i32> {
    prop_oneof![any::<i32>(), Just(0), Just(-1), Just(i32::MIN), Just(i32::MAX)]
}

fn non_positive_pid() -> impl Strategy<Value = i32> {
    prop_oneof![i32::MIN..=0, Just(0), Just(-1), Just(i32::MIN)]
}

fn related() -> impl Strategy<Value = (ProcessIdentity, ProcessIdentity)> {
    (
        identity(live_pid()),
        identity(live_pid()),
        prop::array::uniform5(any::<bool>()),
    )
        .prop_map(|(expected, other, take)| {
            let mut fresh = expected.clone();
            if take[0] {
                fresh.kernel.boot_session_uuid = other.kernel.boot_session_uuid;
            }
            if take[1] {
                fresh.kernel.pid = other.kernel.pid;
            }
            if take[2] {
                fresh.kernel.start_time_us = other.kernel.start_time_us;
            }
            if take[3] {
                fresh.kernel.uid = other.kernel.uid;
            }
            if take[4] {
                fresh.evidence.exe_path = other.evidence.exe_path;
            }
            (expected, fresh)
        })
}

fn differs(field: Field, a: &ProcessIdentity, b: &ProcessIdentity) -> bool {
    match field {
        Field::BootSession => a.kernel.boot_session_uuid != b.kernel.boot_session_uuid,
        Field::Pid => a.kernel.pid != b.kernel.pid,
        Field::StartTime => a.kernel.start_time_us != b.kernel.start_time_us,
        Field::Uid => a.kernel.uid != b.kernel.uid,
        Field::ExePath => a.evidence.exe_path.as_os_str() != b.evidence.exe_path.as_os_str(),
    }
}

fn mutate(field: Field, identity: &ProcessIdentity) -> ProcessIdentity {
    let mut copy = identity.clone();
    match field {
        Field::BootSession => copy.kernel.boot_session_uuid.push('x'),
        Field::Pid => copy.kernel.pid = copy.kernel.pid.wrapping_add(1),
        Field::StartTime => copy.kernel.start_time_us = copy.kernel.start_time_us.wrapping_add(1),
        Field::Uid => copy.kernel.uid = copy.kernel.uid.wrapping_add(1),
        Field::ExePath => {
            let mut bytes = copy.evidence.exe_path.into_os_string().into_vec();
            bytes.push(b'x');
            copy.evidence.exe_path = PathBuf::from(OsString::from_vec(bytes));
        }
    }
    copy
}

#[derive(Debug, Clone)]
enum Scripted {
    Present(ProcessIdentity),
    Gone,
    PathUnreadable(KernelIdentity),
    Fail,
}

fn scripted() -> impl Strategy<Value = Scripted> {
    prop_oneof![
        identity(any_pid()).prop_map(Scripted::Present),
        Just(Scripted::Gone),
        kernel(any_pid()).prop_map(Scripted::PathUnreadable),
        Just(Scripted::Fail),
    ]
}

fn provider_for(pid: i32, steps: &[Scripted]) -> ScriptedProvider {
    steps
        .iter()
        .cloned()
        .fold(ScriptedProvider::new(), |provider, step| match step {
            Scripted::Present(identity) => provider.then(pid, ProcessRead::Present(identity)),
            Scripted::Gone => provider.then(pid, ProcessRead::Gone),
            Scripted::PathUnreadable(kernel) => provider.then(pid, ProcessRead::PathUnreadable(kernel)),
            Scripted::Fail => provider.then_fail(pid, ErrorKind::PermissionDenied),
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn match_exactly_when_every_field_is_equal((expected, fresh) in related()) {
        let provider = ScriptedProvider::new().then(expected.kernel.pid, ProcessRead::Present(fresh.clone()));
        let result = revalidate(&expected, &provider);
        prop_assert_eq!(result == Revalidation::Match, expected == fresh);
    }

    #[test]
    fn a_difference_names_the_first_differing_field_in_order((expected, fresh) in related()) {
        let provider = ScriptedProvider::new().then(expected.kernel.pid, ProcessRead::Present(fresh.clone()));
        let first = FIELDS.into_iter().find(|field| differs(*field, &expected, &fresh));
        let want = first.map_or(Revalidation::Match, Revalidation::Changed);
        prop_assert_eq!(revalidate(&expected, &provider), want);
    }

    #[test]
    fn mutating_exactly_one_field_names_that_field(
        expected in identity(live_pid()),
        index in 0..FIELDS.len(),
    ) {
        let field = FIELDS[index];
        let fresh = mutate(field, &expected);
        let provider = ScriptedProvider::new().then(expected.kernel.pid, ProcessRead::Present(fresh));
        prop_assert_eq!(revalidate(&expected, &provider), Revalidation::Changed(field));
    }

    #[test]
    fn a_different_boot_session_invalidates_everything(
        (expected, fresh) in related(),
        rebooted in any::<String>(),
        unreadable in any::<bool>(),
    ) {
        prop_assume!(rebooted != expected.kernel.boot_session_uuid);
        let mut fresh = fresh;
        fresh.kernel.boot_session_uuid = rebooted;
        let read = if unreadable {
            ProcessRead::PathUnreadable(fresh.kernel)
        } else {
            ProcessRead::Present(fresh)
        };
        let provider = ScriptedProvider::new().then(expected.kernel.pid, read);
        prop_assert_eq!(revalidate(&expected, &provider), Revalidation::Changed(Field::BootSession));
    }

    #[test]
    fn a_provider_reporting_gone_gives_gone(expected in identity(live_pid())) {
        let provider = ScriptedProvider::new().then(expected.kernel.pid, ProcessRead::Gone);
        prop_assert_eq!(revalidate(&expected, &provider), Revalidation::Gone);
    }

    #[test]
    fn an_unreadable_path_with_an_equal_kernel_identity_gives_unreadable(expected in identity(live_pid())) {
        let provider = ScriptedProvider::new()
            .then(expected.kernel.pid, ProcessRead::PathUnreadable(expected.kernel.clone()));
        prop_assert_eq!(revalidate(&expected, &provider), Revalidation::Unreadable);
    }

    #[test]
    fn an_unreadable_path_never_matches(expected in identity(live_pid()), fresh_kernel in kernel(live_pid())) {
        let provider = ScriptedProvider::new().then(expected.kernel.pid, ProcessRead::PathUnreadable(fresh_kernel));
        prop_assert_ne!(revalidate(&expected, &provider), Revalidation::Match);
    }

    #[test]
    fn a_failed_read_gives_unreadable(expected in identity(live_pid())) {
        let provider = ScriptedProvider::new().then_fail(expected.kernel.pid, ErrorKind::PermissionDenied);
        prop_assert_eq!(revalidate(&expected, &provider), Revalidation::Unreadable);
    }

    #[test]
    fn a_non_positive_pid_never_matches_and_is_never_read(
        expected in identity(non_positive_pid()),
        step in scripted(),
    ) {
        let provider = provider_for(expected.kernel.pid, &[step, Scripted::Present(expected.clone())]);
        prop_assert_eq!(revalidate(&expected, &provider), Revalidation::Unreadable);
        prop_assert_eq!(provider.reads(expected.kernel.pid), 0);
    }

    #[test]
    fn one_fresh_read_of_the_expected_pid_is_taken(
        expected in identity(live_pid()),
        step in scripted(),
    ) {
        let provider = provider_for(expected.kernel.pid, &[step]);
        revalidate(&expected, &provider);
        prop_assert_eq!(provider.reads(expected.kernel.pid), 1);
    }

    #[test]
    fn revalidation_is_deterministic_and_never_panics_for_arbitrary_input(
        expected in identity(any_pid()),
        steps in prop::collection::vec(scripted(), 0..4),
    ) {
        let first = provider_for(expected.kernel.pid, &steps);
        let second = provider_for(expected.kernel.pid, &steps);
        for _ in 0..steps.len().max(1) {
            prop_assert_eq!(revalidate(&expected, &first), revalidate(&expected, &second));
        }
    }
}
