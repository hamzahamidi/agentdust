#![cfg(target_os = "macos")]

mod apply_support;

use std::collections::{BTreeSet, HashMap};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use agentdust_core::apply::exec::{Deps, Outcome, Reason, Settings};
use agentdust_core::apply::lock::IdentityLock;
use agentdust_core::apply::server::{Server, Step};
use agentdust_core::apply::signal::Signaller;
use agentdust_core::apply::timer::SystemTimer;
use agentdust_core::class::Class;
use agentdust_core::classifier::{Finding, Policy, Provenance, classify};
use agentdust_core::cwd::CwdRelation;
use agentdust_core::darwin::{DarwinProvider, DarwinSource};
use agentdust_core::finding::ModelFinding;
use agentdust_core::inventory::{Clock, IDLE_SAMPLE_GAP, LaunchdSource, SystemClock, take};
use agentdust_core::journal::{Agent, AgentIdentity, Kind, Record, SCHEMA_VERSION};
use agentdust_core::live::LiveSurveyor;
use agentdust_core::provider::{ProcessProvider, ProcessRead};
use agentdust_core::revalidate::Revalidation;
use agentdust_core::secret::Secret;
use agentdust_core::session::{ProviderLiveness, scopes};
use agentdust_core::survey::Surveyor;
use agentdust_core::tag::SessionTag;
use agentdust_testkit::harness::{Harness, ProcHandle, Signal};
use agentdust_testkit::spec::ProcSpec;
use agentdust_testkit::wait_until;
use apply_support::{Gate, Guarded, Scratch, WAIT, accept, call_for, item_for, results};

const FIXTURE: &str = env!("CARGO_BIN_EXE_fixture-sleeper");
const MINUTE_US: u64 = 60_000_000;

struct Later;

impl Clock for Later {
    fn now_us(&self) -> u64 {
        SystemClock.now_us() + 31 * MINUTE_US
    }

    fn sleep(&self, _requested: Duration) {
        thread::sleep(Duration::from_millis(150));
    }
}

struct Listed(Arc<Mutex<BTreeSet<i32>>>);

impl LaunchdSource for Listed {
    fn pids(&self) -> io::Result<BTreeSet<i32>> {
        Ok(self.0.lock().unwrap().clone())
    }
}

fn my_uid() -> u32 {
    // SAFETY: geteuid takes no arguments and cannot fail.
    unsafe { libc::geteuid() }
}

struct Shared {
    secret: Secret,
    records: Mutex<Vec<Record>>,
    launchd: Arc<Mutex<BTreeSet<i32>>>,
}

struct Machine(Arc<Shared>);

impl Surveyor for Machine {
    fn survey(&self) -> io::Result<Vec<Finding>> {
        let source = DarwinSource::new(Some(&self.0.secret))?;
        let snapshot = take(
            &source,
            &Listed(Arc::clone(&self.0.launchd)),
            &Later,
            IDLE_SAMPLE_GAP,
        )?;
        let provider = DarwinProvider::new()?;
        let found = scopes(&self.0.records.lock().unwrap(), &ProviderLiveness(&provider));
        Ok(classify(
            &snapshot,
            &Provenance::Available(&found),
            &Policy::new(my_uid(), std::process::id() as i32),
        ))
    }

    fn describe(&self, finding: &Finding) -> ModelFinding {
        ModelFinding::new(finding, CwdRelation::Other)
    }
}

struct Shifting {
    inner: DarwinProvider,
    armed: Arc<AtomicBool>,
    reads: Arc<AtomicU64>,
}

impl ProcessProvider for Shifting {
    fn read(&self, pid: i32) -> io::Result<ProcessRead> {
        let read = self.inner.read(pid)?;
        self.reads.fetch_add(1, Ordering::SeqCst);
        match read {
            ProcessRead::Present(mut identity) if self.armed.load(Ordering::SeqCst) => {
                identity.kernel.start_time_us += 1;
                Ok(ProcessRead::Present(identity))
            }
            other => Ok(other),
        }
    }
}

struct Live {
    harness: Arc<Mutex<Harness>>,
    handles: HashMap<i32, ProcHandle>,
    shared: Arc<Shared>,
    data: Scratch,
    ended_children: Vec<ProcHandle>,
    tunnel: ProcHandle,
    stubborn: ProcHandle,
    spare: ProcHandle,
}

fn spec() -> ProcSpec {
    ProcSpec::new().seconds(120)
}

