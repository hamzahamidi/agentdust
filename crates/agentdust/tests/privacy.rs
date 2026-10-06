mod common;
mod privacy_support;

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::panic::catch_unwind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use agentdust_core::clock::wall_ms;
use agentdust_core::darwin::boot_session_uuid;
use agentdust_core::identity::ProcessIdentity;
use agentdust_core::journal::retention::Policy;
use agentdust_core::journal::{self, Agent, Kind, Record, SCHEMA_VERSION};
use agentdust_core::secret::SECRET_FILE;
use common::chain::{Chain, run_hop};
use common::private_dir;
use privacy_support::{
    Scan, Sentinels, TempWatch, contains, is_journal_file, journal_bytes, scan_data_dir, system_temp_roots,
};

const EARLIER_BOOT: &str = "an-earlier-boot";
const WRITTEN_BY_THE_HOOK: [&str; 12] = [
    "v",
    "kind",
    "agent",
    "session_id",
    "subagent_id",
    "agent_identity",
    "tool_use_id",
    "wall_ts",
    "mono_ts",
    "boot",
    "session_tag_key",
    "cwd_key",
];
const IDENTITY_KEYS_WRITTEN: [&str; 4] = ["pid", "start_time_us", "uid", "exe_base"];

#[test]
#[ignore = "runs only as a hop of a process chain that another test started"]
fn relay_hop() {
    run_hop();
}

fn run_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!("{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))
}

fn big_output(s: &Sentinels) -> String {
    let filler = "x".repeat(2_500_000);
    format!(
        "{} {filler} {} {filler} {}",
        s.output_start, s.output_middle, s.output_end
    )
}

fn event(s: &Sentinels, name: &str, extra: &str) -> Vec<u8> {
    format!(
        r#"{{"session_id":"{}","transcript_path":"/Users/{}/.claude/projects/{}/{}.jsonl","cwd":"{}","hook_event_name":"{name}","{}":"{}"{extra}}}"#,
        s.session_id, s.user, s.repo, s.transcript, s.cwd, s.unknown_name, s.unknown_value
    )
    .into_bytes()
}

fn tool_use(s: &Sentinels, name: &str, extra: &str) -> Vec<u8> {
    event(
        s,
        name,
        &format!(
            r#","tool_name":"Bash","tool_use_id":"toolu_privacy_1","agent_id":"{}"{extra}"#,
            s.subagent_id
        ),
    )
}

fn subagent(s: &Sentinels, name: &str) -> Vec<u8> {
    event(
        s,
        name,
        &format!(
            r#","agent_id":"{}","agent_type":"{}","agent_transcript_path":"/Users/{}/subagents/{}.jsonl","last_assistant_message":"{}","stop_hook_active":true"#,
            s.subagent_id, s.agent_type, s.user, s.transcript, s.command
        ),
    )
}

