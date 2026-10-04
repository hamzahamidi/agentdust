mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Output;

use agentdust_core::cwd::cwd_key;
use agentdust_core::digest::{Domain, keyed_digest};
use agentdust_core::journal::{self, Kind};
use agentdust_core::secret::{SECRET_FILE, Secret, load_or_create};
use common::{
    FIXTURE_CWD, HANG_GUARD, make_fifo, pre_tool_use, pre_tool_use_with, private_dir, run_hook,
    run_hook_with, run_hook_within, run_hooks_together, scratch_dir,
};

const SENTINEL_DIR: &str = "sentinel-cwd-7f3a91";
const SENTINEL_NAME: &str = "secret-project-b2d4";
const SENTINEL_COMMAND: &str = "sentinel-command-41c7e0";
const SENTINEL_OUTPUT: &str = "sentinel-output-9e25b3";
const SENTINEL_TAG: &str = "sentinel-session-tag-60d1f8";

fn stored_secret(dir: &Path) -> Secret {
    let bytes: [u8; 32] = fs::read(dir.join(SECRET_FILE)).unwrap().try_into().unwrap();
    Secret::from_bytes(bytes)
}

fn keys(dir: &Path) -> Vec<Option<String>> {
    journal::read(dir)
        .unwrap()
        .records
        .iter()
        .map(|record| record.cwd_key.as_ref().map(|key| key.as_str().to_owned()))
        .collect()
}

