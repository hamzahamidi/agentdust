mod support;

use std::cell::Cell;
use std::fs::{self, DirBuilder};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt, symlink};
use std::path::PathBuf;

use agentdust_agents::claude_setup::{
    InstallOutcome, McpState, Overall, RemoveOutcome, SetupEnv, SetupError, check, install, remove,
};
use agentdust_agents::hook_config::{HOOK_SPECS, HookState};
use agentdust_agents::native_cli::{CliRunner, mcp_hash};
use agentdust_core::manifest::{self, Entry, MANIFEST_FILE, Manifest, ManifestError, Origin, Resource};
use serde_json::{Value, json};
use support::{FakeClaude, Mode, TempDir};

const EXE: &str = "/opt/homebrew/bin/agentdust";
const CMD: &str = "/opt/homebrew/bin/agentdust hook claude";

const SETTINGS: &str = "{\n  \"model\": \"opus\",\n  \"permissions\": {\n    \"allow\": [\"Bash(ls:*)\"]\n  },\n  \"hooks\": {\n    \"Stop\": [\n      {\n        \"hooks\": [{\"type\": \"command\", \"command\": \"afplay done.aiff\"}]\n      }\n    ]\n  }\n}\n";

struct World {
    dir: TempDir,
    config: PathBuf,
    data: PathBuf,
}

fn world(name: &str) -> World {
    let dir = TempDir::new(name);
    let config = dir.join("claude");
    let data = dir.join("data");
    DirBuilder::new().mode(0o755).create(&config).unwrap();
    DirBuilder::new().mode(0o700).create(&data).unwrap();
    World { dir, config, data }
}

impl World {
    fn env<'a>(&'a self, runner: Option<&'a dyn CliRunner>) -> SetupEnv<'a> {
        SetupEnv {
            config_dir: self.config.clone(),
            data_dir: self.data.clone(),
            exe: PathBuf::from(EXE),
            claude_cli: runner.map(|_| PathBuf::from("/usr/local/bin/claude")),
            runner,
        }
    }

    fn settings(&self) -> PathBuf {
        self.config.join("settings.json")
    }

    fn write_settings(&self, text: &str, mode: u32) {
        fs::write(self.settings(), text).unwrap();
        fs::set_permissions(self.settings(), fs::Permissions::from_mode(mode)).unwrap();
    }

    fn settings_json(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.settings()).unwrap()).unwrap()
    }

    fn manifest(&self) -> Option<Manifest> {
        manifest::load(&self.data).unwrap()
    }

    fn mode(&self) -> u32 {
        fs::metadata(self.settings()).unwrap().permissions().mode() & 0o7777
    }
}

