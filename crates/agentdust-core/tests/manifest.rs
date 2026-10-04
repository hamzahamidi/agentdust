mod scratch;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::Path;

use agentdust_core::manifest::{
    Entry, MANIFEST_FILE, MANIFEST_VERSION, Manifest, ManifestError, Origin, Resource, delete, load, store,
};
use agentdust_core::safe_open::SafeOpenError;
use scratch::{private_dir, scratch_dir};
use serde_json::{Value, json};

fn hook_entry(event: &str, origin: Origin) -> Entry {
    Entry {
        resource: Resource::Hook,
        target: "/home/u/.claude/settings.json".into(),
        key: event.into(),
        origin,
        command: "/opt/homebrew/bin/agentdust hook claude".into(),
        args: Vec::new(),
        hash: "ab".repeat(32),
        created_hooks_key: true,
        created_event_key: false,
    }
}

fn mcp_entry() -> Entry {
    Entry {
        resource: Resource::McpServer,
        target: "/home/u/.claude".into(),
        key: "agentdust".into(),
        origin: Origin::Created,
        command: "/opt/homebrew/bin/agentdust".into(),
        args: vec!["mcp".into()],
        hash: "cd".repeat(32),
        created_hooks_key: false,
        created_event_key: false,
    }
}

fn sample() -> Manifest {
    Manifest {
        version: MANIFEST_VERSION,
        entries: vec![
            hook_entry("SessionStart", Origin::Created),
            hook_entry("PreToolUse", Origin::PreExisting),
            mcp_entry(),
        ],
        agent_executables: BTreeMap::from([("claude".to_owned(), "/Users/u/.local/share/claude/2.1".to_owned())]),
    }
}

