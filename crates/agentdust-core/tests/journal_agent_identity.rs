mod journal_support;
mod scratch;

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use agentdust_core::identity::{IdentityEvidence, KernelIdentity, ProcessIdentity};
use agentdust_core::journal::{
    AgentIdentity, ExeBase, FieldError, MAX_AGENT_IDENTITY_LEN, MAX_EXE_BASE_LEN, Record, SessionTagKey,
    decode,
};
use journal_support::{frame, journal, named};
use scratch::TempDir;
use serde_json::json;

const KEY: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
const BOOT: &str = "11111111-2222-3333-4444-555555555555";

fn claude() -> AgentIdentity {
    AgentIdentity::new(
        4242,
        1_800_000_000_000_000,
        501,
        Some(ExeBase::try_from("claude").unwrap()),
    )
    .unwrap()
}

fn process(pid: i32, path: impl AsRef<OsStr>) -> ProcessIdentity {
    ProcessIdentity {
        kernel: KernelIdentity {
            boot_session_uuid: BOOT.to_owned(),
            pid,
            start_time_us: 1_800_000_000_000_007,
            uid: 501,
        },
        evidence: IdentityEvidence {
            exe_path: PathBuf::from(path.as_ref()),
        },
    }
}

#[test]
fn an_identity_needs_a_positive_pid() {
    assert!(AgentIdentity::new(1, 0, 0, None).is_ok());
    assert!(AgentIdentity::new(i32::MAX, u64::MAX, u32::MAX, None).is_ok());
    for pid in [0, -1, i32::MIN] {
        assert_eq!(
            AgentIdentity::new(pid, 1, 1, None).unwrap_err(),
            FieldError::NotPositive,
            "{pid}"
        );
    }
}

#[test]
fn an_identity_serialises_four_keys_in_a_fixed_order() {
    assert_eq!(
        serde_json::to_string(&claude()).unwrap(),
        r#"{"pid":4242,"start_time_us":1800000000000000,"uid":501,"exe_base":"claude"}"#
    );
}

#[test]
fn an_identity_without_an_executable_name_omits_the_key() {
    let bare = AgentIdentity::new(7, 8, 9, None).unwrap();
    assert_eq!(
        serde_json::to_string(&bare).unwrap(),
        r#"{"pid":7,"start_time_us":8,"uid":9}"#
    );
}

#[test]
fn an_identity_round_trips() {
    for identity in [claude(), AgentIdentity::new(7, 8, 9, None).unwrap()] {
        let text = serde_json::to_string(&identity).unwrap();
        assert_eq!(serde_json::from_str::<AgentIdentity>(&text).unwrap(), identity);
    }
}

#[test]
fn the_accessors_return_what_the_constructor_took() {
    let identity = claude();
    assert_eq!(identity.pid(), 4242);
    assert_eq!(identity.start_time_us(), 1_800_000_000_000_000);
    assert_eq!(identity.uid(), 501);
    assert_eq!(identity.exe_base().map(ExeBase::as_str), Some("claude"));
}

#[test]
fn a_pid_outside_one_to_i32_max_does_not_deserialise() {
    for bad in [
        json!(0),
        json!(-1),
        json!(2_147_483_648u64),
        json!(1.5),
        json!("1"),
        json!(null),
    ] {
        let value = json!({"pid": bad, "start_time_us": 1, "uid": 1});
        assert!(serde_json::from_value::<AgentIdentity>(value).is_err(), "{bad}");
    }
}

#[test]
fn the_start_time_and_the_user_are_unsigned_integers_of_their_own_width() {
    let widest = json!({"pid": 1, "start_time_us": u64::MAX, "uid": u32::MAX});
    assert!(serde_json::from_value::<AgentIdentity>(widest).is_ok());
    for (field, bad) in [
        ("start_time_us", json!(-1)),
        ("start_time_us", json!(1.5)),
        ("start_time_us", json!("1")),
        ("uid", json!(-1)),
        ("uid", json!(u64::from(u32::MAX) + 1)),
        ("uid", json!("501")),
    ] {
        let mut value = json!({"pid": 1, "start_time_us": 1, "uid": 1});
        value[field] = bad.clone();
        assert!(
            serde_json::from_value::<AgentIdentity>(value).is_err(),
            "{field} {bad}"
        );
    }
}

#[test]
fn an_identity_with_a_missing_key_does_not_deserialise() {
    for missing in ["pid", "start_time_us", "uid"] {
        let mut value = json!({"pid": 1, "start_time_us": 1, "uid": 1});
        value.as_object_mut().unwrap().remove(missing);
        assert!(
            serde_json::from_value::<AgentIdentity>(value).is_err(),
            "{missing}"
        );
    }
}

#[test]
fn a_key_the_build_does_not_know_is_ignored() {
    let value = json!({"pid": 1, "start_time_us": 2, "uid": 3, "future": [1, 2]});
    assert_eq!(
        serde_json::from_value::<AgentIdentity>(value).unwrap(),
        AgentIdentity::new(1, 2, 3, None).unwrap()
    );
}

#[test]
fn an_identity_with_an_invalid_executable_name_does_not_deserialise() {
    for bad in [
        json!("a\u{1b}[0m"),
        json!("n".repeat(65)),
        json!(7),
        json!(["node"]),
    ] {
        let value = json!({"pid": 1, "start_time_us": 2, "uid": 3, "exe_base": bad});
        assert!(serde_json::from_value::<AgentIdentity>(value).is_err(), "{bad}");
    }
}

