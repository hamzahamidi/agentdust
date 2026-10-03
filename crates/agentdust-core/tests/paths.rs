use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::Path;

use agentdust_core::paths::resolve_data_dir;

fn home() -> Option<&'static OsStr> {
    Some(OsStr::new("/Users/a"))
}

#[test]
fn an_absolute_override_is_used_as_is() {
    let dir = resolve_data_dir(Some(OsStr::new("/tmp/agentdust")), home()).unwrap();
    assert_eq!(dir, Path::new("/tmp/agentdust"));
}

#[test]
fn an_empty_override_counts_as_unset() {
    let dir = resolve_data_dir(Some(OsStr::new("")), home()).unwrap();
    assert_eq!(dir, Path::new("/Users/a/Library/Application Support/agentdust"));
}

#[test]
fn a_relative_override_is_rejected() {
    let err = resolve_data_dir(Some(OsStr::new("rel")), home()).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
}

#[test]
fn a_missing_empty_or_relative_home_is_rejected() {
    for home in [None, Some(OsStr::new("")), Some(OsStr::new("Users/a"))] {
        assert!(resolve_data_dir(None, home).is_err(), "{home:?}");
    }
}
