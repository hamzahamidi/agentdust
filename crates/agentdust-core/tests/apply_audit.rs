mod scratch;

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::thread;

use agentdust_core::apply::audit::{AUDIT_FILE, AUDIT_KEYS, AUDIT_MAX_BYTES, AuditError, AuditLog, Entry};
use agentdust_core::class::Class;
use agentdust_core::classifier::{AttributionOwner, Evidence, Finding};
use agentdust_core::cwd::CwdRelation;
use agentdust_core::finding::ModelFinding;
use agentdust_core::identity::KernelIdentity;
use agentdust_core::inventory::RawIdentity;
use agentdust_core::journal::Agent;
use scratch::TempDir;
use serde_json::Value;

const PLAN: &str = "0123456789abcdef0123456789abcdef";

fn kernel(pid: i32) -> KernelIdentity {
    KernelIdentity {
        boot_session_uuid: "boot-1".to_owned(),
        pid,
        start_time_us: 1_800_000_000_000_000 + pid as u64,
        uid: 501,
    }
}

fn model(pid: i32, class: Class, evidence: Vec<Evidence>, exe: &str) -> ModelFinding {
    let finding = Finding {
        identity: RawIdentity {
            kernel: kernel(pid),
            exe_path: Some(exe.into()),
            ppid: 1,
            pgid: pid,
        },
        class,
        evidence,
        age_us: 7_500_000_000,
        attribution_owners: (class == Class::OwnedEnded).then(|| {
            vec![AttributionOwner {
                agent: Agent::Claude,
                session_id: format!("session-{pid}"),
                identity: kernel(pid + 100_000),
            }]
        }),
    };
    ModelFinding::new(&finding, CwdRelation::Other)
}

fn owned(pid: i32) -> ModelFinding {
    model(
        pid,
        Class::OwnedEnded,
        vec![Evidence::OwnedTag, Evidence::OwnedAgentGone],
        "/opt/homebrew/bin/node",
    )
}

