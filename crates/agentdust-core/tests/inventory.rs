mod inventory_support;

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use agentdust_core::digest::{Domain, keyed_digest};
use agentdust_core::inventory::{ArgsView, Cpu, IDLE_SAMPLE_GAP, Launchd, Snapshot, Tag, read_args, take};
use agentdust_core::secret::Secret;
use agentdust_core::tag::SessionTag;
use inventory_support::{Edit, Later, NOW, ScriptedClock, ScriptedLaunchd, ScriptedSource, log, raw};

fn snapshot(source: &ScriptedSource, launchd: &ScriptedLaunchd, clock: &ScriptedClock) -> Snapshot {
    take(source, launchd, clock, IDLE_SAMPLE_GAP).unwrap()
}

#[test]
fn the_idle_gap_is_two_seconds() {
    assert_eq!(IDLE_SAMPLE_GAP, Duration::from_secs(2));
}

#[test]
fn a_snapshot_scans_waits_samples_cpu_again_then_lists_launchd_then_reads_the_clock() {
    let log = log();
    let source = ScriptedSource::new(&log, vec![raw(101), raw(102)]);
    let launchd = ScriptedLaunchd::new(&log, &[7]);
    let clock = ScriptedClock::new(&log, NOW);
    snapshot(&source, &launchd, &clock);
    assert_eq!(
        *log.borrow(),
        ["scan", "sleep 2000ms", "cpu 101", "cpu 102", "launchd", "now"]
    );
}

#[test]
fn the_snapshot_time_is_read_after_the_wait() {
    let log = log();
    let source = ScriptedSource::new(&log, vec![raw(101)]);
    let launchd = ScriptedLaunchd::new(&log, &[]);
    let clock = ScriptedClock::new(&log, NOW);
    let taken = snapshot(&source, &launchd, &clock);
    assert_eq!(taken.taken_at_us, NOW + 2_000_000);
}

#[test]
fn the_second_cpu_read_fills_the_later_sample_and_leaves_the_first_alone() {
    let log = log();
    let source =
        ScriptedSource::new(&log, vec![raw(101), raw(102), raw(103)]).later(102, Later::Value(5_000));
    let launchd = ScriptedLaunchd::new(&log, &[]);
    let clock = ScriptedClock::new(&log, NOW);
    let taken = snapshot(&source, &launchd, &clock);
    let cpu = |pid: i32| {
        taken
            .processes
            .iter()
            .find(|process| process.identity.kernel.pid == pid)
            .unwrap()
            .cpu
    };
    assert_eq!(
        cpu(101),
        Cpu {
            first_ns: Some(1_000),
            later_ns: Some(1_000)
        }
    );
    assert_eq!(
        cpu(102),
        Cpu {
            first_ns: Some(1_000),
            later_ns: Some(5_000)
        }
    );
    assert!(cpu(101).idle());
    assert!(!cpu(102).idle());
}

#[test]
fn a_gone_or_unreadable_second_sample_is_never_idle() {
    let log = log();
    let source = ScriptedSource::new(&log, vec![raw(101), raw(102)])
        .later(101, Later::Gone)
        .later(102, Later::Fails);
    let launchd = ScriptedLaunchd::new(&log, &[]);
    let clock = ScriptedClock::new(&log, NOW);
    let taken = snapshot(&source, &launchd, &clock);
    for process in &taken.processes {
        assert_eq!(process.cpu.later_ns, None);
        assert!(!process.cpu.idle());
    }
}

#[test]
fn a_process_whose_first_read_failed_is_not_read_again() {
    let log = log();
    let source = ScriptedSource::new(&log, vec![raw(101).cpu(None, None), raw(102)]);
    let launchd = ScriptedLaunchd::new(&log, &[]);
    let clock = ScriptedClock::new(&log, NOW);
    let taken = snapshot(&source, &launchd, &clock);
    assert_eq!(
        *log.borrow(),
        ["scan", "sleep 2000ms", "cpu 102", "launchd", "now"]
    );
    assert!(!taken.processes[0].cpu.idle());
}

