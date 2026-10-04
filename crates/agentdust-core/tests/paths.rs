use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::Path;

use agentdust_core::paths::{CLAUDE_CONFIG_DIR_ENV, resolve_claude_config_dir, resolve_data_dir};

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

#[test]
fn the_claude_config_variable_is_named_as_claude_code_names_it() {
    assert_eq!(CLAUDE_CONFIG_DIR_ENV, "CLAUDE_CONFIG_DIR");
}

#[test]
fn the_claude_config_dir_defaults_to_dot_claude_under_home() {
    let dir = resolve_claude_config_dir(None, home()).unwrap();
    assert_eq!(dir, Path::new("/Users/a/.claude"));
}

#[test]
fn an_absolute_claude_config_override_wins_over_home() {
    let dir = resolve_claude_config_dir(Some(OsStr::new("/work/claude")), home()).unwrap();
    assert_eq!(dir, Path::new("/work/claude"));
    let dir = resolve_claude_config_dir(Some(OsStr::new("/work/claude")), None).unwrap();
    assert_eq!(dir, Path::new("/work/claude"));
}

#[test]
fn an_empty_claude_config_override_counts_as_unset() {
    let dir = resolve_claude_config_dir(Some(OsStr::new("")), home()).unwrap();
    assert_eq!(dir, Path::new("/Users/a/.claude"));
}

#[test]
fn a_relative_claude_config_override_is_rejected() {
    let err = resolve_claude_config_dir(Some(OsStr::new("rel/claude")), home()).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
}

#[test]
fn the_claude_config_dir_needs_an_override_or_an_absolute_home() {
    for home in [None, Some(OsStr::new("")), Some(OsStr::new("Users/a"))] {
        assert!(resolve_claude_config_dir(None, home).is_err(), "{home:?}");
    }
}