fn live() -> Live {
    let mut harness = Harness::new(FIXTURE).unwrap();
    let secret = Secret::from_bytes([7; 32]);
    let tag = SessionTag::from_bytes([0xc3; 16]);
    let ended = harness
        .spawn_tree(&spec().spawn(2).env("AGENTDUST_SESSION", tag.as_str()))
        .unwrap();
    let tunnel = harness.spawn_tree(&spec().spawn(1).setsid()).unwrap();
    let stubborn = harness
        .spawn_tree(&spec().spawn(1).setsid().ignore_term())
        .unwrap();
    let spare = harness.spawn_tree(&spec().spawn(1).setsid()).unwrap();
    let identity = harness.identity(ended.parent).clone();
    let record = Record {
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
    };
    for tree in [&ended, &tunnel, &stubborn, &spare] {
        harness.orphan(tree.parent).unwrap();
    }
    let mut handles = HashMap::new();
    for handle in ended
        .children
        .iter()
        .chain(&tunnel.children)
        .chain(&stubborn.children)
        .chain(&spare.children)
    {
        handles.insert(harness.pid(*handle), *handle);
    }
    Live {
        handles,
        shared: Arc::new(Shared {
            secret,
            records: Mutex::new(vec![record]),
            launchd: Arc::default(),
        }),
        data: Scratch::new("data"),
        ended_children: ended.children.clone(),
        tunnel: tunnel.children[0],
        stubborn: stubborn.children[0],
        spare: spare.children[0],
        harness: Arc::new(Mutex::new(harness)),
    }
}

fn settings() -> Settings {
    Settings {
        term_wait: Duration::from_millis(600),
        ..Settings::default()
    }
}

impl Live {
    fn pid(&self, handle: ProcHandle) -> i32 {
        self.harness.lock().unwrap().pid(handle)
    }

    fn guarded(&self, gate: Option<Gate>) -> Box<dyn Signaller> {
        Box::new(Guarded {
            harness: Arc::clone(&self.harness),
            handles: self.handles.clone(),
            gate,
        })
    }

    fn server_with(
        &self,
        provider: Box<dyn ProcessProvider + Send + Sync>,
        signaller: Box<dyn Signaller>,
    ) -> Server {
        Server::new(
            Deps {
                data_dir: self.data.path().to_path_buf(),
                surveyor: Arc::new(Machine(Arc::clone(&self.shared))),
                provider,
                signaller,
                timer: Arc::new(SystemTimer::new()),
            },
            settings(),
        )
    }

    fn server(&self) -> Server {
        self.server_with(Box::new(DarwinProvider::new().unwrap()), self.guarded(None))
    }

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

    fn add_owner(&self, session_id: &str, identity: &agentdust_core::identity::KernelIdentity) {
        let tag = SessionTag::from_bytes([0xc3; 16]);
        self.shared.records.lock().unwrap().push(Record {
            v: SCHEMA_VERSION,
            kind: Kind::SessionStart,
            agent: Agent::Claude,
            session_id: session_id.to_owned(),
            subagent_id: None,
            agent_identity: Some(
                AgentIdentity::new(identity.pid, identity.start_time_us, identity.uid, None).unwrap(),
            ),
            tool_use_id: None,
            wall_ts: 2,
            mono_ts: 2,
            boot: identity.boot_session_uuid.clone(),
            session_tag_key: Some(tag.key(&self.shared.secret)),
            cwd_key: None,
            exe_base: None,
        });
    }

    fn exited(&self, handle: ProcHandle) -> bool {
        wait_until(|| self.state(handle) == Revalidation::Gone, WAIT)
    }
}

#[test]
fn a_cooperative_suspect_is_terminated_and_only_it_gets_a_term() {
    let live = live();
    let server = live.server();
    let created = server.plan().unwrap();
    let pid = live.pid(live.tunnel);
    assert_eq!(item_for(&created, pid).class, Class::Suspect);
    let report = server
        .run(&call_for(&created, &[pid]), &mut |challenge| accept(challenge))
        .unwrap();
    assert_eq!(results(&report), [(Outcome::Terminated, None)]);
    assert!(live.exited(live.tunnel));
    assert_eq!(live.terms(), [pid]);
    assert_eq!(live.state(live.spare), Revalidation::Match);
}

#[test]
fn an_owned_ended_batch_takes_one_code_and_ends_every_child() {
    let live = live();
    let server = live.server();
    let created = server.plan().unwrap();
    let pids: Vec<i32> = live
        .ended_children
        .iter()
        .map(|handle| live.pid(*handle))
        .collect();
    for pid in &pids {
        assert_eq!(item_for(&created, *pid).class, Class::OwnedEnded);
    }
    let mut prompts = 0;
    let report = server
        .run(&call_for(&created, &pids), &mut |challenge| {
            prompts += 1;
            accept(challenge)
        })
        .unwrap();
    assert_eq!(prompts, 1);
    assert_eq!(results(&report), vec![(Outcome::Terminated, None); 2]);
    for handle in &live.ended_children {
        assert!(live.exited(*handle));
    }
    assert_eq!(live.terms(), pids);
}