fn lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn parsed(path: &Path) -> Vec<Value> {
    lines(path)
        .iter()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn mode_of(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

#[test]
fn the_limit_is_5_mib_and_the_file_is_audit_log() {
    assert_eq!(AUDIT_MAX_BYTES, 5 * 1024 * 1024);
    assert_eq!(AUDIT_FILE, "audit.log");
}

#[test]
fn a_result_entry_is_one_json_line_with_the_fixed_keys() {
    let dir = TempDir::absent("audit-one");
    let log = AuditLog::new(&dir, AUDIT_MAX_BYTES);
    let item = owned(4242);
    log.append(&Entry::result(PLAN, &item, &kernel(4242), "terminated", None))
        .unwrap();

    let path = dir.join(AUDIT_FILE);
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.ends_with('\n'));
    assert_eq!(text.lines().count(), 1);
    assert!(text.starts_with("{\"v\":1,\"wall_ms\":"), "{text}");

    let value: Value = serde_json::from_str(text.trim_end()).unwrap();
    let object = value.as_object().unwrap();
    let keys: BTreeSet<&str> = object.keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = AUDIT_KEYS.iter().copied().collect();
    assert_eq!(keys, expected);
    assert_eq!(AUDIT_KEYS.len(), 13);
    assert_eq!(object["v"], 1);
    assert!(object["wall_ms"].as_u64().unwrap() > 1_700_000_000_000);
    assert_eq!(object["plan"], PLAN);
    assert_eq!(object["item"], item.item_id);
    assert_eq!(object["pid"], 4242);
    assert_eq!(object["start_us"], 1_800_000_000_004_242u64);
    assert_eq!(object["uid"], 501);
    assert_eq!(object["class"], "owned-ended");
    assert_eq!(
        object["evidence"],
        serde_json::json!(["owned.tag", "owned.agent_gone"])
    );
    assert_eq!(object["exe_base"], "node");
    assert_eq!(object["phase"], "result");
    assert_eq!(object["result"], "terminated");
    assert_eq!(object["reason"], Value::Null);
}

#[test]
fn an_attempt_entry_has_no_result_yet_and_a_reason_is_recorded() {
    let dir = TempDir::absent("audit-two");
    let log = AuditLog::new(&dir, AUDIT_MAX_BYTES);
    let item = owned(7);
    log.append(&Entry::attempt(PLAN, &item, &kernel(7))).unwrap();
    log.append(&Entry::result(
        PLAN,
        &item,
        &kernel(7),
        "revalidation_failed",
        Some("class_changed"),
    ))
    .unwrap();
    let entries = parsed(&dir.join(AUDIT_FILE));
    assert_eq!(entries[0]["phase"], "attempt");
    assert_eq!(entries[0]["result"], Value::Null);
    assert_eq!(entries[1]["phase"], "result");
    assert_eq!(entries[1]["result"], "revalidation_failed");
    assert_eq!(entries[1]["reason"], "class_changed");
}

#[test]
fn an_executable_name_that_is_not_plain_is_recorded_as_null() {
    let dir = TempDir::absent("audit-exe");
    let log = AuditLog::new(&dir, AUDIT_MAX_BYTES);
    let item = model(
        9,
        Class::Suspect,
        vec![Evidence::SuspectAge],
        "/Users/someone/repo/evil name\u{1b}[31m",
    );
    log.append(&Entry::result(PLAN, &item, &kernel(9), "gone", None))
        .unwrap();
    let entries = parsed(&dir.join(AUDIT_FILE));
    assert_eq!(entries[0]["exe_base"], Value::Null);
    assert_eq!(entries[0]["class"], "suspect");
    let text = fs::read_to_string(dir.join(AUDIT_FILE)).unwrap();
    assert!(!text.contains("someone"));
    assert!(!text.contains("/Users"));
}

#[test]
fn the_directory_and_file_are_created_private_and_entries_append() {
    let dir = TempDir::absent("audit-modes");
    let log = AuditLog::new(&dir, AUDIT_MAX_BYTES);
    for pid in 1..=3 {
        log.append(&Entry::attempt(PLAN, &owned(pid + 100), &kernel(pid + 100)))
            .unwrap();
    }
    assert_eq!(mode_of(&dir), 0o700);
    assert_eq!(mode_of(&dir.join(AUDIT_FILE)), 0o600);
    assert_eq!(lines(&dir.join(AUDIT_FILE)).len(), 3);
}

#[test]
fn a_full_file_moves_to_one_older_generation_and_stays_within_the_bound() {
    let dir = TempDir::absent("audit-rotate");
    let limit = 1000;
    let log = AuditLog::new(&dir, limit);
    for pid in 1..=40 {
        log.append(&Entry::result(
            PLAN,
            &owned(pid),
            &kernel(pid),
            "terminated",
            None,
        ))
        .unwrap();
        let size = fs::metadata(dir.join(AUDIT_FILE)).unwrap().len();
        assert!(size <= limit, "{size} bytes after entry {pid}");
    }
    let older = dir.join("audit.log.1");
    assert!(older.exists());
    assert_eq!(mode_of(&older), 0o600);
    assert!(!dir.join("audit.log.2").exists());
    let newest = parsed(&dir.join(AUDIT_FILE));
    assert_eq!(newest.last().unwrap()["pid"], 40);
    let previous = parsed(&older);
    let last_of_previous = previous.last().unwrap()["pid"].as_i64().unwrap();
    assert_eq!(
        newest.first().unwrap()["pid"].as_i64().unwrap(),
        last_of_previous + 1
    );
}

#[test]
fn one_entry_larger_than_the_limit_is_still_written_to_an_empty_file() {
    let dir = TempDir::absent("audit-tiny");
    let log = AuditLog::new(&dir, 10);
    log.append(&Entry::result(PLAN, &owned(1), &kernel(1), "gone", None))
        .unwrap();
    log.append(&Entry::result(PLAN, &owned(2), &kernel(2), "gone", None))
        .unwrap();
    assert_eq!(lines(&dir.join(AUDIT_FILE)).len(), 1);
    assert_eq!(parsed(&dir.join(AUDIT_FILE))[0]["pid"], 2);
    assert_eq!(parsed(&dir.join("audit.log.1"))[0]["pid"], 1);
}

#[test]
fn writers_in_several_threads_never_interleave_lines() {
    let dir = TempDir::absent("audit-threads");
    thread::scope(|scope| {
        for worker in 0..4 {
            let dir = &dir;
            scope.spawn(move || {
                let log = AuditLog::new(dir, AUDIT_MAX_BYTES);
                for step in 0..40 {
                    let pid = worker * 1000 + step + 1;
                    log.append(&Entry::result(
                        PLAN,
                        &owned(pid),
                        &kernel(pid),
                        "terminated",
                        None,
                    ))
                    .unwrap();
                }
            });
        }
    });
    let entries = parsed(&dir.join(AUDIT_FILE));
    assert_eq!(entries.len(), 160);
    let pids: BTreeSet<i64> = entries
        .iter()
        .map(|entry| entry["pid"].as_i64().unwrap())
        .collect();
    assert_eq!(pids.len(), 160);
}

#[test]
fn writers_that_rotate_together_leave_only_whole_lines_within_the_bound() {
    let dir = TempDir::absent("audit-threads-rotate");
    let limit = 3000;
    thread::scope(|scope| {
        for worker in 0..4 {
            let dir = &dir;
            scope.spawn(move || {
                let log = AuditLog::new(dir, limit);
                for step in 0..60 {
                    let pid = worker * 1000 + step + 1;
                    log.append(&Entry::result(
                        PLAN,
                        &owned(pid),
                        &kernel(pid),
                        "terminated",
                        None,
                    ))
                    .unwrap();
                }
            });
        }
    });
    for name in ["audit.log", "audit.log.1"] {
        let path = dir.join(name);
        assert!(fs::metadata(&path).unwrap().len() <= limit, "{name}");
        assert!(!parsed(&path).is_empty());
    }
    assert!(!dir.join("audit.log.2").exists());
}

#[test]
fn unsafe_files_make_the_append_refuse_and_write_nothing() {
    let dir = TempDir::private("audit-unsafe");
    let log = AuditLog::new(&dir, AUDIT_MAX_BYTES);
    let entry = Entry::attempt(PLAN, &owned(1), &kernel(1));

    let elsewhere = dir.join("elsewhere");
    fs::write(&elsewhere, b"keep").unwrap();
    fs::set_permissions(&elsewhere, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&elsewhere, dir.join(AUDIT_FILE)).unwrap();
    assert!(matches!(log.append(&entry), Err(AuditError::Refused(_))));
    assert_eq!(fs::read(&elsewhere).unwrap(), b"keep");
    fs::remove_file(dir.join(AUDIT_FILE)).unwrap();

    fs::write(dir.join(AUDIT_FILE), b"").unwrap();
    fs::set_permissions(dir.join(AUDIT_FILE), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(log.append(&entry), Err(AuditError::Refused(_))));
    fs::remove_file(dir.join(AUDIT_FILE)).unwrap();

    fs::write(dir.join(AUDIT_FILE), b"").unwrap();
    fs::set_permissions(dir.join(AUDIT_FILE), fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(dir.join(AUDIT_FILE), dir.join("second-name")).unwrap();
    assert!(matches!(log.append(&entry), Err(AuditError::Refused(_))));
    fs::remove_file(dir.join("second-name")).unwrap();
    assert_eq!(fs::read(dir.join(AUDIT_FILE)).unwrap(), b"");
    fs::remove_file(dir.join(AUDIT_FILE)).unwrap();

    fs::remove_file(dir.join("audit.lock")).unwrap();
    symlink(&elsewhere, dir.join("audit.lock")).unwrap();
    assert!(matches!(log.append(&entry), Err(AuditError::Refused(_))));
    assert!(!dir.join(AUDIT_FILE).exists());
}

#[test]
fn a_data_directory_that_is_a_link_or_loose_makes_the_append_refuse() {
    let real = TempDir::private("audit-real");
    let parent = TempDir::private("audit-parent");
    let link = parent.join("data");
    symlink(real.path(), &link).unwrap();
    let entry = Entry::attempt(PLAN, &owned(1), &kernel(1));
    assert!(matches!(
        AuditLog::new(&link, AUDIT_MAX_BYTES).append(&entry),
        Err(AuditError::Refused(_))
    ));

    let loose = TempDir::private("audit-loose");
    fs::set_permissions(loose.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        AuditLog::new(&loose, AUDIT_MAX_BYTES).append(&entry),
        Err(AuditError::Refused(_))
    ));
    assert!(!loose.join(AUDIT_FILE).exists());
}

#[test]
fn a_rotated_generation_that_is_a_link_is_replaced_not_followed() {
    let dir = TempDir::private("audit-gen-link");
    let elsewhere = dir.join("elsewhere");
    fs::write(&elsewhere, b"keep").unwrap();
    symlink(&elsewhere, dir.join("audit.log.1")).unwrap();
    let log = AuditLog::new(&dir, 10);
    for pid in 1..=3 {
        log.append(&Entry::result(PLAN, &owned(pid), &kernel(pid), "gone", None))
            .unwrap();
    }
    assert_eq!(fs::read(&elsewhere).unwrap(), b"keep");
    assert!(
        !fs::symlink_metadata(dir.join("audit.log.1"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}