#[test]
fn the_idle_rule_needs_two_equal_samples() {
    let cases = [
        (Some(5), Some(5), true),
        (Some(5), Some(6), false),
        (Some(6), Some(5), false),
        (Some(5), None, false),
        (None, Some(5), false),
        (None, None, false),
        (Some(0), Some(0), true),
    ];
    for (first_ns, later_ns, idle) in cases {
        assert_eq!(
            Cpu { first_ns, later_ns }.idle(),
            idle,
            "{first_ns:?} {later_ns:?}"
        );
    }
}

#[test]
fn launchd_pids_are_known_when_the_list_is_read() {
    let log = log();
    let source = ScriptedSource::new(&log, vec![raw(101)]);
    let launchd = ScriptedLaunchd::new(&log, &[7, 9]);
    let clock = ScriptedClock::new(&log, NOW);
    let taken = snapshot(&source, &launchd, &clock);
    assert_eq!(taken.launchd, Launchd::Known(BTreeSet::from([7, 9])));
}

#[test]
fn a_failed_launchd_list_is_unavailable_and_not_an_empty_set() {
    let log = log();
    let source = ScriptedSource::new(&log, vec![raw(101)]);
    let launchd = ScriptedLaunchd::failing(&log);
    let clock = ScriptedClock::new(&log, NOW);
    let taken = snapshot(&source, &launchd, &clock);
    assert_eq!(taken.launchd, Launchd::Unavailable);
}

#[test]
fn a_failed_scan_is_an_error_and_nothing_else_runs() {
    let log = log();
    let source = ScriptedSource::new(&log, vec![raw(101)]).failing();
    let launchd = ScriptedLaunchd::new(&log, &[]);
    let clock = ScriptedClock::new(&log, NOW);
    assert!(take(&source, &launchd, &clock, IDLE_SAMPLE_GAP).is_err());
    assert_eq!(*log.borrow(), ["scan"]);
}

fn buffer(exec_path: &str, args: &[&str], env: &[&str]) -> Vec<u8> {
    let mut buf = (args.len() as i32).to_ne_bytes().to_vec();
    buf.extend_from_slice(exec_path.as_bytes());
    buf.extend_from_slice(&[0, 0, 0]);
    for part in args.iter().chain(env) {
        buf.extend_from_slice(part.as_bytes());
        buf.push(0);
    }
    buf
}

fn secret() -> Secret {
    Secret::from_bytes([7; 32])
}

fn view(buf: Option<&[u8]>, exe: &str, secret: Option<&Secret>) -> ArgsView {
    read_args(buf, Some(Path::new(exe)), secret)
}

#[test]
fn the_session_tag_is_hashed_with_the_install_secret_at_once() {
    let secret = secret();
    let tag = SessionTag::from_bytes([3; 16]);
    let env = format!("AGENTDUST_SESSION={}", tag.as_str());
    let buf = buffer("/bin/x", &["x"], &["HOME=/h", &env]);
    let found = view(Some(&buf), "/bin/x", Some(&secret));
    assert_eq!(found.tag, Tag::Keyed(tag.key(&secret)));
    assert!(!format!("{found:?}").contains(tag.as_str()));
}

