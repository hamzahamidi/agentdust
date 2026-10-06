#![cfg(target_os = "macos")]

mod apply_support;

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentdust_core::apply::exec::{Deps, Outcome, Reason, Settings};
use agentdust_core::apply::server::{Server, Step};
use agentdust_core::apply::timer::SystemTimer;
use agentdust_core::class::Class;
use agentdust_core::darwin::DarwinProvider;
use agentdust_core::journal::{ACTIVE_FILE, Agent, AgentIdentity, Kind, Record, SCHEMA_VERSION, append};
use agentdust_core::live::LiveSurveyor;
use agentdust_core::revalidate::Revalidation;
use agentdust_core::secret::{SECRET_FILE, load_or_create};
use agentdust_core::tag::SessionTag;
use agentdust_testkit::harness::{Harness, ProcHandle, Signal};
use agentdust_testkit::spec::ProcSpec;
use agentdust_testkit::wait_until;
use apply_support::{Guarded, Scratch, WAIT, accept, call_for, item_for, results};

const FIXTURE: &str = env!("CARGO_BIN_EXE_fixture-sleeper");

struct Setup {
    harness: Arc<Mutex<Harness>>,
    children: Vec<ProcHandle>,
    pids: Vec<i32>,
    data: Scratch,
    server: Server,
}

fn setup() -> Setup {
    let data = Scratch::new("e2e");
    let secret = load_or_create(data.path()).unwrap();
    let tag = SessionTag::from_bytes([0xd4; 16]);
    let mut harness = Harness::new(FIXTURE).unwrap();
    let tree = harness
        .spawn_tree(
            &ProcSpec::new()
                .seconds(120)
                .spawn(2)
                .env("AGENTDUST_SESSION", tag.as_str()),
        )
        .unwrap();
    let identity = harness.identity(tree.parent).clone();
    append(
        data.path(),
        &Record {
            v: SCHEMA_VERSION,
            kind: Kind::SessionStart,
            agent: Agent::Claude,
            session_id: "ended".to_owned(),
            subagent_id: None,
            agent_identity: Some(AgentIdentity::from_process(&identity).unwrap()),
            tool_use_id: None,
            wall_ts: 1,
            mono_ts: 1,
            boot: identity.kernel.boot_session_uuid.clone(),
            session_tag_key: Some(tag.key(&secret)),
            cwd_key: None,
            exe_base: None,
        },
    )
    .unwrap();
    harness.orphan(tree.parent).unwrap();
    let pids: Vec<i32> = tree.children.iter().map(|child| harness.pid(*child)).collect();
    let handles: HashMap<i32, ProcHandle> = pids.iter().copied().zip(tree.children.iter().copied()).collect();
    let harness = Arc::new(Mutex::new(harness));
    let server = Server::new(
        Deps {
            data_dir: data.path().to_path_buf(),
            surveyor: Arc::new(LiveSurveyor::new(data.path())),
            provider: Box::new(DarwinProvider::new().unwrap()),
            signaller: Box::new(Guarded {
                harness: Arc::clone(&harness),
                handles,
                gate: None,
            }),
            timer: Arc::new(SystemTimer::new()),
        },
        Settings {
            term_wait: Duration::from_millis(600),
            ..Settings::default()
        },
    );
    Setup {
        harness,
        children: tree.children,
        pids,
        data,
        server,
    }
}

impl Setup {
    fn terms(&self) -> Vec<i32> {
        self.harness
            .lock()
            .unwrap()
            .signal_log()
            .iter()
            .filter(|event| event.signal == Signal::Term)
            .map(|event| event.pid)
            .collect()
    }

    fn state(&self, handle: ProcHandle) -> Revalidation {
        self.harness.lock().unwrap().revalidate(handle)
    }
}

#[test]
fn leftovers_of_an_ended_session_are_found_in_the_journal_and_stopped_after_one_code() {
    let live = setup();
    let created = live.server.plan().unwrap();
    for pid in &live.pids {
        assert_eq!(item_for(&created, *pid).class, Class::OwnedEnded);
    }
    let mut prompts = 0;
    let report = live
        .server
        .run(&call_for(&created, &live.pids), &mut |challenge| {
            prompts += 1;
            accept(challenge)
        })
        .unwrap();
    assert_eq!(prompts, 1);
    assert_eq!(results(&report), vec![(Outcome::Terminated, None); 2]);
    for child in &live.children {
        assert!(wait_until(|| live.state(*child) == Revalidation::Gone, WAIT));
    }
    assert_eq!(live.terms(), live.pids);
    let audit = fs::read_to_string(live.data.path().join("audit.log")).unwrap();
    assert_eq!(audit.lines().count(), 4);
}

#[test]
fn a_journal_of_a_newer_version_during_approval_leaves_the_processes_alone() {
    let live = setup();
    let created = live.server.plan().unwrap();
    let the_call = call_for(&created, &live.pids);
    let Step::Ask(challenge) = live.server.begin(&the_call).unwrap() else {
        panic!("a prompt is expected");
    };
    fs::OpenOptions::new()
        .append(true)
        .open(live.data.path().join(ACTIVE_FILE))
        .unwrap()
        .write_all(b"\x1e{\"v\":3,\"kind\":\"session_start\"}\n")
        .unwrap();
    let Step::Done(report) = live
        .server
        .answer(&the_call, &challenge.nonce, accept(&challenge))
        .unwrap()
    else {
        panic!("the call has one unit");
    };
    assert_eq!(
        results(&report),
        vec![(Outcome::RevalidationFailed, Some(Reason::ClassChanged)); 2]
    );
    assert!(live.terms().is_empty());
    for child in &live.children {
        assert_eq!(live.state(*child), Revalidation::Match);
    }
    let fresh = live.server.plan().unwrap();
    for pid in &live.pids {
        assert!(fresh.items.iter().all(|item| item.pid != *pid));
    }
}

#[test]
fn a_lost_install_secret_during_approval_leaves_the_processes_alone() {
    let live = setup();
    let created = live.server.plan().unwrap();
    let the_call = call_for(&created, &live.pids);
    let Step::Ask(challenge) = live.server.begin(&the_call).unwrap() else {
        panic!("a prompt is expected");
    };
    fs::remove_file(live.data.path().join(SECRET_FILE)).unwrap();
    let Step::Done(report) = live
        .server
        .answer(&the_call, &challenge.nonce, accept(&challenge))
        .unwrap()
    else {
        panic!("the call has one unit");
    };
    assert_eq!(
        results(&report),
        vec![(Outcome::RevalidationFailed, Some(Reason::ClassChanged)); 2]
    );
    assert!(live.terms().is_empty());
}
