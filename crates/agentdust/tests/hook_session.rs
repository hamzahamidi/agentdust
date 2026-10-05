mod common;

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Output;

use agentdust_core::digest::{Domain, keyed_digest};
use agentdust_core::identity::ProcessIdentity;
use agentdust_core::journal::{self, AgentIdentity, Kind, Record};
use agentdust_core::secret::SECRET_FILE;
use common::chain::{Chain, Outcome, run_hop};
use common::{make_fifo, private_dir, run_hook_guarded, run_hook_with, scratch_dir};

const TAG_PREFIX: &str = "export AGENTDUST_SESSION=";
const FIXTURE_CWD: &str = "/agentdust-fixture/project";

#[test]
#[ignore = "runs only as a hop of a process chain that another test started"]
fn relay_hop() {
    run_hop();
}

fn event(name: &str) -> Vec<u8> {
    format!(
        r#"{{"session_id":"s1","hook_event_name":"{name}","tool_name":"Bash","tool_use_id":"toolu_1","cwd":"{FIXTURE_CWD}","source":"startup"}}"#
    )
    .into_bytes()
}

fn start() -> Vec<u8> {
    event("SessionStart")
}

fn export_lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

fn tag_of(line: &str) -> &str {
    let tag = line
        .strip_prefix(TAG_PREFIX)
        .unwrap_or_else(|| panic!("{line:?}"));
    assert_eq!(tag.len(), 32, "{line:?}");
    assert!(
        tag.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
        "{line:?}"
    );
    tag
}

fn secret_bytes(dir: &Path) -> Vec<u8> {
    fs::read(dir.join(SECRET_FILE)).unwrap()
}

fn key_of(dir: &Path, tag: &str) -> String {
    keyed_digest(&secret_bytes(dir), Domain::Session, tag.as_bytes())
}

fn records(dir: &Path) -> Vec<Record> {
    journal::read(dir).unwrap().records
}

fn identity_of(process: &ProcessIdentity) -> AgentIdentity {
    AgentIdentity::from_process(process).unwrap()
}

fn run_plain(dir: &Path, env_file: Option<&Path>, input: &[u8]) -> Output {
    let mut envs: Vec<(&str, &OsStr)> = vec![("AGENTDUST_DATA_DIR", dir.as_os_str())];
    if let Some(path) = env_file {
        envs.push(("CLAUDE_ENV_FILE", path.as_os_str()));
    }
    run_hook_with(["hook", "claude"], &envs, None, input)
}

fn assert_silent(output: &Output) {
    assert!(output.status.success(), "{:?}", output.status);
    assert!(output.stdout.is_empty());
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_chain(chain: &Chain, dir: &Path, env_file: Option<&Path>, input: &[u8]) -> Outcome {
    chain.run(dir, input, env_file, &[], None)
}

#[test]
fn a_session_start_hands_the_shell_one_tag_and_the_journal_only_its_keyed_digest() {
    let dir = scratch_dir("session-tag");
    let work = private_dir("session-tag-work");
    let env_file = work.join("env.sh");
    let outcome = run_chain(
        &Chain::within(&work, "chain").claude(),
        &dir,
        Some(&env_file),
        &start(),
    );
    outcome.assert_silent_success();
    let lines = export_lines(&env_file);
    assert_eq!(lines.len(), 1, "{lines:?}");
    let tag = tag_of(&lines[0]).to_owned();
    let recorded = records(&dir);
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].kind, Kind::SessionStart);
    assert_eq!(
        recorded[0]
            .session_tag_key
            .as_ref()
            .map(|key| key.as_str().to_owned()),
        Some(key_of(&dir, &tag))
    );
    for entry in fs::read_dir(&dir).unwrap() {
        let bytes = fs::read(entry.unwrap().path()).unwrap();
        assert!(!bytes.windows(tag.len()).any(|window| window == tag.as_bytes()));
    }
    fs::remove_dir_all(&dir).unwrap();
    fs::remove_dir_all(&work).unwrap();
}

