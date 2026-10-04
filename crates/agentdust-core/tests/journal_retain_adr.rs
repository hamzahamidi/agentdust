mod journal_support;
mod maintenance_support;
mod scratch;

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use agentdust_core::journal::{COMPACT_TMP, CORRUPT_PREFIX, MAINT_FILE, MaintenanceError, SCHEMA_VERSION};
use journal_support::{frame, generation_name, join, journal, plant};
use maintenance_support::{
    BOOT, T, corrupt_copies, expired, inode_of, keep_everything, numbered, plant_generation, snapshot,
    without_lock,
};
use scratch::{TempDir, make_fifo};

const ADR: &str = include_str!("../../../docs/m1/adr-journal-format.md");
const STAMP: u64 = 100;
const OLDER: u64 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Left,
    Replaced,
    Deleted,
    Refused,
}

struct Row {
    id: String,
    outcome: Outcome,
    copied: bool,
}

fn retention_section() -> &'static str {
    let start = ADR
        .find("\n## Retention\n")
        .expect("the ADR has a Retention section");
    let rest = &ADR[start + 1..];
    let end = rest[3..].find("\n## ").map_or(rest.len(), |at| at + 3);
    &rest[..end]
}

fn rows() -> Vec<Row> {
    retention_section()
        .lines()
        .filter(|line| line.starts_with("| G"))
        .map(|line| {
            let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
            assert_eq!(cells.len(), 4, "{line}");
            let outcome = if cells[2].starts_with("left as it is") {
                Outcome::Left
            } else if cells[2].starts_with("replaced") {
                Outcome::Replaced
            } else if cells[2].starts_with("deleted") {
                Outcome::Deleted
            } else if cells[2].starts_with("the run is refused") {
                Outcome::Refused
            } else {
                panic!("an outcome the tests do not know: {line}");
            };
            let copied = match cells[3] {
                "yes" => true,
                "no" => false,
                other => panic!("{other} in {line}"),
            };
            Row {
                id: cells[0].to_owned(),
                outcome,
                copied,
            }
        })
        .collect()
}

const GARBAGE: &[u8] = b"\xff\xfe this is not json\n";

fn future_line(padding: usize) -> Vec<u8> {
    let json = format!(
        "{{\"v\":{},\"kind\":\"later\",\"pad\":\"{}\"}}",
        SCHEMA_VERSION + 1,
        "x".repeat(padding)
    );
    join(&[&[0x1e], json.as_bytes(), b"\n"])
}

fn unknown_kind_line() -> Vec<u8> {
    let mut value = serde_json::to_value(numbered("mystery", 9)).unwrap();
    value["kind"] = serde_json::json!("mystery");
    join(&[&[0x1e], &serde_json::to_vec(&value).unwrap(), b"\n"])
}

fn torn() -> Vec<u8> {
    b"\x1e{\"v\":1,\"kind\"".to_vec()
}

fn generation_of(dir: &Path, parts: &[&[u8]]) {
    plant(dir, &generation_name(STAMP), &parts.concat());
}

type Plant = fn(&Path);

fn scenarios() -> Vec<(&'static str, Vec<Plant>)> {
    vec![
        (
            "G1",
            vec![
                |dir: &Path| {
                    generation_of(
                        dir,
                        &[
                            &frame(&expired("gone", 1)),
                            &future_line(10),
                            GARBAGE,
                            &frame(&numbered("kept", 2)),
                        ],
                    )
                },
                |dir: &Path| generation_of(dir, &[&frame(&expired("gone", 1)), &future_line(70_000)]),
            ],
        ),
        (
            "G2",
            vec![|dir: &Path| generation_of(dir, &[&frame(&numbered("a", 1)), &frame(&numbered("b", 2))])],
        ),
        (
            "G3",
            vec![|dir: &Path| generation_of(dir, &[&frame(&numbered("a", 1)), &unknown_kind_line()])],
        ),
        (
            "G4",
            vec![|dir: &Path| generation_of(dir, &[&frame(&numbered("a", 1)), &torn()])],
        ),
        (
            "G5",
            vec![|dir: &Path| generation_of(dir, &[&frame(&numbered("a", 1)), GARBAGE])],
        ),
        (
            "G6",
            vec![|dir: &Path| {
                generation_of(dir, &[&frame(&expired("gone", 1)), &frame(&numbered("kept", 2))])
            }],
        ),
        (
            "G7",
            vec![|dir: &Path| generation_of(dir, &[&frame(&expired("gone", 1))])],
        ),
        (
            "G8",
            vec![|dir: &Path| {
                plant(dir, &generation_name(OLDER), &frame(&numbered("a", 1)));
                generation_of(dir, &[&frame(&numbered("a", 1)), &frame(&numbered("b", 2))]);
            }],
        ),
        (
            "G9",
            vec![|dir: &Path| {
                plant(
                    dir,
                    &generation_name(OLDER),
                    &join(&[&frame(&numbered("a", 1)), &frame(&numbered("b", 2))]),
                );
                generation_of(dir, &[&frame(&numbered("b", 2)), &frame(&numbered("a", 1))]);
            }],
        ),
        (
            "G10",
            vec![|dir: &Path| generation_of(dir, &[&frame(&expired("gone", 1)), &unknown_kind_line()])],
        ),
        (
            "G11",
            vec![|dir: &Path| {
                generation_of(
                    dir,
                    &[&frame(&expired("gone", 1)), &frame(&numbered("kept", 2)), &torn()],
                )
            }],
        ),
        (
            "G12",
            vec![|dir: &Path| {
                generation_of(
                    dir,
                    &[&frame(&expired("gone", 1)), GARBAGE, &frame(&numbered("kept", 2))],
                )
            }],
        ),
        (
            "G13",
            vec![|dir: &Path| generation_of(dir, &[&frame(&expired("gone", 1)), GARBAGE])],
        ),
        ("G14", vec![|dir: &Path| generation_of(dir, &[])]),
        (
            "G15",
            vec![
                |dir: &Path| {
                    std::os::unix::fs::symlink(dir.join("elsewhere"), dir.join(generation_name(STAMP)))
                        .unwrap()
                },
                |dir: &Path| make_fifo(&dir.join(generation_name(STAMP))),
                |dir: &Path| fs::create_dir(dir.join(generation_name(STAMP))).unwrap(),
                |dir: &Path| {
                    generation_of(dir, &[&frame(&expired("gone", 1))]);
                    fs::hard_link(dir.join(generation_name(STAMP)), dir.join("alias")).unwrap();
                },
                |dir: &Path| {
                    generation_of(dir, &[&frame(&expired("gone", 1))]);
                    fs::set_permissions(
                        dir.join(generation_name(STAMP)),
                        fs::Permissions::from_mode(0o644),
                    )
                    .unwrap();
                },
            ],
        ),
    ]
}

