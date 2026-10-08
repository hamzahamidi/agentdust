mod apply_support;
mod scratch;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use agentdust_core::automatic::{self, PolicyGuard};
use agentdust_core::class::Class;
use agentdust_core::session::{Liveness, LivenessProbe};
use apply_support::{item, kernel};
use scratch::TempDir;

#[test]
fn absent_policy_is_disabled_and_read_only() {
    let dir = TempDir::absent("auto-default");
    assert!(!automatic::read(&dir).unwrap().enabled);
    let status = automatic::status(&dir).unwrap();
    assert_eq!(status["enabled"], false);
    assert_eq!(status["worker_running"], false);
    assert!(!dir.exists());
}

#[test]
fn policy_round_trip_is_private_and_preserves_exact_keeps() {
    let dir = TempDir::private("auto-policy");
    let mut guard = PolicyGuard::acquire(&dir).unwrap();
    guard.policy.enabled = true;
    guard.policy.projects.push("a".repeat(64).try_into().unwrap());
    guard.policy.keep.push(kernel(4242));
    guard.write().unwrap();
    let read = automatic::read(&dir).unwrap();
    assert_eq!(read, guard.policy);
    let path = dir.join("automatic/policy.json");
    assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(
        fs::metadata(dir.join("automatic")).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let mut reused = kernel(4242);
    reused.start_time_us += 1;
    assert!(!read.keep.contains(&reused));
}

#[test]
fn unsafe_and_unrecognized_policy_files_cannot_grant_cleanup() {
    for text in [
        "{}",
        "{\"version\":2,\"enabled\":true,\"projects\":[],\"keep\":[]}",
        "{\"version\":1,\"enabled\":true,\"projects\":[],\"keep\":[],\"other\":true}",
    ] {
        let dir = TempDir::private("auto-invalid");
        let mut guard = PolicyGuard::acquire(&dir).unwrap();
        guard.write().unwrap();
        fs::write(dir.join("automatic/policy.json"), text).unwrap();
        assert!(automatic::read(&dir).is_err());
    }
    let dir = TempDir::private("auto-mode");
    let mut guard = PolicyGuard::acquire(&dir).unwrap();
    guard.write().unwrap();
    fs::set_permissions(
        dir.join("automatic/policy.json"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(automatic::read(&dir).is_err());
}

#[test]
fn policy_symlinks_and_hard_links_are_refused() {
    let dir = TempDir::private("auto-links");
    let mut guard = PolicyGuard::acquire(&dir).unwrap();
    guard.write().unwrap();
    let path = dir.join("automatic/policy.json");
    fs::hard_link(&path, dir.join("copy")).unwrap();
    assert!(automatic::read(&dir).is_err());
    fs::remove_file(dir.join("copy")).unwrap();
    fs::rename(&path, dir.join("real")).unwrap();
    symlink(dir.join("real"), &path).unwrap();
    assert!(automatic::read(&dir).is_err());
}

#[test]
fn pause_serializes_with_an_in_flight_policy_holder() {
    let dir = TempDir::private("auto-serialize");
    let mut action = PolicyGuard::acquire(&dir).unwrap();
    action.policy.enabled = true;
    action.write().unwrap();
    let path = dir.path().to_owned();
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let pause = thread::spawn(move || {
        started_tx.send(()).unwrap();
        let mut guard = PolicyGuard::acquire(&path).unwrap();
        guard.policy.enabled = false;
        guard.write().unwrap();
        done_tx.send(()).unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(done_rx.try_recv().is_err());
    drop(action);
    done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    pause.join().unwrap();
    assert!(!automatic::read(&dir).unwrap().enabled);
}

#[test]
fn receipts_survive_restart_and_do_not_follow_a_reused_pid() {
    let dir = TempDir::private("auto-receipt");
    let candidate = item(4242, Class::OwnedEnded);
    let guard = PolicyGuard::acquire(&dir).unwrap();
    assert!(guard.claim_attempt(&candidate).unwrap());
    drop(guard);
    let guard = PolicyGuard::acquire(&dir).unwrap();
    assert!(!guard.claim_attempt(&candidate).unwrap());
    let mut reused = candidate.clone();
    reused.identity.kernel.start_time_us += 1;
    assert!(guard.claim_attempt(&reused).unwrap());
    assert_eq!(fs::read_dir(dir.join("automatic/attempts")).unwrap().count(), 2);
}

struct Probe(Liveness);
impl LivenessProbe for Probe {
    fn probe(&self, _identity: &agentdust_core::identity::KernelIdentity) -> Liveness {
        self.0
    }
}

#[test]
fn only_proven_gone_receipts_are_pruned() {
    let dir = TempDir::private("auto-prune");
    let candidate = item(4242, Class::OwnedEnded);
    let guard = PolicyGuard::acquire(&dir).unwrap();
    guard.claim_attempt(&candidate).unwrap();
    automatic::prune_attempts(&dir, &Probe(Liveness::Unknown)).unwrap();
    assert!(!guard.claim_attempt(&candidate).unwrap());
    automatic::prune_attempts(&dir, &Probe(Liveness::Alive)).unwrap();
    assert!(!guard.claim_attempt(&candidate).unwrap());
    automatic::prune_attempts(&dir, &Probe(Liveness::Gone)).unwrap();
    assert_eq!(fs::read_dir(dir.join("automatic/attempts")).unwrap().count(), 0);
}

#[test]
fn keep_blocks_the_existing_manual_executor() {
    let candidate = item(4242, Class::OwnedEnded);
    let rig = apply_support::Rig::for_item(&candidate).build();
    let mut guard = PolicyGuard::acquire(&rig.dir).unwrap();
    guard.policy.keep.push(candidate.identity.kernel.clone());
    guard.write().unwrap();
    drop(guard);
    assert_eq!(
        rig.executor.execute(apply_support::PLAN, &candidate).outcome,
        agentdust_core::apply::exec::Outcome::Disabled
    );
    assert!(rig.signals().is_empty());
}

#[cfg(target_os = "macos")]
mod execution {
    use super::*;
    use agentdust_core::apply::exec::{Outcome, Reason};
    use agentdust_core::journal::{self, Agent, AgentIdentity, Kind, Record, SCHEMA_VERSION};
    use agentdust_core::plan::PlanItem;
    use apply_support::{Rig, finding, present};

    fn enable(rig: &Rig, candidate: &PlanItem) {
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
    }

    #[test]
    fn automatic_cleanup_uses_the_existing_executor_and_audit_namespace() {
        let candidate = item(4242, Class::OwnedEnded);
        let rig = Rig::for_item(&candidate).build();
        enable(&rig, &candidate);
        assert_eq!(
            rig.executor
                .execute_automatic("auto-test", &candidate)
                .unwrap()
                .outcome,
            Outcome::Terminated
        );
        assert_eq!(rig.signals(), [4242]);
        assert!(rig.audit().iter().all(|line| line["plan"] == "auto-test"));
    }

    #[test]
    fn survivor_receipt_prevents_a_second_automatic_signal() {
        let candidate = item(4242, Class::OwnedEnded);
        let same = present(&candidate);
        let rig = Rig::for_item(&candidate).reads(move |_| Ok(same.clone())).build();
        enable(&rig, &candidate);
        assert_eq!(
            rig.executor
                .execute_automatic("auto-one", &candidate)
                .unwrap()
                .outcome,
            Outcome::Survivor
        );
        assert_eq!(
            rig.executor
                .execute_automatic("auto-two", &candidate)
                .unwrap()
                .outcome,
            Outcome::HandledElsewhere
        );
        assert_eq!(rig.signals(), [4242]);
    }

    #[test]
    fn paused_kept_and_unselected_processes_receive_no_signal() {
        for mode in ["pause", "keep", "scope"] {
            let candidate = item(4242, Class::OwnedEnded);
            let rig = Rig::for_item(&candidate).build();
            enable(&rig, &candidate);
            let mut guard = PolicyGuard::acquire(&rig.dir).unwrap();
            match mode {
                "pause" => guard.policy.enabled = false,
                "keep" => guard.policy.keep.push(candidate.identity.kernel.clone()),
                _ => guard.policy.projects.clear(),
            }
            guard.write().unwrap();
            drop(guard);
            assert!(rig.executor.execute_automatic("auto-no", &candidate).is_err());
            assert!(rig.signals().is_empty());
        }
    }

    #[test]
    fn live_owner_class_change_and_changed_target_identity_abort() {
        let candidate = item(4242, Class::OwnedEnded);
        let rig = Rig::for_item(&candidate)
            .survey(|_| Ok(vec![finding(4242, Class::OwnedLive)]))
            .build();
        enable(&rig, &candidate);
        let result = rig.executor.execute_automatic("auto-live", &candidate).unwrap();
        assert_eq!(result.reason, Some(Reason::ClassChanged));
        assert!(rig.signals().is_empty());
        let mut different = candidate.identity.clone();
        different.kernel.start_time_us += 1;
        let rig = Rig::for_item(&candidate)
            .reads(move |_| Ok(agentdust_core::provider::ProcessRead::Present(different.clone())))
            .build();
        enable(&rig, &candidate);
        assert_eq!(
            rig.executor
                .execute_automatic("auto-reuse", &candidate)
                .unwrap()
                .reason,
            Some(Reason::IdentityChanged)
        );
        assert!(rig.signals().is_empty());
    }

    #[test]
    fn cross_session_ownership_cannot_be_automatic() {
        let mut candidate = item(4242, Class::OwnedEnded);
        candidate
            .attribution_owners
            .as_mut()
            .unwrap()
            .push(agentdust_core::classifier::AttributionOwner {
                agent: Agent::Claude,
                session_id: "other-session".into(),
                identity: kernel(9876),
            });
        let rig = Rig::for_item(&candidate).build();
        enable(&rig, &candidate);
        assert_eq!(
            rig.executor
                .execute_automatic("auto-shared", &candidate)
                .unwrap_err(),
            "shared_ownership"
        );
        assert!(rig.signals().is_empty());
    }

    #[test]
    fn suspect_never_enters_automatic_cleanup() {
        let ended = item(4242, Class::OwnedEnded);
        let candidate = item(4242, Class::Suspect);
        let rig = Rig::for_item(&candidate).build();
        enable(&rig, &ended);
        assert_eq!(
            rig.executor
                .execute_automatic("auto-suspect", &candidate)
                .unwrap_err(),
            "approval_required"
        );
        assert!(rig.signals().is_empty());
    }
}
