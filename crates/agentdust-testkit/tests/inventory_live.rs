#![cfg(target_os = "macos")]

use std::path::Path;
use std::time::Duration;

use agentdust_core::darwin::{DarwinLive, DarwinProvider, DarwinSource, tick_frequency, ticks_to_ns};
use agentdust_core::identity::KernelIdentity;
use agentdust_core::inventory::{LaunchctlList, LaunchdSource, LiveDetails, ProcessSource, RawProcess, Tag};
use agentdust_core::provider::{ProcessProvider, ProcessRead};
use agentdust_core::secret::Secret;
use agentdust_core::tag::SessionTag;
use agentdust_testkit::harness::Harness;
use agentdust_testkit::spec::ProcSpec;
use agentdust_testkit::wait_until;

const FIXTURE: &str = env!("CARGO_BIN_EXE_fixture-sleeper");
const HANG_GUARD: Duration = Duration::from_secs(60);

fn harness() -> Harness {
    Harness::new(FIXTURE).unwrap()
}

fn long() -> ProcSpec {
    ProcSpec::new().seconds(120)
}

fn secret() -> Secret {
    Secret::from_bytes([9; 32])
}

fn scan(secret: Option<&Secret>) -> Vec<RawProcess> {
    DarwinSource::new(secret).unwrap().scan().unwrap()
}

fn pick(processes: &[RawProcess], pid: i32) -> &RawProcess {
    processes
        .iter()
        .find(|process| process.identity.kernel.pid == pid)
        .unwrap_or_else(|| panic!("pid {pid} is not in the scan"))
}