#[test]
fn every_session_start_gets_a_fresh_tag_and_key() {
    let dir = scratch_dir("session-fresh");
    let work = private_dir("session-fresh-work");
    let env_file = work.join("env.sh");
    for _ in 0..3 {
        assert_silent(&run_plain(&dir, Some(&env_file), &start()));
    }
    let lines = export_lines(&env_file);
    let tags: BTreeSet<&str> = lines.iter().map(|line| tag_of(line)).collect();
    assert_eq!(tags.len(), 3);
    let keys: BTreeSet<String> = records(&dir)
        .iter()
        .map(|record| record.session_tag_key.as_ref().unwrap().as_str().to_owned())
        .collect();
    assert_eq!(keys, tags.iter().map(|tag| key_of(&dir, tag)).collect());
    fs::remove_dir_all(&dir).unwrap();
    fs::remove_dir_all(&work).unwrap();
}

#[test]
fn the_tag_is_appended_to_what_the_env_file_holds() {
    let dir = scratch_dir("session-append");
    let work = private_dir("session-append-work");
    let env_file = work.join("env.sh");
    fs::write(&env_file, "export KEEP=1\n").unwrap();
    assert_silent(&run_plain(&dir, Some(&env_file), &start()));
    let text = fs::read_to_string(&env_file).unwrap();
    assert!(text.starts_with("export KEEP=1\n"), "{text:?}");
    assert_eq!(export_lines(&env_file).len(), 2);
    fs::remove_dir_all(&dir).unwrap();
    fs::remove_dir_all(&work).unwrap();
}