fn is_root() -> bool {
    // SAFETY: geteuid takes no arguments and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

fn approve(_plan: &str) -> bool {
    true
}

fn applied(outcome: InstallOutcome) -> agentdust_agents::claude_setup::InstallReport {
    match outcome {
        InstallOutcome::Applied(report) => report,
        other => panic!("{other:?}"),
    }
}

fn installed(world: &World, fake: &FakeClaude) {
    let report = applied(install(&world.env(Some(fake)), &mut approve).unwrap());
    assert!(report.succeeded(), "{}", report.render());
}

fn hook_entries(manifest: &Manifest) -> Vec<&Entry> {
    manifest
        .entries
        .iter()
        .filter(|entry| entry.resource == Resource::Hook)
        .collect()
}

fn mcp_entry(manifest: &Manifest) -> Option<&Entry> {
    manifest
        .entries
        .iter()
        .find(|entry| entry.resource == Resource::McpServer)
}

#[test]
fn a_fresh_install_writes_hooks_registers_the_server_and_records_both() {
    let world = world("cs-fresh");
    let fake = FakeClaude::new(Mode::Honest);
    let report = applied(install(&world.env(Some(&fake)), &mut approve).unwrap());
    assert!(report.succeeded(), "{}", report.render());
    let settings = world.settings_json();
    for spec in HOOK_SPECS {
        let group = &settings["hooks"][spec.event][0];
        assert_eq!(group["hooks"][0]["command"], json!(CMD), "{}", spec.event);
        assert_eq!(group["hooks"][0]["timeout"], json!(10), "{}", spec.event);
        assert_eq!(
            group.get("matcher").and_then(Value::as_str),
            spec.matcher,
            "{}",
            spec.event
        );
    }
    assert_eq!(world.mode(), 0o600);
    assert_eq!(
        fake.state
            .borrow()
            .as_ref()
            .map(|server| (server.command.clone(), server.args.clone())),
        Some((EXE.to_owned(), vec!["mcp".to_owned()]))
    );
    assert_eq!(
        fake.calls(),
        [
            "mcp get agentdust".to_owned(),
            format!("mcp add --scope user agentdust -- {EXE} mcp"),
            "mcp get agentdust".to_owned()
        ]
    );
    let manifest = world.manifest().unwrap();
    assert_eq!(hook_entries(&manifest).len(), 4);
    assert!(
        manifest
            .entries
            .iter()
            .all(|entry| entry.origin == Origin::Created)
    );
    let server = mcp_entry(&manifest).unwrap();
    assert_eq!(
        (server.command.as_str(), server.args.as_slice()),
        (EXE, &["mcp".to_owned()][..])
    );
    assert_eq!(server.hash, mcp_hash(EXE, "mcp"));
    assert_eq!(
        manifest.agent_executables.get("claude").map(String::as_str),
        Some("/usr/local/bin/claude")
    );
}

#[test]
fn an_existing_settings_file_keeps_its_mode_its_keys_and_its_other_hooks() {
    let world = world("cs-existing");
    world.write_settings(SETTINGS, 0o644);
    let fake = FakeClaude::new(Mode::Honest);
    installed(&world, &fake);
    assert_eq!(world.mode(), 0o644);
    let settings = world.settings_json();
    assert_eq!(settings["model"], json!("opus"));
    assert_eq!(settings["permissions"], json!({"allow": ["Bash(ls:*)"]}));
    assert_eq!(
        settings["hooks"]["Stop"][0]["hooks"][0]["command"],
        json!("afplay done.aiff")
    );
    assert_eq!(
        settings["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        json!(CMD)
    );
    let leftovers: Vec<_> = fs::read_dir(&world.config)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(leftovers, ["settings.json"]);
}

#[test]
fn the_consent_screen_shows_the_whole_change_before_anything_is_written() {
    let world = world("cs-screen");
    world.write_settings(SETTINGS, 0o644);
    let fake = FakeClaude::new(Mode::Honest);
    let shown = std::cell::RefCell::new(String::new());
    let outcome = install(&world.env(Some(&fake)), &mut |plan: &str| {
        *shown.borrow_mut() = plan.to_owned();
        assert_eq!(fs::read_to_string(world.settings()).unwrap(), SETTINGS);
        assert!(fake.state.borrow().is_none());
        assert!(world.manifest().is_none());
        false
    })
    .unwrap();
    assert!(matches!(outcome, InstallOutcome::Declined));
    let shown = shown.into_inner();
    for needed in [
        world.settings().to_str().unwrap(),
        world.config.to_str().unwrap(),
        world.data.to_str().unwrap(),
        EXE,
        "--- ",
        "+++ ",
        "+    \"SessionStart\": [",
        &format!("claude mcp add --scope user agentdust -- {EXE} mcp"),
    ] {
        assert!(shown.contains(needed), "missing {needed:?} in:\n{shown}");
    }
    assert!(!shown.contains("-    \"Stop\""), "{shown}");
}

#[test]
fn declining_leaves_everything_untouched() {
    let world = world("cs-decline");
    world.write_settings(SETTINGS, 0o644);
    let fake = FakeClaude::new(Mode::Honest);
    let outcome = install(&world.env(Some(&fake)), &mut |_: &str| false).unwrap();
    assert!(matches!(outcome, InstallOutcome::Declined));
    assert_eq!(fs::read_to_string(world.settings()).unwrap(), SETTINGS);
    assert!(world.manifest().is_none());
    assert!(fake.state.borrow().is_none());
    assert_eq!(fake.calls(), ["mcp get agentdust"]);
    assert!(!world.data.join(MANIFEST_FILE).exists());
}

#[test]
fn a_second_run_changes_nothing_and_does_not_ask() {
    let world = world("cs-idempotent");
    world.write_settings(SETTINGS, 0o644);
    let fake = FakeClaude::new(Mode::Honest);
    installed(&world, &fake);
    let settings = fs::read(world.settings()).unwrap();
    let manifest = fs::read(world.data.join(MANIFEST_FILE)).unwrap();
    let before = fake.calls().len();
    let outcome = install(&world.env(Some(&fake)), &mut |_: &str| panic!("asked again")).unwrap();
    match outcome {
        InstallOutcome::Nothing { complete, .. } => assert!(complete),
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read(world.settings()).unwrap(), settings);
    assert_eq!(fs::read(world.data.join(MANIFEST_FILE)).unwrap(), manifest);
    assert_eq!(fake.calls()[before..], ["mcp get agentdust"]);
}

#[test]
fn check_reports_installed_after_setup_and_not_installed_before() {
    let world = world("cs-check");
    let fake = FakeClaude::new(Mode::Honest);
    let before = check(&world.env(Some(&fake)), true);
    assert_eq!(before.overall(), Overall::NotInstalled);
    assert!(!before.ok());
    installed(&world, &fake);
    let after = check(&world.env(Some(&fake)), true);
    assert_eq!(after.overall(), Overall::Installed);
    assert!(after.ok());
    assert_eq!(after.mcp, McpState::Installed);
    let states: Vec<_> = after
        .hooks
        .as_ref()
        .unwrap()
        .iter()
        .map(|hook| hook.state)
        .collect();
    assert_eq!(states, vec![HookState::Installed; 4]);
}

#[test]
fn check_detects_a_hand_edited_entry() {
    let world = world("cs-drift");
    let fake = FakeClaude::new(Mode::Honest);
    installed(&world, &fake);
    let edited =
        fs::read_to_string(world.settings())
            .unwrap()
            .replacen("\"timeout\": 10", "\"timeout\": 99", 1);
    fs::write(world.settings(), &edited).unwrap();
    let report = check(&world.env(Some(&fake)), true);
    assert_eq!(report.overall(), Overall::Modified);
    assert!(!report.ok());
    assert!(report.render().contains("modified"), "{}", report.render());
    let rerun = install(&world.env(Some(&fake)), &mut approve).unwrap();
    match rerun {
        InstallOutcome::Nothing { complete, .. } => assert!(!complete),
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read_to_string(world.settings()).unwrap(), edited);
}

#[test]
fn check_detects_a_missing_entry_and_a_changed_registration() {
    let world = world("cs-missing");
    let fake = FakeClaude::new(Mode::Honest);
    installed(&world, &fake);
    let mut value = world.settings_json();
    value["hooks"].as_object_mut().unwrap().remove("SessionEnd");
    fs::write(world.settings(), value.to_string()).unwrap();
    assert_eq!(check(&world.env(Some(&fake)), true).overall(), Overall::Partial);
    *fake.state.borrow_mut() = None;
    let report = check(&world.env(Some(&fake)), true);
    assert_eq!(report.mcp, McpState::Missing);
    fake.state.replace(Some(support::Registered {
        command: "/changed".to_owned(),
        args: vec!["mcp".to_owned()],
        scope: "User config (available in all your projects)",
    }));
    assert_eq!(check(&world.env(Some(&fake)), true).mcp, McpState::Modified);
}

#[test]
fn removal_takes_out_only_what_setup_added() {
    let world = world("cs-remove");
    world.write_settings(SETTINGS, 0o644);
    let fake = FakeClaude::new(Mode::Honest);
    installed(&world, &fake);
    let mut value = world.settings_json();
    value["hooks"]["Notification"] = json!([{"hooks": [{"type": "command", "command": "afplay ping.aiff"}]}]);
    value["theme"] = json!("dark");
    fs::write(world.settings(), serde_json::to_string_pretty(&value).unwrap()).unwrap();
    let outcome = remove(&world.env(Some(&fake)), &mut approve).unwrap();
    match outcome {
        RemoveOutcome::Applied(report) => assert!(report.complete, "{}", report.text),
        other => panic!("{other:?}"),
    }
    let after = world.settings_json();
    assert_eq!(after["theme"], json!("dark"));
    assert_eq!(after["model"], json!("opus"));
    assert_eq!(
        after["hooks"]["Stop"][0]["hooks"][0]["command"],
        json!("afplay done.aiff")
    );
    assert_eq!(
        after["hooks"]["Notification"][0]["hooks"][0]["command"],
        json!("afplay ping.aiff")
    );
    for spec in HOOK_SPECS {
        assert!(after["hooks"].get(spec.event).is_none(), "{}", spec.event);
    }
    assert!(fake.state.borrow().is_none());
    assert!(world.manifest().is_none());
}

#[test]
fn removal_restores_the_exact_file_when_nothing_else_changed() {
    let world = world("cs-remove-exact");
    world.write_settings(SETTINGS, 0o644);
    let fake = FakeClaude::new(Mode::Honest);
    installed(&world, &fake);
    assert_ne!(fs::read_to_string(world.settings()).unwrap(), SETTINGS);
    remove(&world.env(Some(&fake)), &mut approve).unwrap();
    assert_eq!(fs::read_to_string(world.settings()).unwrap(), SETTINGS);
    assert_eq!(world.mode(), 0o644);
}

#[test]
fn removal_leaves_an_edited_entry_and_a_changed_registration_and_says_so() {
    let world = world("cs-remove-drift");
    let fake = FakeClaude::new(Mode::Honest);
    installed(&world, &fake);
    let edited =
        fs::read_to_string(world.settings())
            .unwrap()
            .replacen("\"timeout\": 10", "\"timeout\": 99", 1);
    fs::write(world.settings(), &edited).unwrap();
    fake.state.replace(Some(support::Registered {
        command: "/changed".to_owned(),
        args: vec!["mcp".to_owned()],
        scope: "User config (available in all your projects)",
    }));
    let outcome = remove(&world.env(Some(&fake)), &mut approve).unwrap();
    match outcome {
        RemoveOutcome::Applied(report) => {
            assert!(!report.complete);
            assert!(report.text.contains("modified"), "{}", report.text);
        }
        other => panic!("{other:?}"),
    }
    assert!(
        fs::read_to_string(world.settings())
            .unwrap()
            .contains("\"timeout\": 99")
    );
    assert_eq!(fake.state.borrow().as_ref().unwrap().command, "/changed");
    let manifest = world.manifest().unwrap();
    assert_eq!(manifest.entries.len(), 2);
    assert!(mcp_entry(&manifest).is_some());
}

#[test]
fn removal_never_touches_entries_the_user_already_had() {
    let world = world("cs-remove-preexisting");
    let theirs = json!({"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": CMD}]}]}});
    world.write_settings(&serde_json::to_string_pretty(&theirs).unwrap(), 0o644);
    let fake = FakeClaude::with(Mode::Honest, EXE, &["mcp"]);
    installed(&world, &fake);
    let manifest = world.manifest().unwrap();
    assert!(
        manifest
            .entries
            .iter()
            .filter(|entry| entry.key == "SessionStart" || entry.resource == Resource::McpServer)
            .all(|entry| entry.origin == Origin::PreExisting)
    );
    assert!(!fake.calls().iter().any(|call| call.starts_with("mcp add")));
    remove(&world.env(Some(&fake)), &mut approve).unwrap();
    let after = world.settings_json();
    assert_eq!(after["hooks"]["SessionStart"], theirs["hooks"]["SessionStart"]);
    assert!(after["hooks"].get("SessionEnd").is_none());
    assert!(fake.state.borrow().is_some());
    assert!(!fake.calls().iter().any(|call| call.starts_with("mcp remove")));
}