#[test]
fn a_process_that_ignores_sigterm_is_a_survivor_and_is_not_signalled_again() {
    let live = live();
    let server = live.server();
    let created = server.plan().unwrap();
    let pid = live.pid(live.stubborn);
    let report = server
        .run(&call_for(&created, &[pid]), &mut |challenge| accept(challenge))
        .unwrap();
    assert_eq!(results(&report), [(Outcome::Survivor, None)]);
    assert_eq!(live.state(live.stubborn), Revalidation::Match);
    assert_eq!(live.terms(), [pid]);
}

#[test]
fn a_process_that_exits_during_approval_is_reported_gone_without_a_signal() {
    let live = live();
    let server = live.server();
    let created = server.plan().unwrap();
    let pid = live.pid(live.tunnel);
    let the_call = call_for(&created, &[pid]);
    let Step::Ask(challenge) = server.begin(&the_call).unwrap() else {
        panic!("a prompt is expected");
    };
    live.harness
        .lock()
        .unwrap()
        .signal(live.tunnel, Signal::Kill)
        .unwrap();
    assert!(live.exited(live.tunnel));
    let Step::Done(report) = server
        .answer(&the_call, &challenge.nonce, accept(&challenge))
        .unwrap()
    else {
        panic!("the call has one unit");
    };
    assert_eq!(results(&report), [(Outcome::Gone, None)]);
    assert!(live.terms().is_empty());
}

#[test]
fn a_process_that_becomes_managed_during_approval_is_not_signalled() {
    let live = live();
    let server = live.server();
    let created = server.plan().unwrap();
    let pid = live.pid(live.tunnel);
    let the_call = call_for(&created, &[pid]);
    let Step::Ask(challenge) = server.begin(&the_call).unwrap() else {
        panic!("a prompt is expected");
    };
    live.shared.launchd.lock().unwrap().insert(pid);
    let Step::Done(report) = server
        .answer(&the_call, &challenge.nonce, accept(&challenge))
        .unwrap()
    else {
        panic!("the call has one unit");
    };
    assert_eq!(
        results(&report),
        [(Outcome::RevalidationFailed, Some(Reason::ClassChanged))]
    );
    assert!(live.terms().is_empty());
    assert_eq!(live.state(live.tunnel), Revalidation::Match);
}