#[test]
fn the_key_is_the_session_domain_digest_of_the_raw_value() {
    let secret = secret();
    let buf = buffer("/bin/x", &["x"], &["AGENTDUST_SESSION=anything at all"]);
    let found = view(Some(&buf), "/bin/x", Some(&secret));
    let expected = keyed_digest(secret.as_bytes(), Domain::Session, b"anything at all");
    match found.tag {
        Tag::Keyed(key) => assert_eq!(key.as_str(), expected),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_process_without_the_variable_has_no_tag() {
    let secret = secret();
    let buf = buffer(
        "/bin/x",
        &["x"],
        &["HOME=/h", "AGENTDUST_SESSION_OLD=a", "AGENTDUST=b"],
    );
    assert_eq!(view(Some(&buf), "/bin/x", Some(&secret)).tag, Tag::Absent);
}

#[test]
fn a_hidden_environment_is_treated_as_no_tag() {
    assert_eq!(view(None, "/usr/bin/x", Some(&secret())).tag, Tag::Absent);
    assert_eq!(view(None, "/usr/bin/x", None).tag, Tag::Absent);
}

#[test]
fn an_unparsable_buffer_is_unreadable_and_not_absent() {
    let found = view(Some(&[1, 2]), "/bin/x", Some(&secret()));
    assert_eq!(found.tag, Tag::Unreadable);
    assert!(!found.agent_script);
}

#[test]
fn a_tag_that_cannot_be_hashed_without_a_secret_is_unkeyed_and_not_absent() {
    let buf = buffer("/bin/x", &["x"], &["AGENTDUST_SESSION=abc"]);
    assert_eq!(view(Some(&buf), "/bin/x", None).tag, Tag::Unkeyed);
    let none = buffer("/bin/x", &["x"], &["HOME=/h"]);
    assert_eq!(view(Some(&none), "/bin/x", None).tag, Tag::Absent);
}

#[test]
fn an_empty_value_is_still_a_tag() {
    let buf = buffer("/bin/x", &["x"], &["AGENTDUST_SESSION="]);
    assert!(matches!(
        view(Some(&buf), "/bin/x", Some(&secret())).tag,
        Tag::Keyed(_)
    ));
}

#[test]
fn node_running_a_script_that_names_claude_is_an_agent_script() {
    let buf = buffer(
        "/opt/homebrew/bin/node",
        &[
            "node",
            "/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js",
        ],
        &[],
    );
    assert!(view(Some(&buf), "/opt/homebrew/bin/node", None).agent_script);
}

#[test]
fn another_script_or_another_executable_is_not_an_agent_script() {
    let other = buffer("/opt/homebrew/bin/node", &["node", "/app/server.js"], &[]);
    assert!(!view(Some(&other), "/opt/homebrew/bin/node", None).agent_script);
    let python = buffer("/usr/bin/python3", &["python3", "/x/claude/run.py"], &[]);
    assert!(!view(Some(&python), "/usr/bin/python3", None).agent_script);
    let bare = buffer("/opt/homebrew/bin/node", &["node"], &[]);
    assert!(!view(Some(&bare), "/opt/homebrew/bin/node", None).agent_script);
}

#[test]
fn a_process_without_a_readable_path_is_never_an_agent_script() {
    let buf = buffer("/opt/homebrew/bin/node", &["node", "/x/claude/cli.js"], &[]);
    assert!(!read_args(Some(&buf), None, None).agent_script);
}

#[test]
fn codex_session_marker_is_domain_separated_and_thread_id_is_not_used() {
    let secret = Secret::from_bytes([42; 32]);
    let session = "12345678-1234-1234-1234-123456789abc";
    let value = format!("CODEX_SESSION_ID={session}");
    let buf = buffer("/bin/x", &["x"], &[&value, "CODEX_THREAD_ID=another-thread"]);
    let key = agentdust_core::tag::codex_key_of(&secret, session.as_bytes()).unwrap();
    assert_eq!(
        read_args(Some(&buf), None, Some(&secret)).tag,
        Tag::Keyed(key.clone())
    );
    assert_ne!(key, agentdust_core::tag::key_of(&secret, session.as_bytes()));
    let thread_only = buffer(
        "/bin/x",
        &["x"],
        &["CODEX_THREAD_ID=12345678-1234-1234-1234-123456789abc"],
    );
    assert_eq!(
        read_args(Some(&thread_only), None, Some(&secret)).tag,
        Tag::Absent
    );
    assert_eq!(read_args(Some(&buf), None, None).tag, Tag::Unkeyed);
}

#[test]
fn invalid_or_dual_agent_session_markers_are_unverifiable() {
    let secret = Secret::from_bytes([42; 32]);
    for vars in [
        vec!["CODEX_SESSION_ID=bad"],
        vec!["CODEX_SESSION_ID="],
        vec![
            "CODEX_SESSION_ID=12345678-1234-1234-1234-123456789abc",
            "AGENTDUST_SESSION=abc",
        ],
    ] {
        let buf = buffer("/bin/x", &["x"], &vars);
        assert_eq!(read_args(Some(&buf), None, Some(&secret)).tag, Tag::Unreadable);
    }
}