#[test]
fn a_session_start_without_an_env_file_still_records_its_key() {
    let dir = scratch_dir("session-no-env");
    assert_silent(&run_plain(&dir, None, &start()));
    let recorded = records(&dir);
    assert_eq!(recorded.len(), 1);
    assert!(recorded[0].session_tag_key.is_some());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_empty_env_file_variable_is_not_a_path() {
    let dir = scratch_dir("session-empty-env");
    let output = run_hook_with(
        ["hook", "claude"],
        &[
            ("AGENTDUST_DATA_DIR", dir.as_os_str()),
            ("CLAUDE_ENV_FILE", OsStr::new("")),
        ],
        None,
        &start(),
    );
    assert_silent(&output);
    assert_eq!(records(&dir).len(), 1);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn events_after_the_start_neither_carry_a_tag_nor_touch_the_env_file() {
    let dir = scratch_dir("session-others");
    let work = private_dir("session-others-work");
    let env_file = work.join("env.sh");
    for name in ["PreToolUse", "PostToolUse", "SessionEnd"] {
        assert_silent(&run_plain(&dir, Some(&env_file), &event(name)));
    }
    assert!(!env_file.exists());
    let recorded = records(&dir);
    assert_eq!(recorded.len(), 3);
    assert!(recorded.iter().all(|record| record.session_tag_key.is_none()));
    fs::remove_dir_all(&dir).unwrap();
    fs::remove_dir_all(&work).unwrap();
}

#[test]
fn events_that_are_not_journaled_hand_out_no_tag() {
    let dir = scratch_dir("session-not-journaled");
    let work = private_dir("session-not-journaled-work");
    let env_file = work.join("env.sh");
    assert_silent(&run_plain(
        &dir,
        Some(&env_file),
        br#"{"session_id":"s1","hook_event_name":"Stop"}"#,
    ));
    assert!(!env_file.exists());
    assert!(!dir.exists());
    fs::remove_dir_all(&work).unwrap();
}

#[test]
fn the_start_still_carries_the_working_directory_key() {
    let dir = scratch_dir("session-cwd");
    assert_silent(&run_plain(&dir, None, &start()));
    assert!(records(&dir)[0].cwd_key.is_some());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_parent_named_claude_is_recorded_as_the_agent_of_the_session() {
    let dir = scratch_dir("session-claude-parent");
    let work = private_dir("session-claude-parent-work");
    let outcome = run_chain(&Chain::within(&work, "chain").claude(), &dir, None, &start());
    outcome.assert_silent_success();
    let recorded = records(&dir);
    assert_eq!(recorded[0].agent_identity, Some(identity_of(&outcome.hops[0])));
    assert_eq!(
        recorded[0]
            .agent_identity
            .as_ref()
            .and_then(|id| id.exe_base())
            .map(|name| name.as_str()),
        Some("claude")
    );
    fs::remove_dir_all(&dir).unwrap();
    fs::remove_dir_all(&work).unwrap();
}

#[test]
fn the_agent_is_found_above_a_shell() {
    let dir = scratch_dir("session-above-shell");
    let work = private_dir("session-above-shell-work");
    let outcome = run_chain(
        &Chain::within(&work, "chain").claude().relay(),
        &dir,
        None,
        &start(),
    );
    outcome.assert_silent_success();
    assert_eq!(outcome.hops.len(), 2);
    assert_eq!(
        records(&dir)[0].agent_identity,
        Some(identity_of(&outcome.hops[0]))
    );
    fs::remove_dir_all(&dir).unwrap();
    fs::remove_dir_all(&work).unwrap();
}

#[test]
fn the_nearest_agent_is_the_one_recorded() {
    let dir = scratch_dir("session-nearest");
    let work = private_dir("session-nearest-work");
    let chain = Chain::within(&work, "chain").claude().relay().claude().relay();
    let outcome = run_chain(&chain, &dir, None, &start());
    outcome.assert_silent_success();
    assert_eq!(outcome.hops.len(), 4);
    assert_eq!(
        records(&dir)[0].agent_identity,
        Some(identity_of(&outcome.hops[2]))
    );
    assert_ne!(identity_of(&outcome.hops[0]), identity_of(&outcome.hops[2]));
    fs::remove_dir_all(&dir).unwrap();
    fs::remove_dir_all(&work).unwrap();
}

#[test]
fn a_node_running_a_script_that_mentions_claude_is_recorded_as_node() {
    let dir = scratch_dir("session-node");
    let work = private_dir("session-node-work");
    let script = "/agentdust-fixture/node_modules/@anthropic-ai/claude-code/cli.js";
    let outcome = run_chain(
        &Chain::within(&work, "chain").node(script).relay(),
        &dir,
        None,
        &start(),
    );
    outcome.assert_silent_success();
    let identity = records(&dir)[0].agent_identity.clone().unwrap();
    assert_eq!(identity, identity_of(&outcome.hops[0]));
    assert_eq!(identity.exe_base().map(|name| name.as_str()), Some("node"));
    fs::remove_dir_all(&dir).unwrap();
    fs::remove_dir_all(&work).unwrap();
}

#[test]
fn a_node_running_another_script_is_not_recorded_as_the_agent() {
    let dir = scratch_dir("session-node-other");
    let work = private_dir("session-node-other-work");
    let chain = Chain::within(&work, "chain")
        .relay()
        .node("/agentdust-fixture/server.js")
        .orphan();
    let outcome = run_chain(&chain, &dir, None, &start());
    outcome.assert_silent_success();
    assert_eq!(records(&dir)[0].agent_identity, None);
    fs::remove_dir_all(&dir).unwrap();
    fs::remove_dir_all(&work).unwrap();
}

#[test]
fn without_an_agent_above_it_the_hook_records_no_identity_and_still_hands_out_the_tag() {
    let dir = scratch_dir("session-no-agent");
    let work = private_dir("session-no-agent-work");
    let env_file = work.join("env.sh");
    let outcome = run_chain(
        &Chain::within(&work, "chain").relay().orphan(),
        &dir,
        Some(&env_file),
        &start(),
    );
    outcome.assert_silent_success();
    let recorded = records(&dir);
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].agent_identity, None);
    let tag = export_lines(&env_file)[0].clone();
    assert_eq!(
        recorded[0]
            .session_tag_key
            .as_ref()
            .map(|key| key.as_str().to_owned()),
        Some(key_of(&dir, tag_of(&tag)))
    );
    fs::remove_dir_all(&dir).unwrap();
    fs::remove_dir_all(&work).unwrap();
}

#[test]
fn every_journaled_event_carries_the_identity_of_its_agent() {
    let dir = scratch_dir("session-events");
    let work = private_dir("session-events-work");
    let chain = Chain::within(&work, "chain").claude().relay();
    let mut agents = Vec::new();
    for name in ["SessionStart", "PreToolUse", "PostToolUse", "SessionEnd"] {
        let outcome = run_chain(&chain, &dir, None, &event(name));
        outcome.assert_silent_success();
        agents.push(identity_of(&outcome.hops[0]));
    }
    let recorded = records(&dir);
    assert_eq!(recorded.len(), 4);
    let kinds: Vec<Kind> = recorded.iter().map(|record| record.kind).collect();
    assert_eq!(
        kinds,
        [
            Kind::SessionStart,
            Kind::ShellStart,
            Kind::ShellEnd,
            Kind::SessionEnd
        ]
    );
    for (record, agent) in recorded.iter().zip(&agents) {
        assert_eq!(record.agent_identity.as_ref(), Some(agent), "{:?}", record.kind);
    }
    fs::remove_dir_all(&dir).unwrap();
    fs::remove_dir_all(&work).unwrap();
}

#[test]
fn the_path_of_the_agent_never_reaches_the_journal() {
    let dir = scratch_dir("session-no-path");
    let work = private_dir("session-no-path-work");
    let outcome = run_chain(&Chain::within(&work, "chain").claude(), &dir, None, &start());
    outcome.assert_silent_success();
    let path = outcome.hops[0].evidence.exe_path.to_string_lossy().into_owned();
    let stored = fs::read(dir.join("journal.jsonl")).unwrap();
    assert!(!stored.windows(path.len()).any(|window| window == path.as_bytes()));
    assert!(!String::from_utf8_lossy(&stored).contains("hop0"));
    fs::remove_dir_all(&dir).unwrap();
    fs::remove_dir_all(&work).unwrap();
}

fn unusable_env_files(work: &Path) -> Vec<(&'static str, PathBuf)> {
    let target = work.join("target.sh");
    fs::write(&target, "keep\n").unwrap();
    let symlinked = work.join("symlinked.sh");
    symlink(&target, &symlinked).unwrap();
    let dangling = work.join("dangling.sh");
    symlink(work.join("created-behind-the-link.sh"), &dangling).unwrap();
    let hard = work.join("hard.sh");
    fs::write(&hard, "keep\n").unwrap();
    fs::hard_link(&hard, work.join("hard-twin.sh")).unwrap();
    let directory = work.join("directory.sh");
    fs::create_dir(&directory).unwrap();
    let fifo = work.join("fifo.sh");
    make_fifo(&fifo);
    vec![
        ("symlink", symlinked),
        ("dangling symlink", dangling),
        ("hard link", hard),
        ("directory", directory),
        ("fifo", fifo),
        ("missing directory", work.join("absent").join("env.sh")),
        ("relative", PathBuf::from("env-relative.sh")),
    ]
}

#[test]
fn an_env_file_the_hook_cannot_use_costs_the_tag_and_never_the_record() {
    let work = private_dir("session-unusable");
    let cwd_before: Vec<_> = fs::read_dir(".")
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    for (name, env_file) in unusable_env_files(&work) {
        let dir = scratch_dir("session-unusable-data");
        let envs: [(&str, &OsStr); 2] = [
            ("AGENTDUST_DATA_DIR", dir.as_os_str()),
            ("CLAUDE_ENV_FILE", env_file.as_os_str()),
        ];
        let output = run_hook_guarded(&envs, &start()).unwrap_or_else(|| panic!("{name}: the hook blocked"));
        assert_silent(&output);
        let recorded = records(&dir);
        assert_eq!(recorded.len(), 1, "{name}");
        assert!(recorded[0].session_tag_key.is_some(), "{name}");
        fs::remove_dir_all(&dir).unwrap();
    }
    assert_eq!(fs::read_to_string(work.join("target.sh")).unwrap(), "keep\n");
    assert_eq!(fs::read_to_string(work.join("hard.sh")).unwrap(), "keep\n");
    assert_eq!(fs::read_to_string(work.join("hard-twin.sh")).unwrap(), "keep\n");
    assert!(!work.join("created-behind-the-link.sh").exists());
    assert!(!work.join("absent").exists());
    assert!(fs::read_dir(work.join("directory.sh")).unwrap().next().is_none());
    let cwd_after: Vec<_> = fs::read_dir(".")
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(cwd_before, cwd_after);
    fs::remove_dir_all(&work).unwrap();
}

fn plant_secret(dir: &Path, bytes: &[u8], mode: u32) {
    fs::create_dir_all(dir).unwrap();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.join(SECRET_FILE);
    fs::write(&path, bytes).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn an_unusable_secret_means_no_key_and_no_tag_and_the_record_is_still_written() {
    for (name, bytes, mode) in [
        ("short", vec![1u8; 31], 0o600),
        ("long", vec![1u8; 33], 0o600),
        ("loose mode", vec![1u8; 32], 0o644),
    ] {
        let dir = scratch_dir("session-bad-secret");
        let work = private_dir("session-bad-secret-work");
        let env_file = work.join("env.sh");
        plant_secret(&dir, &bytes, mode);
        assert_silent(&run_plain(&dir, Some(&env_file), &start()));
        let recorded = records(&dir);
        assert_eq!(recorded.len(), 1, "{name}");
        assert_eq!(recorded[0].session_tag_key, None, "{name}");
        assert_eq!(recorded[0].cwd_key, None, "{name}");
        assert!(!env_file.exists(), "{name}");
        assert_eq!(secret_bytes(&dir), bytes, "{name}");
        fs::remove_dir_all(&dir).unwrap();
        fs::remove_dir_all(&work).unwrap();
    }
}

#[test]
fn a_journal_that_cannot_be_written_means_no_tag_is_handed_out() {
    let dir = scratch_dir("session-no-journal");
    let work = private_dir("session-no-journal-work");
    let env_file = work.join("env.sh");
    plant_secret(&dir, &[7u8; 32], 0o600);
    let elsewhere = work.join("elsewhere.jsonl");
    File::create(&elsewhere).unwrap();
    symlink(&elsewhere, dir.join("journal.jsonl")).unwrap();
    assert_silent(&run_plain(&dir, Some(&env_file), &start()));
    assert!(!env_file.exists());
    assert_eq!(fs::read(&elsewhere).unwrap(), b"");
    fs::remove_dir_all(&dir).unwrap();
    fs::remove_dir_all(&work).unwrap();
}

#[test]
fn a_tag_in_the_environment_of_the_hook_is_not_the_tag_it_hands_out() {
    let dir = scratch_dir("session-env-tag");
    let work = private_dir("session-env-tag-work");
    let env_file = work.join("env.sh");
    let planted = "0123456789abcdef0123456789abcdef";
    let output = run_hook_with(
        ["hook", "claude"],
        &[
            ("AGENTDUST_DATA_DIR", dir.as_os_str()),
            ("CLAUDE_ENV_FILE", env_file.as_os_str()),
            ("AGENTDUST_SESSION", OsStr::new(planted)),
        ],
        None,
        &start(),
    );
    assert_silent(&output);
    assert_ne!(tag_of(&export_lines(&env_file)[0]), planted);
    fs::remove_dir_all(&dir).unwrap();
    fs::remove_dir_all(&work).unwrap();
}

#[test]
fn the_data_directory_still_holds_the_journal_and_the_secret_only() {
    let dir = scratch_dir("session-files");
    let work = private_dir("session-files-work");
    let env_file = work.join("env.sh");
    assert_silent(&run_plain(&dir, Some(&env_file), &start()));
    let mut names: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["install.secret", "journal.jsonl"]);
    fs::remove_dir_all(&dir).unwrap();
    fs::remove_dir_all(&work).unwrap();
}
