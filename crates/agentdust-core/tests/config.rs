mod scratch;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

use agentdust_core::config::{ApplySwitch, CONFIG_FILE, Disabled, apply_switch};
use scratch::{TempDir, make_fifo, returns_promptly};

fn write(dir: &Path, text: &[u8], mode: u32) {
    let path = dir.join(CONFIG_FILE);
    fs::write(&path, text).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
}

fn switch_for(text: &str) -> ApplySwitch {
    let dir = TempDir::private("config");
    write(&dir, text.as_bytes(), 0o600);
    apply_switch(&dir)
}

fn code_of(switch: &ApplySwitch) -> &'static str {
    match switch {
        ApplySwitch::Enabled => "enabled",
        ApplySwitch::Disabled(reason) => reason.code(),
    }
}

#[test]
fn the_file_name_is_config_toml() {
    assert_eq!(CONFIG_FILE, "config.toml");
}

#[test]
fn a_missing_file_leaves_apply_enabled() {
    let dir = TempDir::private("config-missing");
    assert_eq!(apply_switch(&dir), ApplySwitch::Enabled);
}

#[test]
fn a_missing_data_directory_leaves_apply_enabled() {
    let dir = TempDir::absent("config-no-dir");
    assert_eq!(apply_switch(&dir), ApplySwitch::Enabled);
}

#[test]
fn an_empty_file_and_a_file_without_the_setting_leave_apply_enabled() {
    assert_eq!(switch_for(""), ApplySwitch::Enabled);
    assert_eq!(switch_for("# nothing set\n"), ApplySwitch::Enabled);
    assert_eq!(
        switch_for("theme = \"dark\"\nretention_days = 14\n"),
        ApplySwitch::Enabled
    );
}

#[test]
fn apply_true_leaves_apply_enabled() {
    assert_eq!(switch_for("apply = true\n"), ApplySwitch::Enabled);
}

#[test]
fn apply_false_disables_apply_in_every_spelling_toml_allows() {
    for text in [
        "apply = false\n",
        "apply=false\n",
        "  apply   =   false   # off while a release is withdrawn\n",
        "\"apply\" = false\n",
        "other = 1\napply = false\n",
        "apply = false",
    ] {
        assert_eq!(
            switch_for(text),
            ApplySwitch::Disabled(Disabled::SwitchedOff),
            "{text:?}"
        );
    }
}

#[test]
fn a_setting_in_another_table_is_not_the_apply_setting() {
    assert_eq!(switch_for("[ui]\napply = false\n"), ApplySwitch::Enabled);
}

#[test]
fn a_file_that_does_not_parse_disables_apply() {
    for text in [
        "apply = \n",
        "this is not toml {{\n",
        "apply = false\napply = true\n",
        "[unclosed\napply = true\n",
        "apply = tru\n",
    ] {
        assert_eq!(
            switch_for(text),
            ApplySwitch::Disabled(Disabled::Invalid),
            "{text:?}"
        );
    }
}

#[test]
fn an_apply_setting_that_is_not_a_boolean_disables_apply() {
    for text in [
        "apply = \"false\"\n",
        "apply = 0\n",
        "apply = 1\n",
        "apply = []\n",
        "apply = {}\n",
    ] {
        assert_eq!(
            switch_for(text),
            ApplySwitch::Disabled(Disabled::Invalid),
            "{text:?}"
        );
    }
}

#[test]
fn a_file_that_is_not_utf8_disables_apply() {
    let dir = TempDir::private("config-bytes");
    write(&dir, b"apply = true\n\xff\xfe", 0o600);
    assert_eq!(apply_switch(&dir), ApplySwitch::Disabled(Disabled::Invalid));
}

#[test]
fn a_file_over_64_kib_disables_apply() {
    let dir = TempDir::private("config-big");
    let mut text = String::from("apply = true\n");
    text.push_str(&"# padding\n".repeat(7000));
    write(&dir, text.as_bytes(), 0o600);
    assert!(text.len() > 64 * 1024);
    assert_eq!(apply_switch(&dir), ApplySwitch::Disabled(Disabled::Invalid));
}

#[test]
fn a_file_just_under_the_limit_is_read() {
    let dir = TempDir::private("config-near");
    let mut text = String::from("apply = true\n");
    while text.len() + 10 <= 64 * 1024 {
        text.push_str("# padding\n");
    }
    write(&dir, text.as_bytes(), 0o600);
    assert_eq!(apply_switch(&dir), ApplySwitch::Enabled);
}