#[test]
fn removal_without_a_manifest_removes_nothing() {
    let world = world("cs-remove-none");
    world.write_settings(&format!("{{\"hooks\": {{\"Stop\": [{{\"hooks\": [{{\"type\": \"command\", \"command\": \"{CMD}\"}}]}}]}}}}"), 0o644);
    let fake = FakeClaude::with(Mode::Honest, EXE, &["mcp"]);
    let before = fs::read(world.settings()).unwrap();
    let outcome = remove(&world.env(Some(&fake)), &mut approve).unwrap();
    assert!(matches!(outcome, RemoveOutcome::Nothing { .. }));
    assert_eq!(fs::read(world.settings()).unwrap(), before);
    assert!(fake.state.borrow().is_some());
}

#[test]
fn a_symlinked_settings_file_is_never_edited_and_the_change_is_printed() {
    let world = world("cs-symlink");
    let target = world.dir.join("dotfiles-settings.json");
    fs::write(&target, SETTINGS).unwrap();
    symlink(&target, world.settings()).unwrap();
    let fake = FakeClaude::new(Mode::Honest);
    let shown = std::cell::RefCell::new(String::new());
    let outcome = install(&world.env(Some(&fake)), &mut |plan: &str| {
        *shown.borrow_mut() = plan.to_owned();
        true
    })
    .unwrap();
    let report = applied(outcome);
    assert!(!report.succeeded());
    assert_eq!(fs::read_to_string(&target).unwrap(), SETTINGS);
    assert!(
        fs::symlink_metadata(world.settings())
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let shown = shown.into_inner();
    assert!(shown.contains("symbolic link"), "{shown}");
    assert!(shown.contains("\"SessionStart\""), "{shown}");
    assert!(shown.contains(CMD), "{shown}");
    assert!(fake.state.borrow().is_some());
    assert!(hook_entries(&world.manifest().unwrap()).is_empty());
}

#[test]
fn a_hard_linked_or_non_regular_settings_file_is_refused_too() {
    let world = world("cs-hardlink");
    world.write_settings(SETTINGS, 0o644);
    fs::hard_link(world.settings(), world.dir.join("elsewhere.json")).unwrap();
    let fake = FakeClaude::new(Mode::Honest);
    let report = applied(install(&world.env(Some(&fake)), &mut approve).unwrap());
    assert!(!report.succeeded());
    assert!(report.render().contains("hard links"), "{}", report.render());
    assert_eq!(fs::read_to_string(world.settings()).unwrap(), SETTINGS);
    fs::remove_file(world.dir.join("elsewhere.json")).unwrap();
    fs::remove_file(world.settings()).unwrap();
    fs::create_dir(world.settings()).unwrap();
    let fresh = FakeClaude::new(Mode::Honest);
    let report = applied(install(&world.env(Some(&fresh)), &mut approve).unwrap());
    assert!(!report.succeeded());
    assert!(
        report.render().contains("not a regular file"),
        "{}",
        report.render()
    );
    assert!(world.settings().is_dir());
}

#[test]
fn a_symlinked_settings_file_can_still_be_checked_read_only() {
    let world = world("cs-symlink-check");
    let fake = FakeClaude::new(Mode::Honest);
    installed(&world, &fake);
    let real = world.dir.join("dotfiles-settings.json");
    fs::rename(world.settings(), &real).unwrap();
    symlink(&real, world.settings()).unwrap();
    let report = check(&world.env(Some(&fake)), true);
    assert_eq!(report.overall(), Overall::Installed, "{}", report.render());
}

#[test]
fn an_unreadable_settings_file_blocks_the_hooks_and_changes_nothing() {
    if is_root() {
        return;
    }
    let world = world("cs-unreadable");
    world.write_settings(SETTINGS, 0o000);
    let fake = FakeClaude::new(Mode::Honest);
    let report = applied(install(&world.env(Some(&fake)), &mut approve).unwrap());
    assert!(!report.succeeded());
    assert!(report.render().contains("cannot be read"), "{}", report.render());
    assert_eq!(world.mode(), 0o000);
    fs::set_permissions(world.settings(), fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(fs::read_to_string(world.settings()).unwrap(), SETTINGS);
    let checked = check(&world.env(Some(&fake)), true);
    assert!(!checked.ok());
}

#[test]
fn invalid_json_and_odd_shapes_block_the_hooks_and_are_never_repaired() {
    for (name, text) in [
        ("cs-bad-json", "{ not json"),
        ("cs-bad-shape", "{\"hooks\": []}"),
        ("cs-bad-dup", "{\"hooks\": {}, \"hooks\": {}}"),
    ] {
        let world = world(name);
        world.write_settings(text, 0o644);
        let fake = FakeClaude::new(Mode::Honest);
        let report = applied(install(&world.env(Some(&fake)), &mut approve).unwrap());
        assert!(!report.succeeded(), "{text}");
        assert_eq!(fs::read_to_string(world.settings()).unwrap(), text);
    }
}

#[test]
fn a_cli_that_lies_fails_the_install_and_rolls_everything_back() {
    let world = world("cs-liar");
    world.write_settings(SETTINGS, 0o644);
    let fake = FakeClaude::new(Mode::Liar);
    let report = applied(install(&world.env(Some(&fake)), &mut approve).unwrap());
    assert!(!report.succeeded());
    let failure = report.transaction.failure.as_ref().unwrap();
    assert!(failure.reason.contains("no such server"), "{}", failure.reason);
    assert_eq!(fs::read_to_string(world.settings()).unwrap(), SETTINGS);
    assert_eq!(world.mode(), 0o644);
    assert!(world.manifest().is_none());
    assert!(report.transaction.rolled_back_cleanly());
    assert_eq!(report.transaction.rollback.len(), 2);
    let text = report.render();
    assert!(
        text.contains("rolled back") || text.contains("Rolled back"),
        "{text}"
    );
    assert!(text.contains("no such server"), "{text}");
}

#[test]
fn a_failed_install_into_a_fresh_config_directory_leaves_no_settings_file() {
    let world = world("cs-liar-fresh");
    let fake = FakeClaude::new(Mode::AddFails);
    let report = applied(install(&world.env(Some(&fake)), &mut approve).unwrap());
    assert!(!report.succeeded());
    assert!(!world.settings().exists());
    assert!(world.manifest().is_none());
}

#[test]
fn a_cli_that_registers_something_else_is_not_removed_and_not_trusted() {
    let world = world("cs-else");
    let fake = FakeClaude::new(Mode::AddsSomethingElse);
    let report = applied(install(&world.env(Some(&fake)), &mut approve).unwrap());
    assert!(!report.succeeded());
    assert!(report.render().contains("/somewhere/else"), "{}", report.render());
    assert!(!world.settings().exists());
    assert!(fake.state.borrow().is_some());
    assert!(!fake.calls().iter().any(|call| call.starts_with("mcp remove")));
}

#[test]
fn a_file_that_changes_after_the_diff_aborts_the_install_and_rolls_back() {
    let world = world("cs-race");
    world.write_settings(SETTINGS, 0o644);
    let fake = FakeClaude::new(Mode::Honest);
    let changed = "{\n  \"model\": \"sonnet\"\n}\n";
    let outcome = install(&world.env(Some(&fake)), &mut |_: &str| {
        fs::write(world.settings(), changed).unwrap();
        true
    })
    .unwrap();
    let report = applied(outcome);
    assert!(!report.succeeded());
    assert_eq!(fs::read_to_string(world.settings()).unwrap(), changed);
    assert!(world.manifest().is_none());
    assert!(fake.state.borrow().is_none());
    assert!(
        report.render().contains("changed after it was read"),
        "{}",
        report.render()
    );
}

#[test]
fn a_write_failure_halfway_reports_what_was_rolled_back() {
    if is_root() {
        return;
    }
    let world = world("cs-readonly");
    world.write_settings(SETTINGS, 0o644);
    fs::set_permissions(&world.config, fs::Permissions::from_mode(0o555)).unwrap();
    let fake = FakeClaude::new(Mode::Honest);
    let report = applied(install(&world.env(Some(&fake)), &mut approve).unwrap());
    fs::set_permissions(&world.config, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!report.succeeded());
    let failure = report.transaction.failure.as_ref().unwrap();
    assert!(failure.step.contains("settings.json"), "{}", failure.step);
    assert_eq!(report.transaction.rollback.len(), 1);
    assert!(report.transaction.rolled_back_cleanly());
    assert!(world.manifest().is_none());
    assert_eq!(fs::read_to_string(world.settings()).unwrap(), SETTINGS);
    assert!(fake.state.borrow().is_none());
}

#[test]
fn without_the_claude_cli_the_hooks_are_installed_and_the_manual_command_is_printed() {
    let world = world("cs-no-cli");
    let shown = std::cell::RefCell::new(String::new());
    let outcome = install(&world.env(None), &mut |plan: &str| {
        *shown.borrow_mut() = plan.to_owned();
        true
    })
    .unwrap();
    let report = applied(outcome);
    assert!(report.succeeded(), "{}", report.render());
    let manual = format!("claude mcp add --scope user agentdust -- {EXE} mcp");
    assert!(shown.borrow().contains(&manual), "{}", shown.borrow());
    assert!(report.render().contains(&manual), "{}", report.render());
    assert_eq!(
        world.settings_json()["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        json!(CMD)
    );
    let manifest = world.manifest().unwrap();
    assert!(mcp_entry(&manifest).is_none());
    assert!(manifest.agent_executables.is_empty());
    let checked = check(&world.env(None), true);
    assert!(!checked.ok());
    assert_eq!(checked.mcp, McpState::NoCli);
}

#[test]
fn a_manual_command_quotes_a_binary_path_that_needs_it() {
    let world = world("cs-no-cli-quote");
    let mut env = world.env(None);
    env.exe = PathBuf::from("/Users/Jo Doe/bin/agentdust");
    let report = applied(install(&env, &mut approve).unwrap());
    assert!(
        report
            .render()
            .contains("claude mcp add --scope user agentdust -- '/Users/Jo Doe/bin/agentdust' mcp"),
        "{}",
        report.render()
    );
    assert_eq!(
        world.settings_json()["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        json!("'/Users/Jo Doe/bin/agentdust' hook claude")
    );
}

#[test]
fn a_server_with_the_same_name_but_another_command_is_never_overwritten() {
    let world = world("cs-conflict");
    let fake = FakeClaude::with(Mode::Honest, "/theirs/agentdust", &["serve"]);
    let report = applied(install(&world.env(Some(&fake)), &mut approve).unwrap());
    assert!(!report.succeeded());
    assert!(
        report.render().contains("/theirs/agentdust"),
        "{}",
        report.render()
    );
    assert_eq!(fake.state.borrow().as_ref().unwrap().command, "/theirs/agentdust");
    assert!(
        !fake
            .calls()
            .iter()
            .any(|call| call.starts_with("mcp add") || call.starts_with("mcp remove"))
    );
    match check(&world.env(Some(&fake)), true).mcp {
        McpState::Conflict(reason) => assert!(reason.contains("/theirs/agentdust"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert!(mcp_entry(&world.manifest().unwrap()).is_none());
}

#[test]
fn a_server_that_exists_in_project_scope_counts_as_a_conflict() {
    let world = world("cs-conflict-scope");
    let fake = FakeClaude::with(Mode::Honest, EXE, &["mcp"]);
    fake.state.borrow_mut().as_mut().unwrap().scope = "Project config (shared via .mcp.json)";
    assert!(matches!(
        check(&world.env(Some(&fake)), true).mcp,
        McpState::Conflict(_)
    ));
}

#[test]
fn an_entry_written_for_another_binary_is_reported_stale_and_never_doubled() {
    let world = world("cs-stale");
    let fake = FakeClaude::new(Mode::Honest);
    installed(&world, &fake);
    let mut env = world.env(Some(&fake));
    env.exe = PathBuf::from("/Users/u/target/debug/agentdust");
    let report = check(&env, true);
    assert_eq!(report.overall(), Overall::Modified);
    assert_eq!(report.mcp, McpState::Stale);
    let before = fs::read(world.settings()).unwrap();
    let outcome = install(&env, &mut approve).unwrap();
    match outcome {
        InstallOutcome::Nothing { complete, .. } => assert!(!complete),
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read(world.settings()).unwrap(), before);
    assert_eq!(
        world.settings_json()["hooks"]["SessionStart"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_manifest_from_a_future_version_refuses_install_remove_and_check() {
    let world = world("cs-future");
    let path = world.data.join(MANIFEST_FILE);
    fs::write(&path, "{\"version\":2,\"entries\":[]}").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let fake = FakeClaude::new(Mode::Honest);
    assert!(matches!(
        install(&world.env(Some(&fake)), &mut approve),
        Err(SetupError::Manifest(ManifestError::UnknownVersion { found: 2 }))
    ));
    assert!(matches!(
        remove(&world.env(Some(&fake)), &mut approve),
        Err(SetupError::Manifest(ManifestError::UnknownVersion { found: 2 }))
    ));
    let report = check(&world.env(Some(&fake)), true);
    assert!(!report.ok());
    assert!(report.render().contains("version 2"), "{}", report.render());
    assert!(!world.settings().exists());
    assert!(fake.calls().is_empty());
}

#[test]
fn a_data_directory_that_is_not_private_fails_before_any_file_is_edited() {
    let world = world("cs-loose-data");
    fs::set_permissions(&world.data, fs::Permissions::from_mode(0o755)).unwrap();
    world.write_settings(SETTINGS, 0o644);
    let fake = FakeClaude::new(Mode::Honest);
    assert!(matches!(
        install(&world.env(Some(&fake)), &mut approve),
        Err(SetupError::Manifest(ManifestError::Refused(_)))
    ));
    assert_eq!(fs::read_to_string(world.settings()).unwrap(), SETTINGS);
    assert!(fake.calls().is_empty());
}

#[test]
fn entries_for_another_config_directory_are_left_alone() {
    let world = world("cs-two-configs");
    let other = Entry {
        resource: Resource::Hook,
        target: "/elsewhere/.claude/settings.json".to_owned(),
        key: "SessionStart".to_owned(),
        origin: Origin::Created,
        command: "/old/agentdust hook claude".to_owned(),
        args: Vec::new(),
        hash: "00".repeat(32),
        created_hooks_key: true,
        created_event_key: true,
    };
    manifest::store(
        &world.data,
        &Manifest {
            entries: vec![other.clone()],
            ..Manifest::new()
        },
    )
    .unwrap();
    let fake = FakeClaude::new(Mode::Honest);
    installed(&world, &fake);
    assert!(world.manifest().unwrap().entries.contains(&other));
    remove(&world.env(Some(&fake)), &mut approve).unwrap();
    assert_eq!(world.manifest().unwrap().entries, vec![other]);
}

#[test]
fn the_cli_is_asked_nothing_destructive_during_a_check() {
    let world = world("cs-check-readonly");
    let fake = FakeClaude::new(Mode::Honest);
    installed(&world, &fake);
    let before = fake.calls().len();
    check(&world.env(Some(&fake)), true);
    assert_eq!(fake.calls()[before..], ["mcp get agentdust"]);
    let before = fake.calls().len();
    let unverified = check(&world.env(Some(&fake)), false);
    assert_eq!(fake.calls().len(), before);
    assert_eq!(unverified.mcp, McpState::Recorded);
    assert_eq!(unverified.overall(), Overall::Installed);
}

#[test]
fn an_unreadable_listing_blocks_the_registration_without_touching_the_cli_config() {
    let world = world("cs-unknown");
    let fake = FakeClaude::new(Mode::UnreadableGet);
    let report = applied(install(&world.env(Some(&fake)), &mut approve).unwrap());
    assert!(!report.succeeded());
    assert!(!fake.calls().iter().any(|call| call.starts_with("mcp add")));
    assert!(
        report
            .render()
            .contains("claude mcp add --scope user agentdust --"),
        "{}",
        report.render()
    );
}

#[test]
fn the_asked_plan_is_only_requested_once_and_only_when_something_changes() {
    let world = world("cs-ask-count");
    let fake = FakeClaude::new(Mode::Honest);
    let asked = Cell::new(0);
    install(&world.env(Some(&fake)), &mut |_: &str| {
        asked.set(asked.get() + 1);
        true
    })
    .unwrap();
    assert_eq!(asked.get(), 1);
}

struct RewritesSettingsOnFirstCall<'a> {
    inner: &'a FakeClaude,
    settings: PathBuf,
    rewrite: Box<dyn Fn(&str) -> String + 'a>,
    done: Cell<bool>,
}

impl CliRunner for RewritesSettingsOnFirstCall<'_> {
    fn run(
        &self,
        args: &[&str],
    ) -> Result<agentdust_agents::native_cli::CliOutput, agentdust_agents::native_cli::CliError> {
        if !self.done.replace(true) {
            let current = fs::read_to_string(&self.settings).unwrap();
            fs::write(&self.settings, (self.rewrite)(&current)).unwrap();
        }
        self.inner.run(args)
    }
}

#[test]
fn a_cli_that_rewrites_settings_on_its_first_call_does_not_break_the_install() {
    let world = world("cs-migrate-install");
    world.write_settings("{\"model\":\"opus\",\"env\":{\"A\":\"1\"}}", 0o644);
    let fake = FakeClaude::new(Mode::Honest);
    let migrating = RewritesSettingsOnFirstCall {
        inner: &fake,
        settings: world.settings(),
        rewrite: Box::new(|_| {
            "{\n  \"env\": {\n    \"A\": \"1\"\n  },\n  \"model\": \"opus[1m]\"\n}\n".to_owned()
        }),
        done: Cell::new(false),
    };
    let report = applied(install(&world.env(Some(&migrating)), &mut approve).unwrap());
    assert!(report.succeeded(), "{}", report.render());
    let settings = world.settings_json();
    assert_eq!(settings["model"], json!("opus[1m]"));
    assert_eq!(settings["env"], json!({"A": "1"}));
    assert_eq!(
        settings["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        json!(CMD)
    );
}

#[test]
fn a_cli_that_rewrites_settings_on_its_first_call_does_not_break_the_removal() {
    let world = world("cs-migrate-remove");
    world.write_settings(SETTINGS, 0o644);
    let fake = FakeClaude::new(Mode::Honest);
    installed(&world, &fake);
    let migrating = RewritesSettingsOnFirstCall {
        inner: &fake,
        settings: world.settings(),
        rewrite: Box::new(|current| {
            let value: Value = serde_json::from_str(current).unwrap();
            serde_json::to_string_pretty(&value)
                .unwrap()
                .replace("  ", "    ")
        }),
        done: Cell::new(false),
    };
    let outcome = remove(&world.env(Some(&migrating)), &mut approve).unwrap();
    match outcome {
        RemoveOutcome::Applied(report) => assert!(report.complete, "{}", report.text),
        other => panic!("{other:?}"),
    }
    let after = world.settings_json();
    for spec in HOOK_SPECS {
        assert!(after["hooks"].get(spec.event).is_none(), "{}", spec.event);
    }
    assert_eq!(after["model"], json!("opus"));
    assert!(fake.state.borrow().is_none());
}
