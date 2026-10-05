#![allow(dead_code)]

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentdust_core::apply::audit::AUDIT_FILE;
use agentdust_core::apply::exec::{Deps, Executor, Settings};
use agentdust_core::apply::signal::{SignalResult, Signaller};
use agentdust_core::apply::timer::Timer;
use agentdust_core::class::Class;
use agentdust_core::classifier::{Evidence, Finding};
use agentdust_core::cwd::CwdRelation;
use agentdust_core::finding::ModelFinding;
use agentdust_core::identity::KernelIdentity;
use agentdust_core::inventory::RawIdentity;
use agentdust_core::plan::PlanItem;
use agentdust_core::provider::{ProcessProvider, ProcessRead};
use agentdust_core::survey::Surveyor;
use serde_json::Value;

use crate::scratch::TempDir;

pub const PLAN: &str = "0123456789abcdef0123456789abcdef";
pub const START: u64 = 1_800_000_000_000_000;
pub const EXE: &str = "/opt/homebrew/bin/node";

pub fn kernel(pid: i32) -> KernelIdentity {
    KernelIdentity {
        boot_session_uuid: "boot-1".to_owned(),
        pid,
        start_time_us: START.wrapping_add(pid as i64 as u64),
        uid: 501,
    }
}

pub fn evidence_of(class: Class) -> Vec<Evidence> {
    match class {
        Class::OwnedEnded => vec![Evidence::OwnedTag, Evidence::OwnedAgentGone],
        Class::Suspect => vec![
            Evidence::SuspectParentLaunchd,
            Evidence::SuspectSameUser,
            Evidence::SuspectAge,
            Evidence::SuspectIdle,
        ],
        Class::OwnedLive => vec![Evidence::OwnedTag, Evidence::OwnedAgentAlive],
        Class::Managed => vec![Evidence::ManagedLaunchd],
        Class::LikelyOwned | Class::Unknown => Vec::new(),
    }
}

pub fn finding_at(pid: i32, class: Class, exe: &str) -> Finding {
    Finding {
        identity: RawIdentity {
            kernel: kernel(pid),
            exe_path: Some(exe.into()),
            ppid: 1,
            pgid: pid,
        },
        class,
        evidence: evidence_of(class),
        age_us: 3 * 3600 * 1_000_000,
    }
}

pub fn finding(pid: i32, class: Class) -> Finding {
    finding_at(pid, class, EXE)
}

pub fn describe(finding: &Finding) -> ModelFinding {
    ModelFinding::new(finding, CwdRelation::Other)
}

pub fn item(pid: i32, class: Class) -> PlanItem {
    let found = finding(pid, class);
    PlanItem::from_finding(&found, describe(&found)).expect("an actionable finding makes a plan item")
}

pub fn write_config(dir: &Path, text: &str, mode: u32) {
    let path = dir.join("config.toml");
    fs::write(&path, text).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
}

pub fn present(item: &PlanItem) -> ProcessRead {
    ProcessRead::Present(item.identity.clone())
}

pub fn audit_len(dir: &Path) -> u64 {
    fs::metadata(dir.join(AUDIT_FILE)).map_or(0, |meta| meta.len())
}

#[derive(Clone, Default)]
pub struct Events(Arc<Mutex<Vec<String>>>);

impl Events {
    pub fn push(&self, event: impl Into<String>) {
        self.0.lock().unwrap().push(event.into());
    }