fn write_raw(dir: &Path, text: &str) {
    let path = dir.join(MANIFEST_FILE);
    fs::write(&path, text).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn mode_of(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

#[test]
fn the_version_and_file_name_are_fixed() {
    assert_eq!(MANIFEST_VERSION, 1);
    assert_eq!(MANIFEST_FILE, "manifest.json");
    assert_eq!(Manifest::new().version, 1);
    assert!(Manifest::new().entries.is_empty());
}

#[test]
fn a_missing_manifest_or_data_directory_loads_as_none() {
    let dir = private_dir("mf-missing");
    assert!(load(&dir).unwrap().is_none());
    let absent = scratch_dir("mf-missing-dir");
    assert!(load(&absent).unwrap().is_none());
    assert!(!absent.exists());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_stored_manifest_loads_back_unchanged() {
    let dir = private_dir("mf-round-trip");
    store(&dir, &sample()).unwrap();
    assert_eq!(load(&dir).unwrap(), Some(sample()));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn store_creates_the_data_directory_0700_and_the_file_0600() {
    let dir = scratch_dir("mf-create");
    store(&dir, &sample()).unwrap();
    assert_eq!(mode_of(&dir), 0o700);
    assert_eq!(mode_of(&dir.join(MANIFEST_FILE)), 0o600);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn store_replaces_the_manifest_by_rename_and_leaves_no_temporary_file() {
    let dir = private_dir("mf-replace");
    store(&dir, &Manifest::new()).unwrap();
    let before = fs::metadata(dir.join(MANIFEST_FILE)).unwrap().ino();
    store(&dir, &sample()).unwrap();
    assert_ne!(fs::metadata(dir.join(MANIFEST_FILE)).unwrap().ino(), before);
    let names: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, [MANIFEST_FILE]);
    assert_eq!(load(&dir).unwrap(), Some(sample()));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_file_holds_only_the_documented_keys() {
    let dir = private_dir("mf-shape");
    store(&dir, &sample()).unwrap();
    let value: Value = serde_json::from_slice(&fs::read(dir.join(MANIFEST_FILE)).unwrap()).unwrap();
    assert_eq!(
        value,
        json!({
            "version": 1,
            "entries": [
                {
                    "resource": "hook",
                    "target": "/home/u/.claude/settings.json",
                    "key": "SessionStart",
                    "origin": "created",
                    "command": "/opt/homebrew/bin/agentdust hook claude",
                    "hash": "ab".repeat(32),
                    "created_hooks_key": true,
                    "created_event_key": false
                },
                {
                    "resource": "hook",
                    "target": "/home/u/.claude/settings.json",
                    "key": "PreToolUse",
                    "origin": "pre_existing",
                    "command": "/opt/homebrew/bin/agentdust hook claude",
                    "hash": "ab".repeat(32),
                    "created_hooks_key": true,
                    "created_event_key": false
                },
                {
                    "resource": "mcp_server",
                    "target": "/home/u/.claude",
                    "key": "agentdust",
                    "origin": "created",
                    "command": "/opt/homebrew/bin/agentdust",
                    "args": ["mcp"],
                    "hash": "cd".repeat(32),
                    "created_hooks_key": false,
                    "created_event_key": false
                }
            ],
            "agent_executables": { "claude": "/Users/u/.local/share/claude/2.1" }
        })
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_manifest_from_a_future_version_fails_closed() {
    let dir = private_dir("mf-future");
    write_raw(&dir, r#"{"version":2,"entries":[],"something_new":true}"#);
    match load(&dir) {
        Err(ManifestError::UnknownVersion { found }) => assert_eq!(found, 2),
        other => panic!("{other:?}"),
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_manifest_from_version_zero_also_fails_closed() {
    let dir = private_dir("mf-zero");
    write_raw(&dir, r#"{"version":0,"entries":[]}"#);
    assert!(matches!(
        load(&dir),
        Err(ManifestError::UnknownVersion { found: 0 })
    ));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_missing_or_malformed_version_is_corrupt() {
    let dir = private_dir("mf-no-version");
    for text in [
        r#"{"entries":[]}"#,
        r#"{"version":"1","entries":[]}"#,
        r#"{"version":1.5,"entries":[]}"#,
        r#"{"version":-1,"entries":[]}"#,
        "[]",
        "null",
    ] {
        write_raw(&dir, text);
        assert!(matches!(load(&dir), Err(ManifestError::Corrupt(_))), "{text}");
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn garbage_and_unknown_fields_are_corrupt() {
    let dir = private_dir("mf-garbage");
    for text in [
        "",
        "not json",
        "{",
        r#"{"version":1,"entries":[],"extra":1}"#,
        r#"{"version":1,"entries":[{"resource":"hook"}]}"#,
        r#"{"version":1,"entries":[{"resource":"cursor_rules","target":"t","key":"k","origin":"created","command":"c","hash":"h"}]}"#,
        r#"{"version":1,"entries":[{"resource":"hook","target":"t","key":"k","origin":"adopted","command":"c","hash":"h"}]}"#,
    ] {
        write_raw(&dir, text);
        assert!(matches!(load(&dir), Err(ManifestError::Corrupt(_))), "{text:?}");
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_symlinked_manifest_is_refused_for_load_and_store() {
    let dir = private_dir("mf-symlink");
    let target = dir.join("elsewhere.json");
    fs::write(&target, "keep").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&target, dir.join(MANIFEST_FILE)).unwrap();
    assert!(matches!(
        load(&dir),
        Err(ManifestError::Refused(SafeOpenError::Symlink))
    ));
    assert!(matches!(
        store(&dir, &sample()),
        Err(ManifestError::Refused(SafeOpenError::Symlink))
    ));
    assert_eq!(fs::read(&target).unwrap(), b"keep");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_hard_linked_or_loose_manifest_is_refused() {
    let dir = private_dir("mf-hardlink");
    write_raw(&dir, r#"{"version":1,"entries":[]}"#);
    fs::hard_link(dir.join(MANIFEST_FILE), dir.join("second")).unwrap();
    assert!(matches!(
        load(&dir),
        Err(ManifestError::Refused(SafeOpenError::HardLinked { .. }))
    ));
    assert!(matches!(
        store(&dir, &sample()),
        Err(ManifestError::Refused(SafeOpenError::HardLinked { .. }))
    ));
    fs::remove_file(dir.join("second")).unwrap();
    fs::set_permissions(dir.join(MANIFEST_FILE), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        load(&dir),
        Err(ManifestError::Refused(SafeOpenError::LooseMode { .. }))
    ));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_data_directory_that_is_not_private_is_refused_for_store() {
    let dir = private_dir("mf-loose-dir");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        store(&dir, &sample()),
        Err(ManifestError::Refused(SafeOpenError::LooseMode { .. }))
    ));
    assert!(!dir.join(MANIFEST_FILE).exists());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn delete_removes_the_manifest_and_tolerates_a_missing_one() {
    let dir = private_dir("mf-delete");
    store(&dir, &sample()).unwrap();
    delete(&dir).unwrap();
    assert!(!dir.join(MANIFEST_FILE).exists());
    delete(&dir).unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn delete_refuses_a_symlink_and_leaves_its_target() {
    let dir = private_dir("mf-delete-symlink");
    let target = dir.join("elsewhere.json");
    fs::write(&target, "keep").unwrap();
    symlink(&target, dir.join(MANIFEST_FILE)).unwrap();
    assert!(delete(&dir).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"keep");
    fs::remove_dir_all(&dir).unwrap();
}
