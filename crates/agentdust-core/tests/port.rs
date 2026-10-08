mod apply_support;
mod scratch;

use std::io;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use agentdust_core::class::Class;
use agentdust_core::identity::ProcessIdentity;
use agentdust_core::port::{self, ListenerSource, PortSurveyor};
use agentdust_core::survey::Surveyor;
use apply_support::{Rig, ScriptedSurveyor, finding, item};

struct Listeners {
    calls: AtomicUsize,
    script: Box<dyn Fn(usize) -> io::Result<Vec<ProcessIdentity>> + Send + Sync>,
}
impl Listeners {
    fn new(script: impl Fn(usize) -> io::Result<Vec<ProcessIdentity>> + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            script: Box::new(script),
        })
    }
}
impl ListenerSource for Listeners {
    fn listeners(&self, _: u16) -> io::Result<Vec<ProcessIdentity>> {
        (self.script)(self.calls.fetch_add(1, Ordering::SeqCst))
    }
}

#[test]
fn parses_ipv4_ipv6_and_deduplicates_multiple_sockets() {
    assert_eq!(
        port::parse_lsof(
            b"p42\0\nf3\0n127.0.0.1:3000\0\nf4\0n[::1]:3000\0\np43\0\nf5\0n*:3000\0\n",
            3000
        )
        .unwrap(),
        [42, 43]
    );
    assert!(port::parse_lsof(b"", 3000).unwrap().is_empty());
}

#[test]
fn refuses_malformed_truncated_wrong_port_and_oversized_lists() {
    for bytes in [
        b"p42\0\nf3\0n*:3000".as_slice(),
        b"p42\0\nf3\0n*:3001\0\n",
        b"p0\0\nf3\0n*:3000\0\n",
        b"n*:3000\0\n",
        b"p42\0\n",
        b"p42\0\nxsecret\0\n",
    ] {
        assert!(port::parse_lsof(bytes, 3000).is_err());
    }
    let bytes = (1..=17)
        .map(|pid| format!("p{pid}\0\nf3\0n*:3000\0\n"))
        .collect::<String>();
    assert!(port::parse_lsof(bytes.as_bytes(), 3000).is_err());
}

#[test]
fn complete_classification_is_preserved_before_listener_filtering() {
    let candidate = item(42, Class::OwnedEnded);
    let identity = candidate.identity;
    let processes = Arc::new(ScriptedSurveyor::fixed(vec![
        finding(42, Class::OwnedLive),
        finding(43, Class::Managed),
    ]));
    let surveyor = PortSurveyor {
        port: 3000,
        listeners: Listeners::new(move |_| Ok(vec![identity.clone()])),
        processes,
    };
    let found = surveyor.survey().unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].class, Class::OwnedLive);
}

#[test]
fn changing_pid_start_time_path_or_membership_cannot_match_a_listener() {
    for mode in ["pid", "start", "path", "gone"] {
        let identity = item(42, Class::OwnedEnded).identity;
        let changed = identity.clone();
        let listeners = Listeners::new(move |call| {
            let mut identity = changed.clone();
            if call > 0 {
                match mode {
                    "pid" => identity.kernel.pid += 1,
                    "start" => identity.kernel.start_time_us += 1,
                    "path" => identity.evidence.exe_path = "/other".into(),
                    _ => return Ok(vec![]),
                }
            }
            Ok(vec![identity])
        });
        let surveyor = PortSurveyor {
            port: 3000,
            listeners,
            processes: Arc::new(ScriptedSurveyor::fixed(vec![finding(42, Class::OwnedEnded)])),
        };
        assert!(surveyor.survey().unwrap().is_empty());
    }
}

#[test]
fn unavailable_scan_never_signals_and_does_not_claim_no_listener() {
    let candidate = item(42, Class::OwnedEnded);
    let rig = Rig::for_item(&candidate).build();
    let surveyor = PortSurveyor {
        port: 3000,
        listeners: Listeners::new(|_| Err(io::Error::other("partial"))),
        processes: Arc::new(ScriptedSurveyor::fixed(vec![finding(42, Class::OwnedEnded)])),
    };
    let report = port::run(3000, true, &surveyor, &rig.executor).unwrap();
    assert_eq!(report.state, "unavailable");
    assert!(report.error.is_some());
    assert!(rig.signals().is_empty());
}