    pub fn all(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

pub struct FakeTimer {
    nanos: AtomicU64,
    events: Events,
}

impl FakeTimer {
    pub fn new(events: &Events) -> Self {
        Self {
            nanos: AtomicU64::new(0),
            events: events.clone(),
        }
    }

    pub fn advance(&self, by: Duration) {
        self.nanos.fetch_add(by.as_nanos() as u64, Ordering::SeqCst);
    }
}

impl Timer for FakeTimer {
    fn now(&self) -> Duration {
        Duration::from_nanos(self.nanos.load(Ordering::SeqCst))
    }

    fn sleep(&self, duration: Duration) {
        self.events.push("sleep");
        self.advance(duration);
    }
}

type SurveyScript = Box<dyn Fn(usize) -> io::Result<Vec<Finding>> + Send + Sync>;
type ReadScript = Box<dyn Fn(usize) -> io::Result<ProcessRead> + Send + Sync>;
type SignalHook = Box<dyn Fn(i32) + Send + Sync>;

pub struct ScriptedSurveyor {
    script: SurveyScript,
    calls: Arc<AtomicUsize>,
    events: Events,
    audit_dir: PathBuf,
    described: Arc<AtomicUsize>,
}

impl ScriptedSurveyor {
    pub fn fixed(findings: Vec<Finding>) -> Self {
        Self::scripted(move |_| Ok(findings.clone()))
    }

    pub fn scripted(script: impl Fn(usize) -> io::Result<Vec<Finding>> + Send + Sync + 'static) -> Self {
        Self {
            script: Box::new(script),
            calls: Arc::default(),
            events: Events::default(),
            audit_dir: PathBuf::new(),
            described: Arc::default(),
        }
    }

    pub fn described(&self) -> usize {
        self.described.load(Ordering::SeqCst)
    }
}

impl Surveyor for ScriptedSurveyor {
    fn survey(&self) -> io::Result<Vec<Finding>> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        self.events
            .push(format!("survey len={}", audit_len(&self.audit_dir)));
        (self.script)(call)
    }

    fn describe(&self, found: &Finding) -> ModelFinding {
        self.described.fetch_add(1, Ordering::SeqCst);
        describe(found)
    }
}

pub struct ScriptedProvider {
    script: ReadScript,
    reads: Arc<AtomicUsize>,
    events: Events,
    audit_dir: PathBuf,
}

impl ProcessProvider for ScriptedProvider {
    fn read(&self, _pid: i32) -> io::Result<ProcessRead> {
        let call = self.reads.fetch_add(1, Ordering::SeqCst);
        self.events
            .push(format!("read len={}", audit_len(&self.audit_dir)));
        (self.script)(call)
    }
}

pub struct RecordingSignaller {
    answer: SignalResult,
    signalled: Arc<Mutex<Vec<i32>>>,
    events: Events,
    audit_dir: PathBuf,
    hook: Option<SignalHook>,
}

impl Signaller for RecordingSignaller {
    fn sigterm(&self, pid: i32) -> SignalResult {
        self.events
            .push(format!("sigterm len={}", audit_len(&self.audit_dir)));
        self.signalled.lock().unwrap().push(pid);
        if let Some(hook) = &self.hook {
            hook(pid);
        }
        self.answer
    }
}

pub struct Rig {
    pub dir: TempDir,
    pub timer: Arc<FakeTimer>,
    pub events: Events,
    pub executor: Executor,
    signalled: Arc<Mutex<Vec<i32>>>,
    surveys: Arc<AtomicUsize>,
    reads: Arc<AtomicUsize>,
}

impl Rig {
    pub fn signals(&self) -> Vec<i32> {
        self.signalled.lock().unwrap().clone()
    }

    pub fn surveys(&self) -> usize {
        self.surveys.load(Ordering::SeqCst)
    }

    pub fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    pub fn locks(&self) -> PathBuf {
        self.dir.join("locks")
    }

    pub fn audit(&self) -> Vec<Value> {
        let Ok(text) = fs::read_to_string(self.dir.join(AUDIT_FILE)) else {
            return Vec::new();
        };
        text.lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    pub fn audit_results(&self) -> Vec<String> {
        self.audit()
            .iter()
            .filter(|entry| entry["phase"] == "result")
            .map(|entry| entry["result"].as_str().unwrap().to_owned())
            .collect()
    }
}

pub struct RigBuilder {
    dir: TempDir,
    survey: SurveyScript,
    read: ReadScript,
    answer: SignalResult,
    settings: Settings,
    hook: Option<SignalHook>,
}

impl Rig {
    pub fn for_item(item: &PlanItem) -> RigBuilder {
        let same = item.clone();
        let first = item.clone();
        RigBuilder {
            dir: TempDir::private("apply-rig"),
            survey: Box::new(move |_| {
                Ok(vec![Finding {
                    identity: RawIdentity {
                        kernel: same.identity.kernel.clone(),
                        exe_path: Some(same.identity.evidence.exe_path.clone()),
                        ppid: 1,
                        pgid: same.identity.kernel.pid,
                    },
                    class: same.model.class,
                    evidence: same.model.evidence.clone(),
                    age_us: 1,
                }])
            }),
            read: Box::new(move |call| {
                if call == 0 {
                    Ok(present(&first))
                } else {
                    Ok(ProcessRead::Gone)
                }
            }),
            answer: SignalResult::Delivered,
            settings: Settings::default(),
            hook: None,
        }
    }
}

impl RigBuilder {
    pub fn dir(&self) -> &Path {
        self.dir.path()
    }

    pub fn survey(
        mut self,
        script: impl Fn(usize) -> io::Result<Vec<Finding>> + Send + Sync + 'static,
    ) -> Self {
        self.survey = Box::new(script);
        self
    }