#[test]
fn a_file_that_cannot_be_read_disables_apply() {
    let dir = TempDir::private("config-denied");
    write(&dir, b"apply = true\n", 0o000);
    assert_eq!(apply_switch(&dir), ApplySwitch::Disabled(Disabled::Unreadable));
}

#[test]
fn a_data_directory_that_is_a_file_disables_apply() {
    let dir = TempDir::private("config-notdir");
    let not_a_dir = dir.join("file");
    fs::write(&not_a_dir, b"x").unwrap();
    assert_eq!(
        apply_switch(&not_a_dir),
        ApplySwitch::Disabled(Disabled::Unreadable)
    );
}

#[test]
fn a_symbolic_link_is_not_followed_and_disables_apply() {
    let dir = TempDir::private("config-link");
    let real = dir.join("real.toml");
    fs::write(&real, b"apply = true\n").unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&real, dir.join(CONFIG_FILE)).unwrap();
    assert!(matches!(
        apply_switch(&dir),
        ApplySwitch::Disabled(Disabled::Unsafe(_))
    ));
}

#[test]
fn a_hard_linked_file_disables_apply() {
    let dir = TempDir::private("config-hard");
    write(&dir, b"apply = true\n", 0o600);
    fs::hard_link(dir.join(CONFIG_FILE), dir.join("other")).unwrap();
    assert!(matches!(
        apply_switch(&dir),
        ApplySwitch::Disabled(Disabled::Unsafe(_))
    ));
}

#[test]
fn a_directory_in_place_of_the_file_disables_apply() {
    let dir = TempDir::private("config-dir");
    fs::create_dir(dir.join(CONFIG_FILE)).unwrap();
    assert!(matches!(
        apply_switch(&dir),
        ApplySwitch::Disabled(Disabled::Unsafe(_))
    ));
}

#[test]
fn a_fifo_in_place_of_the_file_disables_apply_without_blocking() {
    let dir = TempDir::private("config-fifo");
    make_fifo(&dir.join(CONFIG_FILE));
    let path = dir.path().to_path_buf();
    let switch = returns_promptly(move || apply_switch(&path));
    assert!(matches!(switch, ApplySwitch::Disabled(Disabled::Unsafe(_))));
}

#[test]
fn a_file_readable_by_others_disables_apply_and_the_reason_names_the_modes() {
    let dir = TempDir::private("config-loose");
    write(&dir, b"apply = true\n", 0o644);
    let ApplySwitch::Disabled(Disabled::Unsafe(detail)) = apply_switch(&dir) else {
        panic!("a loose mode must disable apply");
    };
    assert!(detail.contains("644"), "{detail}");
    assert!(detail.contains("600"), "{detail}");
}

#[test]
fn a_read_only_private_file_is_accepted() {
    let dir = TempDir::private("config-ro");
    write(&dir, b"apply = false\n", 0o400);
    assert_eq!(apply_switch(&dir), ApplySwitch::Disabled(Disabled::SwitchedOff));
}

#[test]
fn every_refusal_has_a_stable_code_and_a_description() {
    let reasons = [
        (Disabled::SwitchedOff, "switched_off"),
        (
            Disabled::Unsafe("mode 644 is looser than 600".to_owned()),
            "unsafe",
        ),
        (Disabled::Unreadable, "unreadable"),
        (Disabled::Invalid, "invalid"),
    ];
    for (reason, code) in reasons {
        assert_eq!(reason.code(), code);
        assert!(!reason.describe().is_empty());
        assert!(!reason.describe().contains('\u{2014}'));
    }
    assert!(Disabled::SwitchedOff.describe().contains("apply = false"));
    assert!(Disabled::Invalid.describe().contains("config.toml"));
}

#[test]
fn the_setting_is_read_each_time() {
    let dir = TempDir::private("config-again");
    assert_eq!(apply_switch(&dir), ApplySwitch::Enabled);
    write(&dir, b"apply = false\n", 0o600);
    assert_eq!(code_of(&apply_switch(&dir)), "switched_off");
    write(&dir, b"apply = true\n", 0o600);
    assert_eq!(code_of(&apply_switch(&dir)), "enabled");
    write(&dir, b"apply = what\n", 0o600);
    assert_eq!(code_of(&apply_switch(&dir)), "invalid");
}