#[test]
fn read_only_diagnosis_includes_all_classes_without_signals() {
    for class in [
        Class::OwnedEnded,
        Class::Suspect,
        Class::OwnedLive,
        Class::Managed,
        Class::Unknown,
        Class::LikelyOwned,
    ] {
        let candidate = item(42, Class::OwnedEnded);
        let identity = candidate.identity.clone();
        let rig = Rig::for_item(&candidate).build();
        let surveyor = PortSurveyor {
            port: 3000,
            listeners: Listeners::new(move |_| Ok(vec![identity.clone()])),
            processes: Arc::new(ScriptedSurveyor::fixed(vec![finding(42, class)])),
        };
        let report = port::run(3000, false, &surveyor, &rig.executor).unwrap();
        assert_eq!(report.state, "listening");
        assert_eq!(report.listeners[0].finding.as_ref().unwrap().class, class);
        assert!(rig.signals().is_empty());
    }
}

#[test]
fn failed_verification_is_unavailable_even_after_a_valid_diagnosis() {
    let candidate = item(42, Class::OwnedEnded);
    let identity = candidate.identity.clone();
    let rig = Rig::for_item(&candidate).build();
    let surveyor = PortSurveyor {
        port: 3000,
        listeners: Listeners::new(move |call| {
            if call == 3 {
                Err(io::Error::other("partial"))
            } else {
                Ok(vec![identity.clone()])
            }
        }),
        processes: Arc::new(ScriptedSurveyor::fixed(vec![finding(42, Class::OwnedEnded)])),
    };
    let report = port::run(3000, false, &surveyor, &rig.executor).unwrap();
    assert_eq!(report.state, "unavailable");
    assert_eq!(report.error, Some("verification_unavailable"));
    assert!(rig.signals().is_empty());
}

#[test]
fn executor_refresh_rejects_a_listener_that_stopped_listening() {
    use agentdust_core::automatic::PolicyGuard;
    use agentdust_core::journal::{self, Agent, AgentIdentity, Kind, Record, SCHEMA_VERSION};
    let candidate = item(42, Class::OwnedEnded);
    let identity = candidate.identity.clone();
    let surveyor = Arc::new(PortSurveyor {
        port: 3000,
        listeners: Listeners::new(move |call| {
            if call >= 3 {
                Ok(vec![])
            } else {
                Ok(vec![identity.clone()])
            }
        }),
        processes: Arc::new(ScriptedSurveyor::fixed(vec![finding(42, Class::OwnedEnded)])),
    });
    let fresh = surveyor.clone();
    let rig = Rig::for_item(&candidate).survey(move |_| fresh.survey()).build();
    let secret = agentdust_core::secret::load_or_create(&rig.dir).unwrap();
    let key = agentdust_core::cwd::cwd_key(&secret, rig.dir.to_str().unwrap()).unwrap();
    let owner = &candidate.attribution_owners.as_ref().unwrap()[0];
    journal::append(
        &rig.dir,
        &Record {
            v: SCHEMA_VERSION,
            kind: Kind::SessionStart,
            agent: Agent::Claude,
            session_id: owner.session_id.clone(),
            subagent_id: None,
            agent_identity: Some(
                AgentIdentity::new(
                    owner.identity.pid,
                    owner.identity.start_time_us,
                    owner.identity.uid,
                    None,
                )
                .unwrap(),
            ),
            tool_use_id: None,
            wall_ts: 1,
            mono_ts: 1,
            boot: owner.identity.boot_session_uuid.clone(),
            session_tag_key: None,
            cwd_key: Some(key.clone()),
            exe_base: None,
        },
    )
    .unwrap();
    let mut guard = PolicyGuard::acquire(&rig.dir).unwrap();
    guard.policy.enabled = true;
    guard.policy.projects.push(key);
    guard.write().unwrap();
    drop(guard);
    let report = port::run(3000, true, &surveyor, &rig.executor).unwrap();
    assert_eq!(report.state, "no_visible_listener");
    assert!(rig.signals().is_empty());
    assert_ne!(report.listeners[0].result, "terminated");
}
