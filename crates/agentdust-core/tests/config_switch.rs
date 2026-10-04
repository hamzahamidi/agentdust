mod scratch;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

use agentdust_core::config::{ApplySwitch, CONFIG_FILE, apply_switch, parse_apply};
use scratch::{private_dir, scratch_dir};

fn write_config(dir: &Path, text: &[u8], mode: u32) {
    let path = dir.join(CONFIG_FILE);
    fs::write(&path, text).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
}

fn is_root() -> bool {
    // SAFETY: geteuid takes no arguments and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

fn unreadable(switch: &ApplySwitch) -> bool {
    matches!(switch, ApplySwitch::Unreadable(_))
}

#[test]
fn the_file_name_is_config_toml() {
    assert_eq!(CONFIG_FILE, "config.toml");
}

#[test]
fn a_missing_file_or_directory_leaves_apply_enabled() {
    let dir = private_dir("cfg-missing");
    assert_eq!(apply_switch(&dir), ApplySwitch::Enabled);
    let absent = scratch_dir("cfg-missing-dir");
    assert_eq!(apply_switch(&absent), ApplySwitch::Enabled);
    assert!(!absent.exists());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_empty_file_or_one_without_the_key_leaves_apply_enabled() {
    let dir = private_dir("cfg-empty");
    for text in [
        "",
        "\n\n",
        "# only a comment\n",
        "retention_days = 14\n",
        "name = \"x\"\n",
    ] {
        write_config(&dir, text.as_bytes(), 0o600);
        assert_eq!(apply_switch(&dir), ApplySwitch::Enabled, "{text:?}");
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn apply_false_disables_apply_and_apply_true_enables_it() {
    let dir = private_dir("cfg-values");
    write_config(&dir, b"apply = false\n", 0o600);
    assert_eq!(apply_switch(&dir), ApplySwitch::Disabled);
    write_config(&dir, b"apply = true\n", 0o600);
    assert_eq!(apply_switch(&dir), ApplySwitch::Enabled);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn only_an_enabled_switch_allows_apply() {
    assert!(ApplySwitch::Enabled.allows_apply());
    assert!(!ApplySwitch::Disabled.allows_apply());
    assert!(!ApplySwitch::Unreadable("x".into()).allows_apply());
}

#[test]
fn comments_blank_lines_spacing_and_other_keys_do_not_change_the_answer() {
    assert_eq!(parse_apply("apply=false"), Ok(Some(false)));
    assert_eq!(parse_apply("  apply   =   false   # because\n"), Ok(Some(false)));
    assert_eq!(
        parse_apply("# top\n\nname = \"a b\"\nretention = 14\napply = false\nother = 'x'\n"),
        Ok(Some(false))
    );
    assert_eq!(parse_apply("apply = true\r\nname = \"x\"\r\n"), Ok(Some(true)));
    assert_eq!(parse_apply("name = \"x\"\n"), Ok(None));
}

#[test]
fn a_key_inside_a_table_is_not_the_top_level_switch() {
    assert_eq!(parse_apply("[other]\napply = false\n"), Ok(None));
    assert_eq!(
        parse_apply("apply = true\n[other]\napply = false\n"),
        Ok(Some(true))
    );
    assert_eq!(parse_apply("[[rules]]\napply = false\n"), Ok(None));
}

#[test]
fn syntax_the_reader_does_not_understand_is_unreadable_never_enabled() {
    for text in [
        "\"apply\" = false\n",
        "'apply' = false\n",
        "apply.mode = false\n",
        "apply = \"false\"\n",
        "apply = 0\n",
        "apply = False\n",
        "apply false\n",
        "apply = false extra\n",
        "apply = false\napply = true\n",
        "apply = [false]\n",
        "apply = { x = 1 }\n",
        "a = \"\"\"\napply = true\n\"\"\"\n",
        "= false\n",
        "[unclosed\n",
        "apply = false\u{0}\n",
    ] {
        assert!(parse_apply(text).is_err(), "{text:?}");
    }
}

#[test]
fn an_unparseable_file_disables_apply() {
    let dir = private_dir("cfg-garbage");
    write_config(&dir, b"apply false\n", 0o600);
    let switch = apply_switch(&dir);
    assert!(unreadable(&switch), "{switch:?}");
    assert!(!switch.allows_apply());
    write_config(&dir, b"apply = false\n\xff\xfe", 0o600);
    assert!(unreadable(&apply_switch(&dir)));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_symlinked_hard_linked_or_loose_file_is_unreadable() {
    let dir = private_dir("cfg-unsafe");
    let target = dir.join("real.toml");
    fs::write(&target, "apply = true\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&target, dir.join(CONFIG_FILE)).unwrap();
    assert!(unreadable(&apply_switch(&dir)));
    fs::remove_file(dir.join(CONFIG_FILE)).unwrap();

    write_config(&dir, b"apply = true\n", 0o600);
    fs::hard_link(dir.join(CONFIG_FILE), dir.join("second")).unwrap();
    assert!(unreadable(&apply_switch(&dir)));
    fs::remove_file(dir.join("second")).unwrap();

    write_config(&dir, b"apply = true\n", 0o644);
    assert!(unreadable(&apply_switch(&dir)));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_file_that_cannot_be_read_is_unreadable() {
    if is_root() {
        return;
    }
    let dir = private_dir("cfg-denied");
    write_config(&dir, b"apply = true\n", 0o000);
    assert!(unreadable(&apply_switch(&dir)));
    fs::set_permissions(dir.join(CONFIG_FILE), fs::Permissions::from_mode(0o600)).unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_config_path_that_is_a_directory_is_unreadable() {
    let dir = private_dir("cfg-is-dir");
    fs::create_dir(dir.join(CONFIG_FILE)).unwrap();
    assert!(unreadable(&apply_switch(&dir)));
    fs::remove_dir_all(&dir).unwrap();
}