#[test]
fn a_null_executable_name_reads_as_absent() {
    let value = json!({"pid": 1, "start_time_us": 2, "uid": 3, "exe_base": null});
    assert_eq!(
        serde_json::from_value::<AgentIdentity>(value).unwrap(),
        AgentIdentity::new(1, 2, 3, None).unwrap()
    );
}

#[test]
fn the_widest_identity_stays_under_the_documented_bound() {
    let widest = AgentIdentity::new(
        i32::MAX,
        u64::MAX,
        u32::MAX,
        Some(ExeBase::try_from("x".repeat(MAX_EXE_BASE_LEN)).unwrap()),
    )
    .unwrap();
    let encoded = serde_json::to_string(&widest).unwrap();
    assert!(encoded.len() <= MAX_AGENT_IDENTITY_LEN, "{}", encoded.len());
    assert_eq!(MAX_AGENT_IDENTITY_LEN, 160);
}

#[test]
fn an_identity_is_taken_from_a_process_by_its_executable_name() {
    let identity = AgentIdentity::from_process(&process(
        4242,
        "/Applications/Claude Code/claude.app/MacOS/claude",
    ))
    .unwrap();
    assert_eq!(identity.pid(), 4242);
    assert_eq!(identity.start_time_us(), 1_800_000_000_000_007);
    assert_eq!(identity.uid(), 501);
    assert_eq!(identity.exe_base().map(ExeBase::as_str), Some("claude"));
}

#[test]
fn the_path_of_the_process_is_not_part_of_the_identity() {
    let secret = "/Users/user-sentinel/.local/bin/claude";
    let identity = AgentIdentity::from_process(&process(4242, secret)).unwrap();
    let text = serde_json::to_string(&identity).unwrap();
    assert!(!text.contains("user-sentinel"));
    assert!(!text.contains('/'));
}

#[test]
fn an_executable_name_that_cannot_be_stored_leaves_the_identity_without_one() {
    let long = format!("/bin/{}", "n".repeat(MAX_EXE_BASE_LEN + 1));
    let control = "/bin/no\u{1b}[31mde";
    for path in [long.as_str(), control, "/"] {
        let identity = AgentIdentity::from_process(&process(9, path)).unwrap();
        assert_eq!(identity.exe_base(), None, "{path:?}");
        assert_eq!(identity.pid(), 9);
    }
    let not_unicode = process(9, OsStr::from_bytes(b"/bin/\xff\xfe"));
    assert_eq!(
        AgentIdentity::from_process(&not_unicode).unwrap().exe_base(),
        None
    );
}

#[test]
fn a_process_without_a_positive_pid_gives_no_identity() {
    for pid in [0, -1] {
        assert_eq!(
            AgentIdentity::from_process(&process(pid, "/bin/claude")).unwrap_err(),
            FieldError::NotPositive
        );
    }
}

#[test]
fn the_kernel_identity_is_rebuilt_with_the_boot_of_the_record() {
    let kernel = claude().kernel(BOOT);
    assert_eq!(
        kernel,
        KernelIdentity {
            boot_session_uuid: BOOT.to_owned(),
            pid: 4242,
            start_time_us: 1_800_000_000_000_000,
            uid: 501,
        }
    );
    assert_ne!(claude().kernel("another-boot"), kernel);
}

#[test]
fn a_tag_key_is_the_type_the_digest_produces() {
    let digest = agentdust_core::digest::keyed_digest(b"k", agentdust_core::digest::Domain::Session, b"tag");
    assert!(SessionTagKey::try_from(digest).is_ok());
}

fn with_both() -> Record {
    Record {
        agent_identity: Some(claude()),
        session_tag_key: Some(SessionTagKey::try_from(KEY).unwrap()),
        ..named("s1")
    }
}

#[test]
fn a_frame_with_both_fields_decodes_to_the_same_record() {
    let report = decode(&frame(&with_both())[..]).unwrap();
    assert_eq!(report.records, vec![with_both()]);
    assert_eq!(report.skipped_lines(), 0);
}

#[test]
fn a_frame_with_an_out_of_range_identity_is_counted_as_malformed() {
    let mut value = serde_json::to_value(with_both()).unwrap();
    value["agent_identity"]["pid"] = json!(0);
    let mut bytes = vec![0x1e];
    bytes.extend(serde_json::to_vec(&value).unwrap());
    bytes.push(b'\n');
    let report = decode(&bytes[..]).unwrap();
    assert!(report.records.is_empty());
    assert_eq!(report.malformed_lines, 1);
}

#[test]
fn a_record_with_both_fields_survives_an_append_and_a_read() {
    let dir = TempDir::absent("identity-roundtrip");
    journal(&dir).append(&with_both()).unwrap();
    let report = journal(&dir).read().unwrap();
    assert_eq!(report.records, vec![with_both()]);
}

#[test]
fn the_widest_record_fits_in_a_frame_with_room_to_spare() {
    let widest = Record {
        agent_identity: Some(
            AgentIdentity::new(
                i32::MAX,
                u64::MAX,
                u32::MAX,
                Some(ExeBase::try_from("x".repeat(MAX_EXE_BASE_LEN)).unwrap()),
            )
            .unwrap(),
        ),
        ..with_both()
    };
    assert!(frame(&widest).len() < 1024);
}