    pub fn reads(
        mut self,
        script: impl Fn(usize) -> io::Result<ProcessRead> + Send + Sync + 'static,
    ) -> Self {
        self.read = Box::new(script);
        self
    }

    pub fn answer(mut self, answer: SignalResult) -> Self {
        self.answer = answer;
        self
    }

    pub fn settings(mut self, edit: impl FnOnce(&mut Settings)) -> Self {
        edit(&mut self.settings);
        self
    }

    pub fn on_signal(mut self, hook: impl Fn(i32) + Send + Sync + 'static) -> Self {
        self.hook = Some(Box::new(hook));
        self
    }

    pub fn build(self) -> Rig {
        let dir = self.dir;
        let events = Events::default();
        let timer = Arc::new(FakeTimer::new(&events));
        let signalled = Arc::new(Mutex::new(Vec::new()));
        let surveys = Arc::new(AtomicUsize::new(0));
        let reads = Arc::new(AtomicUsize::new(0));
        let executor = Executor::new(
            Deps {
                data_dir: dir.path().to_path_buf(),
                surveyor: Arc::new(ScriptedSurveyor {
                    script: self.survey,
                    calls: Arc::clone(&surveys),
                    events: events.clone(),
                    audit_dir: dir.path().to_path_buf(),
                    described: Arc::default(),
                }),
                provider: Box::new(ScriptedProvider {
                    script: self.read,
                    reads: Arc::clone(&reads),
                    events: events.clone(),
                    audit_dir: dir.path().to_path_buf(),
                }),
                signaller: Box::new(RecordingSignaller {
                    answer: self.answer,
                    signalled: Arc::clone(&signalled),
                    events: events.clone(),
                    audit_dir: dir.path().to_path_buf(),
                    hook: self.hook,
                }),
                timer: Arc::clone(&timer) as Arc<dyn Timer>,
            },
            self.settings,
        );
        Rig {
            dir,
            timer,
            events,
            executor,
            signalled,
            surveys,
            reads,
        }
    }
}

pub type Hook = Box<dyn Fn(i32) + Send + Sync>;

struct WorldState {
    findings: Vec<Finding>,
    exited: std::collections::HashSet<i32>,
    stubborn: std::collections::HashSet<i32>,
    signalled: Vec<i32>,
}

pub struct World {
    state: Mutex<WorldState>,
}

impl World {
    pub fn new(findings: Vec<Finding>) -> Arc<World> {
        Arc::new(World {
            state: Mutex::new(WorldState {
                findings,
                exited: Default::default(),
                stubborn: Default::default(),
                signalled: Vec::new(),
            }),
        })
    }

    pub fn exit(&self, pid: i32) {
        self.state.lock().unwrap().exited.insert(pid);
    }

    pub fn stubborn(&self, pid: i32) {
        self.state.lock().unwrap().stubborn.insert(pid);
    }

    pub fn set_class(&self, pid: i32, class: Class) {
        let mut state = self.state.lock().unwrap();
        for found in state.findings.iter_mut().filter(|f| f.identity.kernel.pid == pid) {
            found.class = class;
            found.evidence = evidence_of(class);
        }
    }

    pub fn set_path(&self, pid: i32, exe: &str) {
        let mut state = self.state.lock().unwrap();
        for found in state.findings.iter_mut().filter(|f| f.identity.kernel.pid == pid) {
            found.identity.exe_path = Some(exe.into());
        }
    }

    pub fn replace(&self, pid: i32) {
        let mut state = self.state.lock().unwrap();
        for found in state.findings.iter_mut().filter(|f| f.identity.kernel.pid == pid) {
            found.identity.kernel.start_time_us += 1;
        }
    }

    pub fn alive(&self, pid: i32) -> bool {
        let state = self.state.lock().unwrap();
        !state.exited.contains(&pid) && state.findings.iter().any(|f| f.identity.kernel.pid == pid)
    }

    pub fn signals(&self) -> Vec<i32> {
        self.state.lock().unwrap().signalled.clone()
    }

    pub fn surveyor(self: &Arc<Self>) -> Arc<dyn Surveyor> {
        Arc::new(WorldSurvey(Arc::clone(self)))
    }

    pub fn provider(self: &Arc<Self>) -> Box<dyn ProcessProvider + Send + Sync> {
        Box::new(WorldProvider(Arc::clone(self)))
    }

