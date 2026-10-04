use crate::identity::{KernelIdentity, ProcessIdentity};
use crate::provider::{ProcessProvider, ProcessRead};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    BootSession,
    Pid,
    StartTime,
    Uid,
    ExePath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Revalidation {
    Match,
    Gone,
    Changed(Field),
    Unreadable,
}

pub fn revalidate<P: ProcessProvider + ?Sized>(expected: &ProcessIdentity, provider: &P) -> Revalidation {
    if expected.kernel.pid <= 0 {
        return Revalidation::Unreadable;
    }
    match provider.read(expected.kernel.pid) {
        Err(_) => Revalidation::Unreadable,
        Ok(ProcessRead::Gone) => Revalidation::Gone,
        Ok(ProcessRead::PathUnreadable(kernel)) => kernel_difference(&expected.kernel, &kernel)
            .map_or(Revalidation::Unreadable, Revalidation::Changed),
        Ok(ProcessRead::Present(fresh)) => kernel_difference(&expected.kernel, &fresh.kernel)
            .or_else(|| (expected.evidence != fresh.evidence).then_some(Field::ExePath))
            .map_or(Revalidation::Match, Revalidation::Changed),
    }
}

fn kernel_difference(expected: &KernelIdentity, fresh: &KernelIdentity) -> Option<Field> {
    let KernelIdentity {
        boot_session_uuid,
        pid,
        start_time_us,
        uid,
    } = expected;
    if *boot_session_uuid != fresh.boot_session_uuid {
        Some(Field::BootSession)
    } else if *pid != fresh.pid {
        Some(Field::Pid)
    } else if *start_time_us != fresh.start_time_us {
        Some(Field::StartTime)
    } else if *uid != fresh.uid {
        Some(Field::Uid)
    } else {
        None
    }
}
