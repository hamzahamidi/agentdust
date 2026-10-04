mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use agentdust_core::journal::{self, Agent, AgentIdentity, Kind, Record, SCHEMA_VERSION};
use agentdust_core::secret::{self, Secret};
use agentdust_core::tag::SessionTag;
use common::sleeper::{Sleeper, run_sleeper_hop, spawn};
use common::{private_dir, scratch_dir};
use serde_json::Value;

#[test]
#[ignore = "runs only as a process that another test started"]
fn sleeper_hop() {
    run_sleeper_hop();
}

struct World {
    work: PathBuf,
    data: PathBuf,
    secret: Secret,
    tag: SessionTag,
}

impl World {
    fn new(name: &str) -> Self {
        let work = private_dir(name);
        let data = work.join("data");
        let secret = secret::load_or_create(&data).unwrap();
        Self {
            work,
            data,
            secret,
            tag: SessionTag::from_bytes(std::array::from_fn(|i| i as u8 + 1)),
        }
    }

    fn agent(&self) -> Sleeper {
        spawn(&self.work.join("agent-dir"), "agent-bin", &self.work, &[], &[])
    }

    fn leftover(&self, tagged: bool, markers: &[&str]) -> Sleeper {
        let envs: Vec<(&str, &str)> = if tagged {
            vec![("AGENTDUST_SESSION", self.tag.as_str())]
        } else {
            Vec::new()
        };
        spawn(
            &self.work.join("alice-sentinel").join("bin"),
            "leftover-bin",
            &self.work.join("repo-sentinel"),
            &envs,
            markers,
        )
    }

    fn record_session(&self, agent: &Sleeper) {
        let record = Record {
            v: SCHEMA_VERSION,
            kind: Kind::SessionStart,
            agent: Agent::Claude,
            session_id: "doctor-e2e".to_owned(),
            subagent_id: None,
            agent_identity: Some(AgentIdentity::from_process(&agent.identity).unwrap()),
            tool_use_id: None,
            wall_ts: 1,
            mono_ts: 1,
            boot: agent.identity.kernel.boot_session_uuid.clone(),
            session_tag_key: Some(self.tag.key(&self.secret)),
            cwd_key: None,
            exe_base: None,
        };
        journal::append(&self.data, &record).unwrap();
    }

    fn doctor(&self, args: &[&str]) -> Output {
        run_doctor(&self.data, args)
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.work);
    }
}

fn run_doctor(data: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agentdust"))
        .arg("doctor")
        .args(args)
        .env("AGENTDUST_DATA_DIR", data)
        .env_remove("AGENTDUST_SESSION")
        .output()
        .unwrap()
}

fn parsed(output: &Output) -> Value {
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "stdout is not JSON ({err}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn finding(report: &Value, pid: i32) -> Option<&Value> {
    report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["pid"] == pid)
}

#[test]
fn a_leftover_of_an_ended_session_is_owned_ended() {
    let world = World::new("doctor-ended");
    let mut agent = world.agent();
    world.record_session(&agent);
    agent.end();
    let leftover = world.leftover(true, &[]);
    let report = parsed(&world.doctor(&["--json"]));
    let found = finding(&report, leftover.pid()).expect("the leftover is listed");
    assert_eq!(found["class"], "owned-ended");
    assert_eq!(
        found["evidence"],
        serde_json::json!(["owned.tag", "owned.agent_gone"])
    );
    assert_eq!(found["exe_base"], "leftover-bin");
    assert_eq!(found["cwd_relation"], "temp");
    assert_eq!(report["version"], 1);
    assert_eq!(report["owned_classes"]["available"], true);
    assert_eq!(report["owned_classes"]["reason"], Value::Null);
    assert_eq!(report["launchd"]["available"], true);
    assert_eq!(
        report["sessions"],
        serde_json::json!({"records":1,"skipped_lines":0,"active":0,"ended":1,"unknown":0,"degraded":0})
    );
    assert!(report["counts"]["owned-ended"].as_u64().unwrap() >= 1);
    assert!(leftover.alive());
}

#[test]
fn a_leftover_of_a_live_session_is_owned_live_and_its_agent_is_not_listed() {
    let world = World::new("doctor-live");
    let agent = world.agent();
    world.record_session(&agent);
    let leftover = world.leftover(true, &[]);
    let report = parsed(&world.doctor(&["--json"]));
    assert_eq!(finding(&report, leftover.pid()).unwrap()["class"], "owned-live");
    assert!(finding(&report, agent.pid()).is_none());
    assert_eq!(report["sessions"]["active"], 1);
    assert!(leftover.alive() && agent.alive());
}

#[test]
fn a_tag_without_a_session_is_not_listed_and_is_counted() {
    let world = World::new("doctor-unmatched");
    let leftover = world.leftover(true, &[]);
    let report = parsed(&world.doctor(&["--json"]));
    assert!(finding(&report, leftover.pid()).is_none());
    assert!(report["unexplained_tags"].as_u64().unwrap() >= 1);
    assert_eq!(report["owned_classes"]["available"], true);
}