#[test]
fn a_process_that_loses_its_evidence_during_approval_is_not_signalled() {
    let live = live();
    let server = live.server();
    let created = server.plan().unwrap();
    let pids: Vec<i32> = live
        .ended_children
        .iter()
        .map(|handle| live.pid(*handle))
        .collect();
    let the_call = call_for(&created, &pids);
    let Step::Ask(challenge) = server.begin(&the_call).unwrap() else {
        panic!("a prompt is expected");
    };
    live.shared.records.lock().unwrap().clear();
    let Step::Done(report) = server
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

#[test]
fn an_owner_that_resumes_during_approval_keeps_the_candidate_live() {
    let live = live();
    let server = live.server();
    let created = server.plan().unwrap();
    let handle = live.ended_children[0];
    let pid = live.pid(handle);
    let the_call = call_for(&created, &[pid]);
    let Step::Ask(challenge) = server.begin(&the_call).unwrap() else {
        panic!("a prompt is expected");
    };
    let owner = live.harness.lock().unwrap().identity(live.spare).kernel.clone();
    live.add_owner("resumed-owner", &owner);
    let Step::Done(report) = server
        .answer(&the_call, &challenge.nonce, accept(&challenge))
        .unwrap()
    else {
        panic!("the call has one unit");
    };
    assert_eq!(
        results(&report),
        [(Outcome::RevalidationFailed, Some(Reason::ClassChanged))]
    );
    assert!(live.terms().is_empty());
}

#[test]
fn an_owner_set_change_during_approval_aborts_even_when_every_owner_is_gone() {
    let live = live();
    let server = live.server();
    let created = server.plan().unwrap();
    let handle = live.ended_children[0];
    let pid = live.pid(handle);
    let the_call = call_for(&created, &[pid]);
    let Step::Ask(challenge) = server.begin(&the_call).unwrap() else {
        panic!("a prompt is expected");
    };
    let owner = live.harness.lock().unwrap().identity(live.spare).kernel.clone();
    live.harness
        .lock()
        .unwrap()
        .signal(live.spare, Signal::Kill)
        .unwrap();
    assert!(live.exited(live.spare));
    live.add_owner("gone-owner", &owner);
    let Step::Done(report) = server
        .answer(&the_call, &challenge.nonce, accept(&challenge))
        .unwrap()
    else {
        panic!("the call has one unit");
    };
    assert_eq!(
        results(&report),
        [(Outcome::RevalidationFailed, Some(Reason::OwnershipChanged))]
    );
    assert!(live.terms().is_empty());
}

#[test]
fn an_identity_that_changes_between_the_read_and_the_signal_aborts() {
    let live = live();
    let armed = Arc::new(AtomicBool::new(false));
    let reads = Arc::new(AtomicU64::new(0));
    let server = live.server_with(
        Box::new(Shifting {
            inner: DarwinProvider::new().unwrap(),
            armed: Arc::clone(&armed),
            reads: Arc::clone(&reads),
        }),
        live.guarded(None),
    );
    let created = server.plan().unwrap();
    let pid = live.pid(live.tunnel);
    let the_call = call_for(&created, &[pid]);
    let Step::Ask(challenge) = server.begin(&the_call).unwrap() else {
        panic!("a prompt is expected");
    };
    armed.store(true, Ordering::SeqCst);
    let Step::Done(report) = server
        .answer(&the_call, &challenge.nonce, accept(&challenge))
        .unwrap()
    else {
        panic!("the call has one unit");
    };
    assert_eq!(
        results(&report),
        [(Outcome::RevalidationFailed, Some(Reason::IdentityChanged))]
    );
    assert!(live.terms().is_empty());
    assert_eq!(live.state(live.tunnel), Revalidation::Match);
}

#[test]
fn no_lock_is_held_while_a_prompt_waits() {
    let live = live();
    let server = live.server();
    let created = server.plan().unwrap();
    let pid = live.pid(live.tunnel);
    let the_call = call_for(&created, &[pid]);
    let Step::Ask(challenge) = server.begin(&the_call).unwrap() else {
        panic!("a prompt is expected");
    };
    let held =
        IdentityLock::try_acquire(&live.data.path().join("locks"), &item_for(&created, pid).item_id).unwrap();
    assert!(held.is_some(), "the lock is free while the person is asked");
    drop(held);
    let other = live.pid(live.spare);
    let second = server
        .run(&call_for(&created, &[other]), &mut |challenge| accept(challenge))
        .unwrap();
    assert_eq!(results(&second), [(Outcome::Terminated, None)]);
    let Step::Done(report) = server
        .answer(&the_call, &challenge.nonce, accept(&challenge))
        .unwrap()
    else {
        panic!("the call has one unit");
    };
    assert_eq!(results(&report), [(Outcome::Terminated, None)]);
    assert_eq!(live.terms(), [other, pid]);
}

#[test]
fn two_servers_that_apply_the_same_item_signal_it_once() {
    let live = live();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let gated = live.server_with(
        Box::new(DarwinProvider::new().unwrap()),
        live.guarded(Some(Gate {
            entered: Mutex::new(entered_tx),
            release: Mutex::new(release_rx),
        })),
    );
    let plain = live.server();
    let pid = live.pid(live.tunnel);
    let first_plan = gated.plan().unwrap();
    let second_plan = plain.plan().unwrap();
    let first_call = call_for(&first_plan, &[pid]);
    let second_call = call_for(&second_plan, &[pid]);

    thread::scope(|scope| {
        let first = scope.spawn(|| {
            gated
                .run(&first_call, &mut |challenge| accept(challenge))
                .unwrap()
        });
        entered_rx.recv().unwrap();
        let second = plain
            .run(&second_call, &mut |challenge| accept(challenge))
            .unwrap();
        assert_eq!(results(&second), [(Outcome::HandledElsewhere, None)]);
        release_tx.send(()).unwrap();
        let first = first.join().unwrap();
        assert_eq!(results(&first), [(Outcome::Terminated, None)]);
    });
    assert_eq!(live.terms(), [pid]);
    assert!(live.exited(live.tunnel));
}

#[test]
fn the_live_surveyor_classifies_this_process_as_managed_and_signals_nothing() {
    let data = Scratch::new("surveyor");
    let surveyor = LiveSurveyor::new(data.path());
    let found = surveyor.survey().unwrap();
    let me = std::process::id() as i32;
    let own = found
        .iter()
        .find(|finding| finding.identity.kernel.pid == me)
        .expect("the scan lists this process");
    assert_eq!(own.class, Class::Managed);
    let model = surveyor.describe(own);
    assert_eq!(model.pid, me);
    assert_eq!(model.class, Class::Managed);
}
