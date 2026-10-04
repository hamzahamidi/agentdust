use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use agentdust_core::identity::{IdentityEvidence, ProcessIdentity};

mod common;
use common::identity;

#[test]
fn an_identity_round_trips_through_json() {
    let original = identity(4242);
    let json = serde_json::to_string(&original).unwrap();
    let parsed: ProcessIdentity = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, original);
}

#[test]
fn json_names_every_kernel_field_and_the_path() {
    let value = serde_json::to_value(identity(4242)).unwrap();
    assert_eq!(value["kernel"]["pid"], 4242);
    assert_eq!(value["kernel"]["uid"], 501);
    assert_eq!(value["kernel"]["start_time_us"], 1_800_000_000_000_000u64);
    assert_eq!(value["kernel"]["boot_session_uuid"], common::BOOT);
    assert_eq!(value["evidence"]["exe_path"], "/usr/local/bin/node");
}

#[test]
fn a_path_that_is_not_utf8_fails_to_serialize_instead_of_panicking() {
    let evidence = IdentityEvidence {
        exe_path: PathBuf::from(OsString::from_vec(vec![b'/', 0xff, 0xfe])),
    };
    assert!(serde_json::to_string(&evidence).is_err());
}

#[test]
fn paths_that_differ_only_in_separators_are_different_evidence() {
    for other in [
        "/usr/local/bin/node/",
        "/usr//local/bin/node",
        "/usr/./local/bin/node",
    ] {
        let different = common::changed(&identity(7), |id| id.evidence.exe_path = PathBuf::from(other));
        assert_ne!(different, identity(7), "{other}");
    }
}

#[test]
fn identical_paths_are_equal_evidence() {
    assert_eq!(identity(7), identity(7));
    assert_ne!(identity(7), identity(8));
}