#[test]
fn an_untagged_process_with_a_live_parent_is_not_listed() {
    let world = World::new("doctor-untagged");
    let leftover = world.leftover(false, &[]);
    let report = parsed(&world.doctor(&["--json"]));
    assert!(finding(&report, leftover.pid()).is_none());
}

#[test]
fn a_journal_of_a_newer_version_makes_owned_classes_unavailable() {
    let world = World::new("doctor-version");
    let mut agent = world.agent();
    world.record_session(&agent);
    agent.end();
    fs::write(
        world.data.join("journal.jsonl"),
        b"\x1e{\"v\":2,\"kind\":\"session_start\"}\n",
    )
    .unwrap();
    let leftover = world.leftover(true, &[]);
    let report = parsed(&world.doctor(&["--json"]));
    assert_eq!(report["owned_classes"]["available"], false);
    assert_eq!(report["owned_classes"]["reason"], "unsupported_version");
    assert!(finding(&report, leftover.pid()).is_none());
    let text = String::from_utf8(world.doctor(&[]).stdout).unwrap();
    assert!(
        text.contains("journal: owned classes are unavailable: the journal has a schema version this build does not support"),
        "{text}"
    );
}

#[test]
fn without_a_data_directory_nothing_is_created_and_owned_classes_are_unavailable() {
    let work = scratch_dir("doctor-no-data");
    let data = work.join("absent");
    let report = parsed(&run_doctor(&data, &["--json"]));
    assert_eq!(report["owned_classes"]["available"], false);
    assert_eq!(report["owned_classes"]["reason"], "secret_unavailable");
    assert!(!data.exists());
    assert!(!work.exists());
}

#[test]
fn doctor_changes_nothing_in_the_data_directory() {
    let world = World::new("doctor-readonly");
    let mut agent = world.agent();
    world.record_session(&agent);
    agent.end();
    let before = snapshot(&world.data);
    parsed(&world.doctor(&["--json"]));
    world.doctor(&[]);
    assert_eq!(snapshot(&world.data), before);
}

fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>, u32)> {
    use std::os::unix::fs::PermissionsExt;
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let mode = entry.metadata().unwrap().permissions().mode();
            (
                entry.file_name().into_string().unwrap(),
                fs::read(entry.path()).unwrap(),
                mode,
            )
        })
        .collect();
    entries.sort();
    entries
}

#[test]
fn the_json_never_holds_the_command_the_directory_or_a_path() {
    let world = World::new("doctor-privacy");
    let mut agent = world.agent();
    world.record_session(&agent);
    agent.end();
    let leftover = world.leftover(
        true,
        &["command-sentinel-7", "SENTINEL_API_KEY=secret-sentinel-7"],
    );
    let output = world.doctor(&["--json"]);
    let json = String::from_utf8(output.stdout.clone()).unwrap();
    let report = parsed(&output);
    assert!(finding(&report, leftover.pid()).is_some());
    for forbidden in [
        "command-sentinel",
        "secret-sentinel",
        "repo-sentinel",
        "alice-sentinel",
        "SENTINEL_API_KEY",
        world.work.to_str().unwrap(),
    ] {
        assert!(!json.contains(forbidden), "{forbidden} in the JSON");
    }
}

struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_terminal_text_shows_the_redacted_command_and_the_directory_of_each_finding() {
    let world = World::new("doctor-text");
    let mut agent = world.agent();
    world.record_session(&agent);
    agent.end();
    let short = Cleanup(PathBuf::from("/tmp").join(format!("adt-{}", std::process::id())));
    let tag = world.tag.as_str().to_owned();
    let leftover = spawn(
        &short.0,
        "leftover-bin",
        &world.work.join("repo-sentinel"),
        &[("AGENTDUST_SESSION", &tag)],
        &["command-sentinel-8", "SENTINEL_API_KEY=secret-sentinel-8"],
    );
    let output = world.doctor(&[]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with("agentdust doctor\n"), "{text}");
    let block: Vec<&str> = text
        .lines()
        .skip_while(|line| !line.contains(&format!("pid {}", leftover.pid())))
        .take(4)
        .collect();
    assert_eq!(block.len(), 4, "{text}");
    assert!(block[0].contains("owned-ended"), "{block:?}");
    assert!(block[0].contains("leftover-bin"));
    assert!(
        block[2].contains("command-sentinel-8 SENTINEL_API_KEY=***"),
        "{block:?}"
    );
    assert!(block[3].contains("repo-sentinel"), "{block:?}");
    assert!(!text.contains("secret-sentinel-8"));
}

#[test]
fn unknown_arguments_print_the_usage_and_exit_2() {
    let work = private_dir("doctor-usage");
    for args in [
        &["--nope"][..],
        &["--json", "--json"],
        &["--json", "extra"],
        &["extra"],
    ] {
        let output = run_doctor(&work, args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert_eq!(output.stdout, b"", "{args:?}");
        let usage = String::from_utf8_lossy(&output.stderr);
        assert!(
            usage.contains("usage:") && usage.contains("agentdust doctor [--json]"),
            "{args:?}"
        );
    }
    fs::remove_dir_all(&work).unwrap();
}