    pub fn signaller(self: &Arc<Self>, hook: Option<Hook>) -> Box<dyn Signaller> {
        Box::new(WorldSignaller(Arc::clone(self), hook))
    }
}

struct WorldSurvey(Arc<World>);
struct WorldProvider(Arc<World>);
struct WorldSignaller(Arc<World>, Option<Hook>);

impl Surveyor for WorldSurvey {
    fn survey(&self) -> io::Result<Vec<Finding>> {
        let state = self.0.state.lock().unwrap();
        Ok(state
            .findings
            .iter()
            .filter(|f| !state.exited.contains(&f.identity.kernel.pid))
            .cloned()
            .collect())
    }

    fn describe(&self, found: &Finding) -> ModelFinding {
        describe(found)
    }
}

impl ProcessProvider for WorldProvider {
    fn read(&self, pid: i32) -> io::Result<ProcessRead> {
        let state = self.0.state.lock().unwrap();
        if state.exited.contains(&pid) {
            return Ok(ProcessRead::Gone);
        }
        Ok(state
            .findings
            .iter()
            .find(|f| f.identity.kernel.pid == pid)
            .map_or(ProcessRead::Gone, |found| {
                ProcessRead::Present(agentdust_core::identity::ProcessIdentity {
                    kernel: found.identity.kernel.clone(),
                    evidence: agentdust_core::identity::IdentityEvidence {
                        exe_path: found.identity.exe_path.clone().unwrap_or_default(),
                    },
                })
            }))
    }
}

impl Signaller for WorldSignaller {
    fn sigterm(&self, pid: i32) -> SignalResult {
        if let Some(hook) = &self.1 {
            hook(pid);
        }
        let mut state = self.0.state.lock().unwrap();
        state.signalled.push(pid);
        let exists =
            !state.exited.contains(&pid) && state.findings.iter().any(|f| f.identity.kernel.pid == pid);
        if !exists {
            return SignalResult::NoSuchProcess;
        }
        if !state.stubborn.contains(&pid) {
            state.exited.insert(pid);
        }
        SignalResult::Delivered
    }
}

pub struct ServerRig {
    pub dir: TempDir,
    pub timer: Arc<FakeTimer>,
    pub world: Arc<World>,
    pub server: Arc<agentdust_core::apply::server::Server>,
}

impl ServerRig {
    pub fn new(findings: Vec<Finding>) -> Self {
        Self::hooked(findings, None)
    }

    pub fn hooked(findings: Vec<Finding>, hook: Option<Hook>) -> Self {
        let dir = TempDir::private("apply-server");
        let timer = Arc::new(FakeTimer::new(&Events::default()));
        let world = World::new(findings);
        let server = Self::server_on(&dir, &timer, &world, hook);
        Self {
            dir,
            timer,
            world,
            server,
        }
    }

    pub fn server_on(
        dir: &TempDir,
        timer: &Arc<FakeTimer>,
        world: &Arc<World>,
        hook: Option<Hook>,
    ) -> Arc<agentdust_core::apply::server::Server> {
        Arc::new(agentdust_core::apply::server::Server::new(
            Deps {
                data_dir: dir.path().to_path_buf(),
                surveyor: world.surveyor(),
                provider: world.provider(),
                signaller: world.signaller(hook),
                timer: Arc::clone(timer) as Arc<dyn Timer>,
            },
            Settings::default(),
        ))
    }

    pub fn another_server(&self, hook: Option<Hook>) -> Arc<agentdust_core::apply::server::Server> {
        Self::server_on(&self.dir, &self.timer, &self.world, hook)
    }

    pub fn locks(&self) -> PathBuf {
        self.dir.join("locks")
    }

    pub fn audit(&self) -> Vec<Value> {
        let Ok(text) = fs::read_to_string(self.dir.join(AUDIT_FILE)) else {
            return Vec::new();
        };
        text.lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

pub fn code_of(challenge: &agentdust_core::apply::server::Challenge) -> String {
    let at = challenge
        .message
        .find("Type ")
        .expect("the prompt says what to type");
    challenge.message[at + 5..at + 9].to_owned()
}

pub fn accept(
    challenge: &agentdust_core::apply::server::Challenge,
) -> agentdust_core::apply::server::Response {
    agentdust_core::apply::server::Response::Accept(Some(code_of(challenge)))
}

pub fn call(plan_id: &str, items: &[&ModelFinding]) -> agentdust_core::apply::server::Call {
    agentdust_core::apply::server::Call {
        plan_id: plan_id.to_owned(),
        item_ids: items.iter().map(|item| item.item_id.clone()).collect(),
    }
}