fn own_kernel() -> KernelIdentity {
    let provider = DarwinProvider::new().unwrap();
    match provider.read(std::process::id() as i32).unwrap() {
        ProcessRead::Present(identity) => identity.kernel,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_scan_reports_the_kernel_identity_of_a_spawned_process() {
    let mut harness = harness();
    let handle = harness.spawn(&long()).unwrap();
    let identity = harness.identity(handle).clone();
    let processes = scan(None);
    let found = pick(&processes, harness.pid(handle));
    assert_eq!(found.identity.kernel, identity.kernel);
    assert_eq!(
        found.identity.exe_path.as_deref(),
        Some(identity.evidence.exe_path.as_path())
    );
    assert_eq!(found.identity.ppid, std::process::id() as i32);
    // SAFETY: getpgrp takes no arguments and cannot fail.
    assert_eq!(found.identity.pgid, unsafe { libc::getpgrp() });
    assert!(found.cpu.first_ns.is_some());
    assert_eq!(found.cpu.later_ns, None);
    assert!(!found.agent_script);
}

#[test]
fn a_scan_includes_the_tool_itself_and_reads_a_foreign_process_as_such_or_not_at_all() {
    let processes = scan(None);
    let me = pick(&processes, std::process::id() as i32);
    assert_eq!(me.identity.kernel, own_kernel());
    if let Some(init) = processes.iter().find(|p| p.identity.kernel.pid == 1) {
        assert_eq!(init.identity.kernel.uid, 0);
        assert_eq!(init.identity.ppid, 0);
    }
}

#[test]
fn a_scan_has_one_entry_per_pid_and_no_pid_below_one() {
    let processes = scan(None);
    let mut pids: Vec<i32> = processes.iter().map(|p| p.identity.kernel.pid).collect();
    assert!(pids.iter().all(|pid| *pid >= 1));
    pids.sort_unstable();
    let before = pids.len();
    pids.dedup();
    assert_eq!(before, pids.len());
}

#[test]
fn the_session_tag_is_keyed_with_the_secret_and_the_raw_value_is_dropped() {
    let mut harness = harness();
    let tag = SessionTag::from_bytes([5; 16]);
    let tagged = harness
        .spawn(&long().env("AGENTDUST_SESSION", tag.as_str()))
        .unwrap();
    let plain = harness.spawn(&long()).unwrap();
    let secret = secret();
    let keyed = scan(Some(&secret));
    assert_eq!(
        pick(&keyed, harness.pid(tagged)).tag,
        Tag::Keyed(tag.key(&secret))
    );
    assert_eq!(pick(&keyed, harness.pid(plain)).tag, Tag::Absent);
    assert!(!format!("{keyed:?}").contains(tag.as_str()));
    let unkeyed = scan(None);
    assert_eq!(pick(&unkeyed, harness.pid(tagged)).tag, Tag::Unkeyed);
    assert_eq!(pick(&unkeyed, harness.pid(plain)).tag, Tag::Absent);
    assert!(!format!("{unkeyed:?}").contains(tag.as_str()));
}

#[test]
fn cumulative_cpu_time_grows_with_work_and_needs_the_exact_identity() {
    let source = DarwinSource::new(None).unwrap();
    let kernel = own_kernel();
    let first = source.cpu_time_ns(&kernel).unwrap().expect("own cpu time");
    let mut sink = 0u64;
    assert!(wait_until(
        || {
            for n in 0..200_000u64 {
                sink = sink.wrapping_mul(31).wrapping_add(n);
            }
            std::hint::black_box(sink);
            source
                .cpu_time_ns(&kernel)
                .unwrap()
                .is_some_and(|now| now > first)
        },
        HANG_GUARD
    ));
    let mut reused = kernel.clone();
    reused.start_time_us += 1;
    assert_eq!(source.cpu_time_ns(&reused).unwrap(), None);
    let mut other_boot = kernel.clone();
    other_boot.boot_session_uuid.push('x');
    assert_eq!(source.cpu_time_ns(&other_boot).unwrap(), None);
    let mut nobody = kernel;
    nobody.pid = 0;
    assert_eq!(source.cpu_time_ns(&nobody).unwrap(), None);
}

#[test]
fn live_details_read_the_command_and_directory_of_the_identified_process() {
    let mut harness = harness();
    let handle = harness.spawn(&long()).unwrap();
    let kernel = harness.identity(handle).kernel.clone();
    let live = DarwinLive::new().unwrap();
    let command = live.command(&kernel).expect("command of a live process");
    assert!(command[0].ends_with(b"fixture-sleeper"));
    assert!(command.iter().any(|arg| arg == b"120"));
    let cwd = live.cwd(&kernel).expect("directory of a live process");
    assert_eq!(cwd, std::env::current_dir().unwrap().canonicalize().unwrap());
}

#[test]
fn live_details_refuse_a_process_that_is_not_the_one_identified() {
    let mut harness = harness();
    let handle = harness.spawn(&long()).unwrap();
    let live = DarwinLive::new().unwrap();
    let mut reused = harness.identity(handle).kernel.clone();
    reused.start_time_us += 1;
    assert_eq!(live.command(&reused), None);
    assert_eq!(live.cwd(&reused), None);
    let mut other_boot = harness.identity(handle).kernel.clone();
    other_boot.boot_session_uuid.push('x');
    assert_eq!(live.command(&other_boot), None);
    let mut nobody = harness.identity(handle).kernel.clone();
    nobody.pid = 0;
    assert_eq!(live.cwd(&nobody), None);
}

#[test]
fn live_details_of_a_process_that_has_ended_are_none() {
    let mut harness = harness();
    let handle = harness.spawn(&long()).unwrap();
    let kernel = harness.identity(handle).kernel.clone();
    harness
        .signal(handle, agentdust_testkit::harness::Signal::Kill)
        .unwrap();
    let live = DarwinLive::new().unwrap();
    assert!(wait_until(|| live.cwd(&kernel).is_none(), HANG_GUARD));
    assert_eq!(live.command(&kernel), None);
}

#[test]
fn the_launchctl_list_of_this_machine_parses_and_does_not_name_a_fixture() {
    let mut harness = harness();
    let handle = harness.spawn(&long()).unwrap();
    let pids = LaunchctlList.pids().unwrap();
    assert!(!pids.is_empty());
    assert!(pids.iter().all(|pid| *pid > 0));
    assert!(!pids.contains(&harness.pid(handle)));
    assert!(!pids.contains(&(std::process::id() as i32)));
}

#[test]
fn processes_with_an_unreadable_environment_never_block_the_scan() {
    let processes = scan(Some(&secret()));
    assert!(processes.len() > 20);
    let system =
        |process: &&RawProcess| {
            process.identity.exe_path.as_deref().is_some_and(|path| {
                path.starts_with(Path::new("/System")) || path.starts_with(Path::new("/usr"))
            })
        };
    assert!(processes.iter().filter(system).count() > 0);
    assert!(
        processes
            .iter()
            .filter(system)
            .all(|p| !matches!(p.tag, Tag::Keyed(_)))
    );
}

#[test]
fn ticks_become_nanoseconds_by_the_timer_frequency() {
    assert_eq!(ticks_to_ns(24_000_000, 24_000_000), 1_000_000_000);
    assert_eq!(ticks_to_ns(1, 24_000_000), 41);
    assert_eq!(ticks_to_ns(0, 24_000_000), 0);
    assert_eq!(ticks_to_ns(5, 1_000_000_000), 5);
    assert_eq!(ticks_to_ns(5, 0), 5);
    assert_eq!(ticks_to_ns(u64::MAX, 1_000_000_000), u64::MAX);
    assert_eq!(ticks_to_ns(u64::MAX, 24_000_000), u64::MAX);
}

#[test]
fn the_timer_frequency_of_this_machine_is_known() {
    assert!(tick_frequency() >= 1_000_000);
}