fn cwd_field(cwd_json: &str) -> String {
    format!(r#","cwd":{cwd_json}"#)
}

fn run_in(dir: &Path, session: &str, cwd: &str) -> Output {
    run_hook(
        dir,
        &pre_tool_use_with(session, "toolu_1", &cwd_field(&format!("\"{cwd}\""))),
    )
}

fn assert_silent_success(output: &Output) {
    assert!(output.status.success(), "{:?}", output.status);
    assert!(output.stdout.is_empty());
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn names(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect()
}

fn journal_and_secret() -> BTreeSet<String> {
    BTreeSet::from(["journal.jsonl".to_owned(), SECRET_FILE.to_owned()])
}

fn all_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(all_files(&path));
        } else {
            files.push(path);
        }
    }
    files
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|window| window == needle)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn a_hook_with_a_cwd_records_the_keyed_digest_of_it() {
    let dir = scratch_dir("cwd-key");
    assert_silent_success(&run_hook(&dir, &pre_tool_use("s1", "toolu_1")));
    let secret = stored_secret(&dir);
    let expected = keyed_digest(secret.as_bytes(), Domain::Cwd, FIXTURE_CWD.as_bytes());
    assert_eq!(keys(&dir), [Some(expected)]);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_planted_secret_gives_the_key_computed_outside_this_code() {
    let dir = private_dir("cwd-known-answer");
    write_secret(&dir, &std::array::from_fn::<u8, 32, _>(|i| i as u8), 0o600);
    assert_silent_success(&run_hook(&dir, &pre_tool_use("s1", "toolu_1")));
    assert_eq!(
        keys(&dir),
        [Some(
            "3657eb6997470b89d15500eb6c2095e0304efccae14ff2381d92f00155fa9fb7".to_owned()
        )]
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_hook_creates_the_secret_of_32_bytes_and_mode_0600_in_a_directory_of_mode_0700() {
    let dir = scratch_dir("cwd-secret-mode");
    assert_silent_success(&run_hook(&dir, &pre_tool_use("s1", "toolu_1")));
    let path = dir.join(SECRET_FILE);
    assert_eq!(fs::metadata(&path).unwrap().len(), 32);
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o7777, 0o600);
    assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o7777, 0o700);
    assert_eq!(names(&dir), journal_and_secret());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn later_hooks_reuse_the_secret_and_give_the_same_directory_the_same_key() {
    let dir = scratch_dir("cwd-reuse");
    assert_silent_success(&run_in(&dir, "s1", FIXTURE_CWD));
    let path = dir.join(SECRET_FILE);
    let before = (fs::read(&path).unwrap(), fs::metadata(&path).unwrap().ino());
    assert_silent_success(&run_in(&dir, "s2", FIXTURE_CWD));
    assert_silent_success(&run_in(&dir, "s3", FIXTURE_CWD));
    assert_eq!(
        before,
        (fs::read(&path).unwrap(), fs::metadata(&path).unwrap().ino())
    );
    let recorded = keys(&dir);
    assert_eq!(recorded.len(), 3);
    assert!(recorded[0].is_some());
    assert!(recorded.iter().all(|key| *key == recorded[0]));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn different_directories_get_different_keys() {
    let dir = scratch_dir("cwd-different");
    assert_silent_success(&run_in(&dir, "s1", "/agentdust-fixture/a"));
    assert_silent_success(&run_in(&dir, "s2", "/agentdust-fixture/b"));
    let recorded = keys(&dir);
    assert!(recorded.iter().all(Option::is_some));
    assert_ne!(recorded[0], recorded[1]);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn every_spelling_of_one_directory_gets_the_same_key() {
    let dir = scratch_dir("cwd-spellings");
    for (i, cwd) in [
        "/agentdust-fixture/a",
        "/agentdust-fixture/a/",
        "/agentdust-fixture//a",
        "/agentdust-fixture/./a",
        "/agentdust-fixture/b/../a",
    ]
    .iter()
    .enumerate()
    {
        assert_silent_success(&run_in(&dir, &format!("s{i}"), cwd));
    }
    let recorded = keys(&dir);
    assert_eq!(recorded.len(), 5);
    assert!(recorded[0].is_some());
    assert!(recorded.iter().all(|key| *key == recorded[0]), "{recorded:?}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_symlinked_working_directory_gets_the_key_of_its_target() {
    let root = private_dir("cwd-symlinked");
    let real = root.join("real");
    fs::create_dir(&real).unwrap();
    symlink(&real, root.join("link")).unwrap();
    let data = root.join("data");
    assert_silent_success(&run_in(&data, "s1", root.join("link").to_str().unwrap()));
    assert_silent_success(&run_in(&data, "s2", real.to_str().unwrap()));
    let recorded = keys(&data);
    assert!(recorded[0].is_some());
    assert_eq!(recorded[0], recorded[1]);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn an_event_without_a_cwd_records_no_key() {
    let dir = scratch_dir("cwd-absent");
    assert_silent_success(&run_hook(&dir, &pre_tool_use_with("s1", "toolu_1", "")));
    assert_eq!(keys(&dir), [None]);
    assert!(
        !fs::read_to_string(dir.join("journal.jsonl"))
            .unwrap()
            .contains("cwd_key")
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_cwd_that_is_not_an_absolute_path_records_no_key() {
    for cwd_json in [
        "null",
        r#""""#,
        r#""relative/dir""#,
        r#""./x""#,
        r#""\u0000""#,
        r#""/a\u0000b""#,
    ] {
        let dir = scratch_dir("cwd-unusable");
        let output = run_hook(&dir, &pre_tool_use_with("s1", "toolu_1", &cwd_field(cwd_json)));
        assert_silent_success(&output);
        let report = journal::read(&dir).unwrap();
        assert_eq!(report.records.len(), 1, "{cwd_json}");
        assert_eq!(report.records[0].kind, Kind::ShellStart, "{cwd_json}");
        assert_eq!(report.records[0].cwd_key, None, "{cwd_json}");
        assert_eq!(
            names(&dir),
            BTreeSet::from(["journal.jsonl".to_owned()]),
            "{cwd_json}"
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn every_journaled_event_kind_carries_the_key() {
    let dir = scratch_dir("cwd-kinds");
    for event in ["SessionStart", "PreToolUse", "PostToolUse", "SessionEnd"] {
        let input = format!(
            r#"{{"session_id":"s1","hook_event_name":"{event}","tool_name":"Bash","tool_use_id":"toolu_1","cwd":"{FIXTURE_CWD}"}}"#
        );
        assert_silent_success(&run_hook(&dir, input.as_bytes()));
    }
    let recorded = keys(&dir);
    assert_eq!(recorded.len(), 4);
    assert!(recorded[0].is_some());
    assert!(recorded.iter().all(|key| *key == recorded[0]));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn events_that_are_not_journaled_create_neither_a_secret_nor_a_directory() {
    let dir = scratch_dir("cwd-not-journaled");
    let stop = format!(r#"{{"session_id":"s1","hook_event_name":"Stop","cwd":"{FIXTURE_CWD}"}}"#);
    let read = format!(
        r#"{{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"Read","cwd":"{FIXTURE_CWD}"}}"#
    );
    assert_silent_success(&run_hook(&dir, stop.as_bytes()));
    assert_silent_success(&run_hook(&dir, read.as_bytes()));
    assert!(!dir.exists());
}

#[test]
fn a_cwd_of_exactly_4096_bytes_is_keyed() {
    let dir = scratch_dir("cwd-limit");
    let cwd = format!("/{}", "d".repeat(4095));
    assert_silent_success(&run_in(&dir, "s1", &cwd));
    let secret = stored_secret(&dir);
    let expected = cwd_key(&secret, &cwd).unwrap();
    assert_eq!(keys(&dir), [Some(expected.as_str().to_owned())]);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_oversized_cwd_records_nothing_and_still_lets_the_host_finish_writing() {
    let dir = scratch_dir("cwd-oversized");
    let cwd = format!("/{}", "d".repeat(4096));
    let trailing = "x".repeat(1_000_000);
    let input = format!(
        r#"{{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"Bash","cwd":"{cwd}","tool_response":"{trailing}"}}"#
    );
    assert_silent_success(&run_hook(&dir, input.as_bytes()));
    assert!(!dir.exists());
}

type Setup = (&'static str, fn(&Path));

#[derive(Debug, PartialEq)]
enum Entry {
    File { mode: u32, bytes: Vec<u8> },
    Link(PathBuf),
    Dir,
    Other,
}

fn snapshot(dir: &Path) -> BTreeMap<String, Entry> {
    let mut entries = BTreeMap::new();
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap();
        if name == "journal.jsonl" {
            continue;
        }
        let kind = entry.file_type().unwrap();
        let value = if kind.is_symlink() {
            Entry::Link(fs::read_link(entry.path()).unwrap())
        } else if kind.is_file() {
            Entry::File {
                mode: entry.metadata().unwrap().permissions().mode() & 0o7777,
                bytes: fs::read(entry.path()).unwrap(),
            }
        } else if kind.is_dir() {
            Entry::Dir
        } else {
            assert!(kind.is_fifo());
            Entry::Other
        };
        entries.insert(name, value);
    }
    entries
}

fn write_secret(dir: &Path, bytes: &[u8], mode: u32) {
    let path = dir.join(SECRET_FILE);
    fs::write(&path, bytes).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
}

fn elsewhere(dir: &Path) -> PathBuf {
    let target = dir.join("elsewhere");
    fs::write(&target, [3u8; 32]).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    target
}

#[test]
fn an_unusable_secret_costs_the_key_and_never_the_record_or_the_secret() {
    let setups: Vec<Setup> = vec![
        ("empty", |dir| write_secret(dir, &[], 0o600)),
        ("31 bytes", |dir| write_secret(dir, &[1; 31], 0o600)),
        ("33 bytes", |dir| write_secret(dir, &[1; 33], 0o600)),
        ("mode 0644", |dir| write_secret(dir, &[1; 32], 0o644)),
        ("mode 0700", |dir| write_secret(dir, &[1; 32], 0o700)),
        ("symlink", |dir| {
            let target = elsewhere(dir);
            symlink(target, dir.join(SECRET_FILE)).unwrap();
        }),
        ("dangling symlink", |dir| {
            symlink(dir.join("not-yet"), dir.join(SECRET_FILE)).unwrap();
        }),
        ("hard link", |dir| {
            write_secret(dir, &[1; 32], 0o600);
            fs::hard_link(dir.join(SECRET_FILE), dir.join("copy")).unwrap();
        }),
        ("fifo", |dir| make_fifo(&dir.join(SECRET_FILE))),
        ("directory", |dir| fs::create_dir(dir.join(SECRET_FILE)).unwrap()),
    ];
    for (name, prepare) in setups {
        let dir = private_dir(&format!("cwd-bad-secret-{}", name.replace(' ', "-")));
        prepare(&dir);
        let before = snapshot(&dir);
        let output = run_hook_within(&dir, &pre_tool_use("s1", "toolu_1"), HANG_GUARD)
            .unwrap_or_else(|| panic!("{name}: the hook blocked"));
        assert_silent_success(&output);
        let report = journal::read(&dir).unwrap();
        assert_eq!(report.records.len(), 1, "{name}");
        assert_eq!(report.records[0].cwd_key, None, "{name}");
        assert_eq!(snapshot(&dir), before, "{name}");
        fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn no_file_in_the_data_directory_holds_the_working_directory() {
    let dir = scratch_dir("cwd-privacy");
    let cwd = format!("/Users/{SENTINEL_DIR}/{SENTINEL_NAME}");
    for event in ["SessionStart", "PreToolUse", "PostToolUse", "SessionEnd"] {
        let input = format!(
            r#"{{"session_id":"s1","hook_event_name":"{event}","tool_name":"Bash","tool_use_id":"toolu_1","cwd":"{cwd}","tool_input":{{"command":"cd {cwd} && make {SENTINEL_COMMAND}"}},"tool_response":{{"stdout":"{SENTINEL_OUTPUT}"}}}}"#
        );
        let output = run_hook_with(
            ["hook", "claude"],
            &[
                ("AGENTDUST_DATA_DIR", dir.as_os_str()),
                ("AGENTDUST_SESSION", OsStr::new(SENTINEL_TAG)),
            ],
            None,
            input.as_bytes(),
        );
        assert_silent_success(&output);
    }
    let secret = stored_secret(&dir);
    let expected = cwd_key(&secret, &cwd).unwrap();
    let recorded = keys(&dir);
    assert_eq!(recorded.len(), 4);
    assert!(
        recorded
            .iter()
            .all(|key| key.as_deref() == Some(expected.as_str()))
    );
    assert_eq!(names(&dir), journal_and_secret());
    for file in all_files(&dir) {
        let bytes = fs::read(&file).unwrap();
        for needle in [
            SENTINEL_DIR,
            SENTINEL_NAME,
            SENTINEL_COMMAND,
            SENTINEL_OUTPUT,
            SENTINEL_TAG,
            cwd.as_str(),
        ] {
            assert!(!contains(&bytes, needle.as_bytes()), "{needle} in {file:?}");
        }
        if file.file_name().unwrap() != SECRET_FILE {
            assert!(!contains(&bytes, secret.as_bytes()), "the secret in {file:?}");
            assert!(!contains(&bytes, hex(secret.as_bytes()).as_bytes()), "{file:?}");
        }
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_resolved_path_of_a_symlinked_cwd_is_not_stored_either() {
    let root = private_dir("cwd-privacy-link");
    let real = root.join("sentinel-real-9c1e").join("deep-project-5a7");
    fs::create_dir_all(&real).unwrap();
    let link = root.join("sentinel-link-3d8f");
    symlink(&real, &link).unwrap();
    let data = root.join("data");
    assert_silent_success(&run_in(&data, "s1", link.to_str().unwrap()));
    assert_eq!(keys(&data).len(), 1);
    assert!(keys(&data)[0].is_some());
    let root_text = root.to_str().unwrap().to_owned();
    for file in all_files(&data) {
        let bytes = fs::read(&file).unwrap();
        for needle in [
            "sentinel-real-9c1e",
            "deep-project-5a7",
            "sentinel-link-3d8f",
            root_text.as_str(),
        ] {
            assert!(!contains(&bytes, needle.as_bytes()), "{needle} in {file:?}");
        }
    }
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn hooks_that_create_the_secret_together_all_key_with_the_one_secret() {
    for round in 0..4 {
        let dir = scratch_dir(&format!("cwd-race-{round}"));
        let cwds: Vec<String> = (0..16)
            .map(|i| format!("/agentdust-fixture/project-{i}"))
            .collect();
        let inputs: Vec<Vec<u8>> = cwds
            .iter()
            .enumerate()
            .map(|(i, cwd)| {
                pre_tool_use_with(
                    &format!("s{i}"),
                    &format!("toolu_{i}"),
                    &cwd_field(&format!("\"{cwd}\"")),
                )
            })
            .collect();
        for output in run_hooks_together(&dir, &inputs) {
            assert_silent_success(&output);
        }
        let report = journal::read(&dir).unwrap();
        assert_eq!(report.records.len(), 16, "round {round}");
        let secret = stored_secret(&dir);
        for (i, cwd) in cwds.iter().enumerate() {
            let record = report
                .records
                .iter()
                .find(|record| record.session_id == format!("s{i}"))
                .unwrap();
            assert_eq!(record.cwd_key, cwd_key(&secret, cwd), "round {round} hook {i}");
        }
        assert_eq!(names(&dir), journal_and_secret(), "round {round}");
        fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn hooks_started_together_leave_an_existing_secret_alone() {
    let dir = scratch_dir("cwd-race-existing");
    let existing = load_or_create(&dir).unwrap();
    let path = dir.join(SECRET_FILE);
    let inode = fs::metadata(&path).unwrap().ino();
    let inputs: Vec<Vec<u8>> = (0..16)
        .map(|i| pre_tool_use(&format!("s{i}"), &format!("toolu_{i}")))
        .collect();
    for output in run_hooks_together(&dir, &inputs) {
        assert_silent_success(&output);
    }
    assert_eq!(fs::read(&path).unwrap(), existing.as_bytes());
    assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
    let expected = cwd_key(&existing, FIXTURE_CWD).unwrap();
    let recorded = keys(&dir);
    assert_eq!(recorded.len(), 16);
    assert!(
        recorded
            .iter()
            .all(|key| key.as_deref() == Some(expected.as_str()))
    );
    fs::remove_dir_all(&dir).unwrap();
}