fn observe(plant_it: Plant) -> (Outcome, bool) {
    let dir = TempDir::private("adr-row");
    plant_it(&dir);
    let path = dir.join(generation_name(STAMP));
    let before = fs::symlink_metadata(&path)
        .ok()
        .filter(|m| m.is_file())
        .map(|_| (fs::read(&path).unwrap(), inode_of(&path)));
    let files = without_lock(snapshot(&dir));

    let result = journal(&dir).retain(&keep_everything(), T, BOOT);

    if let Err(MaintenanceError::Refused { .. }) = result {
        assert_eq!(
            without_lock(snapshot(&dir)),
            files,
            "a refused run changed something"
        );
        return (Outcome::Refused, !corrupt_copies(&dir).is_empty());
    }
    result.unwrap();
    let (bytes, inode) = before.expect("only the refusal rows plant something that is not a file");
    let copied = !corrupt_copies(&dir).is_empty();
    if !path.exists() {
        return (Outcome::Deleted, copied);
    }
    if fs::read(&path).unwrap() == bytes && inode_of(&path) == inode {
        return (Outcome::Left, copied);
    }
    (Outcome::Replaced, copied)
}

#[test]
fn every_row_of_the_adr_table_is_run_against_the_code_and_agrees() {
    let table = rows();
    assert!(!table.is_empty(), "the Retention section has no outcome table");
    let by_id = scenarios();
    for row in &table {
        let (_, plants) = by_id
            .iter()
            .find(|(id, _)| *id == row.id)
            .unwrap_or_else(|| panic!("the ADR has row {} and no test runs it", row.id));
        for (n, plant_it) in plants.iter().enumerate() {
            let (outcome, copied) = observe(*plant_it);
            assert_eq!(outcome, row.outcome, "{} variant {n}", row.id);
            assert_eq!(copied, row.copied, "{} variant {n}", row.id);
        }
    }
}

#[test]
fn every_test_scenario_is_a_row_of_the_adr_table() {
    let in_adr: BTreeSet<String> = rows().into_iter().map(|row| row.id).collect();
    let in_tests: BTreeSet<String> = scenarios().into_iter().map(|(id, _)| id.to_owned()).collect();
    assert_eq!(in_adr, in_tests);
    assert_eq!(rows().len(), in_adr.len(), "a row id appears twice");
}

#[test]
fn the_adr_names_the_files_the_code_uses() {
    let section = retention_section();
    for name in [MAINT_FILE, COMPACT_TMP, CORRUPT_PREFIX] {
        assert!(
            section.contains(name),
            "the Retention section does not mention {name}"
        );
    }
}

#[test]
fn the_adr_says_retention_never_visits_the_active_file_and_the_code_does_not() {
    assert!(retention_section().contains("never visits the active file"));
    let dir = TempDir::private("adr-active");
    plant(&dir, "journal.jsonl", &frame(&expired("old", 1)));
    plant_generation(&dir, STAMP, &[expired("gone", 2), numbered("kept", 3)]);
    let active = dir.join("journal.jsonl");
    let (bytes, inode) = (fs::read(&active).unwrap(), inode_of(&active));
    journal(&dir).retain(&keep_everything(), T, BOOT).unwrap();
    assert_eq!(fs::read(&active).unwrap(), bytes);
    assert_eq!(inode_of(&active), inode);
}

#[test]
fn the_adr_says_no_record_moves_and_after_a_run_each_record_is_in_one_file() {
    assert!(retention_section().contains("A record is never moved to another file"));
    let dir = TempDir::private("adr-no-move");
    plant_generation(&dir, OLDER, &[numbered("a", 1), numbered("b", 2)]);
    plant_generation(
        &dir,
        STAMP,
        &[numbered("b", 2), expired("gone", 3), numbered("c", 4)],
    );
    journal(&dir).retain(&keep_everything(), T, BOOT).unwrap();
    for session in ["a", "b", "c"] {
        assert_eq!(journal_support::copies_on_disk(&dir, session), 1, "{session}");
    }
    assert!(!dir.join("journal.jsonl").exists());
}