fn payloads(s: &Sentinels) -> Vec<(&'static str, Vec<u8>)> {
    let command = format!(
        r#","tool_input":{{"command":"curl -H 'Authorization: {}' https://example.test/{}"}}"#,
        s.command, s.repo
    );
    let response = format!(
        r#","tool_response":{{"stdout":"{}","stderr":"","interrupted":false}}"#,
        big_output(s)
    );
    vec![
        ("SessionStart", event(s, "SessionStart", r#","source":"startup""#)),
        ("PreToolUse", tool_use(s, "PreToolUse", &command)),
        (
            "PostToolUse",
            tool_use(s, "PostToolUse", &(command.clone() + &response)),
        ),
        ("SessionEnd", event(s, "SessionEnd", r#","reason":"other""#)),
        ("SubagentStart", subagent(s, "SubagentStart")),
        ("SubagentStop", subagent(s, "SubagentStop")),
    ]
}

fn run_all(
    s: &Sentinels,
    chain: &Chain,
    data_dir: &Path,
    env_file: &Path,
    cwd: &Path,
    extra_env: &[(&str, &OsStr)],
) -> Vec<ProcessIdentity> {
    let mut agents = Vec::new();
    for (name, payload) in payloads(s) {
        let mut envs: Vec<(&str, &OsStr)> = vec![
            ("AGENTDUST_SESSION", OsStr::new(&s.tag)),
            ("PRIVACY_TEST_ENV", OsStr::new(&s.env_value)),
        ];
        envs.extend_from_slice(extra_env);
        let outcome = chain.run(data_dir, &payload, Some(env_file), &envs, Some(cwd));
        assert_eq!(outcome.status, Some(0), "{name}");
        assert_eq!(outcome.stdout, "", "{name}");
        assert_eq!(outcome.stderr, "", "{name}");
        agents.push(outcome.hops[0].clone());
    }
    agents
}

fn generated_tag(env_file: &Path) -> String {
    let text = fs::read_to_string(env_file).unwrap();
    let lines: Vec<&str> = text.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(lines.len(), 1, "{lines:?}");
    let tag = lines[0].strip_prefix("export AGENTDUST_SESSION=").unwrap();
    assert_eq!(tag.len(), 32);
    tag.to_owned()
}

fn assert_stage(dir: &Path, stage: &str, records: usize, s: &Sentinels) -> Scan {
    let found = scan_data_dir(dir, stage, s);
    let written: BTreeSet<String> = WRITTEN_BY_THE_HOOK.iter().map(|key| (*key).to_owned()).collect();
    assert!(
        written.is_subset(&found.keys),
        "{stage}: the hook wrote no {:?}",
        written.difference(&found.keys).collect::<Vec<_>>()
    );
    assert!(
        contains(&journal_bytes(dir), &s.session_id),
        "{stage}: the scan cannot see the allowed session id"
    );
    assert_eq!(
        journal::read(dir).unwrap().records.len(),
        records,
        "{stage}: records"
    );
    found
}

fn earlier_boot_marker() -> Record {
    Record {
        v: SCHEMA_VERSION,
        kind: Kind::SessionStart,
        agent: Agent::Claude,
        session_id: "marker-session".to_owned(),
        subagent_id: None,
        tool_use_id: None,
        wall_ts: 1,
        mono_ts: 1,
        boot: EARLIER_BOOT.to_owned(),
        cwd_key: None,
        agent_identity: None,
        session_tag_key: None,
        exe_base: None,
    }
}

#[test]
fn no_command_output_path_environment_or_unknown_field_reaches_the_data_or_the_temp_directory() {
    let mut s = Sentinels::new(&run_id());
    let dir = private_dir("privacy");
    let process_dir = private_dir("privacy-process").join(&s.process_dir);
    fs::create_dir(&process_dir).unwrap();
    let process_root = process_dir.parent().unwrap().to_path_buf();
    let env_file = process_root.join("claude-env.sh");
    let chain = Chain::within(&process_root, "chain").claude();

    let watch = TempWatch::start(&[&dir, &process_root]);
    let agents = run_all(&s, &chain, &dir, &env_file, &process_dir, &[]);
    s.also_never_stored(generated_tag(&env_file));
    s.also_never_stored(agents[0].evidence.exe_path.to_string_lossy().into_owned());

    let after_hooks = assert_stage(&dir, "after the hooks", 6, &s);
    assert_eq!(
        after_hooks.names,
        BTreeSet::from(["journal.jsonl".to_owned(), SECRET_FILE.to_owned()])
    );
    assert_eq!(
        after_hooks.identity_keys,
        IDENTITY_KEYS_WRITTEN
            .iter()
            .map(|key| (*key).to_owned())
            .collect()
    );
    assert!(
        fs::read_dir(&process_dir).unwrap().next().is_none(),
        "the hook created a file in its working directory"
    );
    let leaks = watch.leaks(&s.nowhere_outside_the_journal());
    assert!(leaks.is_empty(), "files in the system temp directory: {leaks:#?}");

    journal::append(&dir, &earlier_boot_marker()).unwrap();
    let boot = boot_session_uuid().unwrap();
    journal::rotate(&dir, wall_ms()).unwrap();
    let rotated = assert_stage(&dir, "after a rotation", 7, &s);
    assert!(rotated.names.contains("journal.maint"));
    assert!(!rotated.names.contains("journal.jsonl"));
    assert_eq!(rotated.journal_files, 1);

    let report = journal::retain(&dir, &Policy::default(), wall_ms(), &boot).unwrap();
    assert_eq!(
        (report.rewritten, report.deleted, report.dropped_earlier_boot),
        (1, 0, 1)
    );
    let compacted = assert_stage(&dir, "after a compaction", 6, &s);
    assert!(compacted.names.contains("journal.maint"));
    assert_eq!(compacted.journal_files, 1);

    fs::remove_dir_all(&dir).unwrap();
    fs::remove_dir_all(&process_root).unwrap();
}

fn entries(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect()
}

#[test]
fn a_hook_given_an_empty_temp_directory_home_and_working_directory_leaves_all_three_empty() {
    let s = Sentinels::new(&format!("{}-empty", run_id()));
    let data = private_dir("privacy-empty-data");
    let tmp = private_dir("privacy-empty-tmp");
    let home = private_dir("privacy-empty-home");
    let cwd = private_dir("privacy-empty-cwd");
    let elsewhere = private_dir("privacy-empty-elsewhere");
    let chain = Chain::within(&elsewhere, "chain").claude();

    run_all(
        &s,
        &chain,
        &data,
        &elsewhere.join("env.sh"),
        &cwd,
        &[("TMPDIR", tmp.as_os_str()), ("HOME", home.as_os_str())],
    );

    assert_eq!(journal::read(&data).unwrap().records.len(), 6);
    for (what, dir) in [("TMPDIR", &tmp), ("HOME", &home), ("working directory", &cwd)] {
        assert_eq!(
            entries(dir),
            Vec::<String>::new(),
            "the hook wrote into its {what}"
        );
    }
    for dir in [data, tmp, home, cwd, elsewhere] {
        fs::remove_dir_all(dir).unwrap();
    }
}

fn scan_refuses(name: &str, content: &str, s: &Sentinels) -> bool {
    let dir = private_dir("privacy-planted");
    let path = dir.join(name);
    fs::write(&path, content).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let refused = catch_unwind(|| scan_data_dir(&dir, "planted", s)).is_err();
    fs::remove_dir_all(&dir).unwrap();
    refused
}

fn frame_with(session_id: &str) -> String {
    format!(
        "\u{1e}{{\"v\":1,\"kind\":\"session_start\",\"agent\":\"claude\",\"session_id\":\"{session_id}\",\"wall_ts\":1,\"mono_ts\":2,\"boot\":\"b\"}}\n"
    )
}

const FILE_KINDS: [&str; 4] = [
    SECRET_FILE,
    "journal.maint",
    "journal.jsonl",
    "journal.1700000000000.jsonl",
];

#[test]
fn the_scan_finds_a_forbidden_value_in_every_kind_of_file() {
    let s = Sentinels::new(&format!("{}-plant", run_id()));
    for name in FILE_KINDS {
        let content = if is_journal_file(name) {
            frame_with(&s.command)
        } else {
            format!("noise {} noise", s.command)
        };
        assert!(
            scan_refuses(name, &content, &s),
            "{} in {name} went unnoticed",
            s.command
        );
    }
}

#[test]
fn the_session_id_is_allowed_in_journal_files_and_nowhere_else() {
    let s = Sentinels::new(&format!("{}-plant", run_id()));
    for name in FILE_KINDS {
        let journal_file = is_journal_file(name);
        let content = if journal_file {
            frame_with(&s.session_id)
        } else {
            format!("noise {} noise", s.session_id)
        };
        assert_eq!(scan_refuses(name, &content, &s), !journal_file, "{name}");
    }
}

#[test]
fn the_scan_refuses_a_journal_key_that_is_not_on_the_list() {
    let s = Sentinels::new(&format!("{}-plant", run_id()));
    let line = frame_with("s").replace("\"boot\":\"b\"", "\"boot\":\"b\",\"tool_name\":\"Bash\"");
    assert!(scan_refuses("journal.jsonl", &line, &s));
    assert!(!scan_refuses("journal.jsonl", &frame_with("s"), &s));
}

#[test]
fn the_scan_refuses_a_journal_segment_that_is_not_a_record() {
    let s = Sentinels::new(&format!("{}-plant", run_id()));
    for content in [
        "not json\n",
        "\u{1e}{\"v\":1,\"kind\":\"session_start\"",
        "\u{1e}{\"v\":3,\"kind\":\"future\"}\n",
    ] {
        assert!(scan_refuses("journal.jsonl", content, &s), "{content:?}");
    }
}

#[test]
fn the_scan_refuses_a_file_it_does_not_know() {
    let s = Sentinels::new(&format!("{}-plant", run_id()));
    for name in [
        "notes.txt",
        "journal.lock",
        "journal.maint.bak",
        "journal.compact.tmp",
        "journal.jsonl.corrupt-1",
        "install.secret.0a1b2c3d.tmp",
    ] {
        assert!(scan_refuses(name, "harmless", &s), "{name}");
    }
}

fn plant(dir: &Path, relative: &str, content: &str) -> PathBuf {
    let path = dir.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, content).unwrap();
    path
}

fn hours_from_now(hours: i64) -> (i64, i64) {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    (now.as_secs() as i64 + hours * 3600, 0)
}

fn planted_needle(name: &str) -> String {
    format!("planted-needle-{name}-{}", run_id())
}

#[test]
fn the_temp_scan_finds_a_value_in_the_bytes_or_the_name_of_a_file_made_after_the_start() {
    let dir = private_dir("privacy-temp-bytes");
    let (in_bytes, in_name) = (planted_needle("bytes"), planted_needle("name"));
    plant(&dir, "one/body.txt", &format!("noise {in_bytes} noise"));
    plant(&dir, &format!("one/{in_name}.txt"), "harmless");
    let watch = TempWatch::since(hours_from_now(-1), vec![dir.clone()], &[]);
    let leaks = watch.leaks(&[&in_bytes, &in_name]);
    assert!(
        leaks
            .iter()
            .any(|leak| leak.starts_with(&format!("{in_bytes} in the bytes of")))
    );
    assert!(
        leaks
            .iter()
            .any(|leak| leak.starts_with(&format!("{in_name} in the name of")))
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_temp_scan_ignores_a_file_that_existed_before_the_start() {
    let dir = private_dir("privacy-temp-before");
    let needle = planted_needle("before");
    plant(&dir, "old.txt", &needle);
    let after_it = TempWatch::since(hours_from_now(1), vec![dir.clone()], &[]);
    assert_eq!(after_it.leaks(&[&needle]), Vec::<String>::new());
    let before_it = TempWatch::since(hours_from_now(-1), vec![dir.clone()], &[]);
    assert_eq!(before_it.leaks(&[&needle]).len(), 1);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_temp_scan_skips_the_directories_it_is_told_to_exclude() {
    let dir = private_dir("privacy-temp-excluded");
    let needle = planted_needle("excluded");
    plant(&dir, "own/body.txt", &needle);
    plant(&dir, "other/body.txt", &needle);
    let own = dir.join("own");
    let watch = TempWatch::since(hours_from_now(-1), vec![dir.clone()], &[&own]);
    let leaks = watch.leaks(&[&needle]);
    assert_eq!(leaks.len(), 1, "{leaks:?}");
    assert!(leaks[0].contains("other"));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_temp_scan_reaches_three_levels_and_no_deeper() {
    let dir = private_dir("privacy-temp-depth");
    let needle = planted_needle("depth");
    plant(&dir, "a/b/third.txt", &needle);
    plant(&dir, "a/b/c/fourth.txt", &needle);
    let watch = TempWatch::since(hours_from_now(-1), vec![dir.clone()], &[]);
    let leaks = watch.leaks(&[&needle]);
    assert_eq!(leaks.len(), 1, "{leaks:?}");
    assert!(leaks[0].contains("third.txt"));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_temp_scan_does_not_follow_a_symlink_or_read_a_fifo() {
    let dir = private_dir("privacy-temp-links");
    let outside = private_dir("privacy-temp-links-outside");
    let needle = planted_needle("links");
    plant(&outside, "body.txt", &needle);
    std::os::unix::fs::symlink(&outside, dir.join("link")).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(dir.join("pipe"))
            .status()
            .unwrap()
            .success()
    );
    let watch = TempWatch::since(hours_from_now(-1), vec![dir.clone()], &[]);
    assert_eq!(watch.leaks(&[&needle]), Vec::<String>::new());
    let touched: Vec<_> = watch.touched();
    assert!(
        touched
            .iter()
            .any(|(path, regular)| path.ends_with("pipe") && !regular)
    );
    fs::remove_dir_all(&dir).unwrap();
    fs::remove_dir_all(&outside).unwrap();
}

#[test]
fn the_temp_scan_covers_the_directory_a_hook_would_use() {
    let roots = system_temp_roots();
    assert!(roots.contains(&std::env::temp_dir()));
    assert!(roots.iter().all(|root| root.is_dir()));
}

#[test]
fn the_scan_refuses_an_identity_key_that_is_not_on_the_list() {
    let s = Sentinels::new(&format!("{}-plant", run_id()));
    let identity = r#""agent_identity":{"pid":1,"start_time_us":2,"uid":3"#;
    let listed = frame_with("s").replace(
        "\"boot\":\"b\"",
        &format!("\"boot\":\"b\",{identity},\"exe_base\":\"claude\"}}"),
    );
    assert!(!scan_refuses("journal.jsonl", &listed, &s), "{listed}");
    let unlisted = frame_with("s").replace(
        "\"boot\":\"b\"",
        &format!("\"boot\":\"b\",{identity},\"exe_path\":\"/Users/a/claude\"}}"),
    );
    assert!(scan_refuses("journal.jsonl", &unlisted, &s), "{unlisted}");
}

#[test]
fn the_scan_finds_a_generated_value_in_a_data_file() {
    let mut s = Sentinels::new(&format!("{}-plant", run_id()));
    s.also_never_stored("a-generated-tag-0123456789abcdef");
    assert!(scan_refuses(
        "install.secret",
        "noise a-generated-tag-0123456789abcdef noise",
        &s
    ));
    assert!(scan_refuses(
        "journal.jsonl",
        &frame_with("a-generated-tag-0123456789abcdef"),
        &s
    ));
}
