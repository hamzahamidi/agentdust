mod journal_support;
mod scratch;

use std::collections::BTreeSet;
use std::path::Path;

use agentdust_core::journal::{ReadReport, Record, SCHEMA_VERSION, decode, encode};
use journal_support::{Dribble, journal, non_empty_segments, plant, presentation_order_violation, sessions};
use scratch::TempDir;

const FILE_SEPARATOR: u8 = 0x1c;
const MAX_FILES: usize = 4;
const CHUNKS: [usize; 6] = [1, 7, 4096, 65_535, 65_536, 65_537];

fn seeds(target: &str) -> Vec<(String, Vec<u8>)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fuzz/seeds")
        .join(target);
    let mut seeds: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("{}: {err}", dir.display()))
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().into_string().unwrap(),
                std::fs::read(entry.path()).unwrap(),
            )
        })
        .collect();
    seeds.sort();
    seeds
}

fn seed(target: &str, name: &str) -> Vec<u8> {
    seeds(target)
        .into_iter()
        .find(|(found, _)| found == name)
        .unwrap_or_else(|| panic!("no seed {name} in {target}"))
        .1
}

fn accounted(report: &ReadReport) -> usize {
    report.records.len() + report.skipped_lines()
}

fn sections(data: &[u8]) -> Vec<&[u8]> {
    data.splitn(MAX_FILES, |byte| *byte == FILE_SEPARATOR).collect()
}

fn read_sections(parts: &[&[u8]]) -> ReadReport {
    let dir = TempDir::private("journal-corpus");
    let last = parts.len() - 1;
    for (index, part) in parts.iter().enumerate() {
        let name = if index == last {
            "journal.jsonl".to_owned()
        } else {
            format!("journal.{}.jsonl", 1_000 + index)
        };
        plant(&dir, &name, part);
    }
    journal(&dir).read().unwrap()
}

fn decoded(target: &str, name: &str) -> ReadReport {
    decode(&seed(target, name)[..]).unwrap()
}

#[test]
fn every_decode_seed_is_decoded_and_every_segment_counted_once() {
    let seeds = seeds("journal_decode");
    assert!(seeds.len() >= 30, "{} seeds", seeds.len());
    for (name, data) in &seeds {
        let report = decode(&data[..]).unwrap();
        assert_eq!(accounted(&report), non_empty_segments(data), "{name}");
        assert_eq!(
            report.unsupported_version,
            report.newer_version_lines > 0,
            "{name}"
        );
        assert!(
            report.torn_frames >= usize::from(report.truncated_last_line),
            "{name}"
        );
        for chunk in CHUNKS {
            assert_eq!(
                decode(Dribble { rest: data, chunk }).unwrap(),
                report,
                "{name} in chunks of {chunk}"
            );
        }
        for record in &report.records {
            let mut current_record = record.clone();
            current_record.v = SCHEMA_VERSION;
            let again = encode(&current_record).unwrap();
            assert_eq!(
                decode(&again[..]).unwrap().records,
                std::slice::from_ref(&current_record),
                "{name}"
            );
        }
    }
}

#[test]
fn the_decode_seeds_reach_every_outcome_of_the_decoder() {
    let reports: Vec<ReadReport> = seeds("journal_decode")
        .iter()
        .map(|(_, data)| decode(&data[..]).unwrap())
        .collect();
    assert!(reports.iter().any(|report| !report.records.is_empty()));
    assert!(reports.iter().any(|report| report.malformed_lines > 0));
    assert!(reports.iter().any(|report| report.torn_frames > 0));
    assert!(reports.iter().any(|report| report.truncated_last_line));
    assert!(reports.iter().any(|report| report.newer_version_lines > 0));
    assert!(reports.iter().any(|report| report.unknown_kind_lines > 0));
    assert!(
        reports.iter().any(|report| report.torn_frames > 0
            && !report.records.is_empty()
            && report.malformed_lines == 0)
    );
}

#[test]
fn a_frame_cut_by_a_short_write_costs_the_cut_record_and_no_other() {
    let report = decoded("journal_decode", "cut-then-next-frame");
    assert_eq!(sessions(&report.records), ["s1", "s2", "s3"]);
    assert_eq!((report.torn_frames, report.malformed_lines), (1, 0));
    assert!(!report.truncated_last_line);

    let report = decoded("journal_decode", "cut-all-but-the-newline");
    assert_eq!(sessions(&report.records), ["s2"]);
    assert_eq!((report.torn_frames, report.malformed_lines), (1, 0));

    let report = decoded("journal_decode", "cut-after-one-byte");
    assert_eq!(sessions(&report.records), ["s1", "s2"]);
    assert_eq!(report.skipped_lines(), 0);

    let report = decoded("journal_decode", "cut-after-zero-bytes");
    assert_eq!(sessions(&report.records), ["s1", "s2"]);
}

#[test]
fn glued_records_cost_both_without_a_separator_and_only_the_cut_one_with_it() {
    let legacy = decoded("journal_decode", "cut-glued-bare-lines");
    assert!(legacy.records.is_empty());
    assert_eq!(legacy.malformed_lines, 1);

    let framed = decoded("journal_decode", "frames-glued-without-newline");
    assert_eq!(sessions(&framed.records), ["s3"]);
    assert_eq!((framed.torn_frames, framed.malformed_lines), (2, 0));
}

#[test]
fn a_tail_without_its_newline_is_torn_even_when_it_is_complete_json() {
    let report = decoded("journal_decode", "torn-tail-complete-json");
    assert_eq!(sessions(&report.records), ["s1"]);
    assert_eq!(report.torn_frames, 1);
    assert!(report.truncated_last_line);
}

#[test]
fn a_newer_version_line_over_the_cap_is_counted_as_newer_and_hides_nothing() {
    let report = decoded("journal_decode", "newer-version-over-cap");
    assert_eq!(report.newer_version_lines, 1);
    assert!(report.unsupported_version);
    assert_eq!(report.malformed_lines, 0);
    assert_eq!(sessions(&report.records), ["s1"]);

    let report = decoded("journal_decode", "version-not-first-over-cap");
    assert_eq!((report.newer_version_lines, report.malformed_lines), (0, 1));
    assert_eq!(sessions(&report.records), ["s1"]);

    let report = decoded("journal_decode", "version-one-over-cap");
    assert_eq!((report.newer_version_lines, report.malformed_lines), (0, 1));
}

#[test]
fn the_frame_cap_seeds_sit_on_both_sides_of_the_limit() {
    let at = decoded("journal_decode", "exactly-the-cap");
    assert_eq!(at.records.len(), 2);
    assert_eq!(at.skipped_lines(), 0);

    let over = decoded("journal_decode", "one-over-the-cap");
    assert_eq!(sessions(&over.records), ["s1"]);
    assert_eq!(over.malformed_lines, 1);

    let unterminated = decoded("journal_decode", "unterminated-over-cap");
    assert_eq!(sessions(&unterminated.records), ["s1"]);
    assert_eq!((unterminated.torn_frames, unterminated.malformed_lines), (1, 0));
    assert!(unterminated.truncated_last_line);
}

#[test]
fn every_read_seed_is_read_as_files_and_agrees_with_the_decoder() {
    let seeds = seeds("journal_read");
    assert!(seeds.len() >= 15, "{} seeds", seeds.len());
    for (name, data) in &seeds {
        let parts = sections(data);
        let report = read_sections(&parts);
        let each: Vec<ReadReport> = parts.iter().map(|part| decode(&part[..]).unwrap()).collect();
        let every: Vec<&Record> = each.iter().flat_map(|part| part.records.iter()).collect();
        let distinct: BTreeSet<&Record> = every.iter().copied().collect();
        assert_eq!(
            report.records.len() + report.duplicates_removed,
            every.len(),
            "{name}"
        );
        assert_eq!(report.records.iter().collect::<BTreeSet<_>>(), distinct, "{name}");
        assert_eq!(
            report.malformed_lines,
            each.iter().map(|part| part.malformed_lines).sum::<usize>(),
            "{name}"
        );
        assert_eq!(
            report.torn_frames,
            each.iter().map(|part| part.torn_frames).sum::<usize>(),
            "{name}"
        );
        assert_eq!(
            report.newer_version_lines,
            each.iter().map(|part| part.newer_version_lines).sum::<usize>(),
            "{name}"
        );
        assert_eq!(
            report.unknown_kind_lines,
            each.iter().map(|part| part.unknown_kind_lines).sum::<usize>(),
            "{name}"
        );
        assert_eq!(
            report.truncated_last_line,
            each.iter().any(|part| part.truncated_last_line),
            "{name}"
        );
        assert_eq!(
            report.unsupported_version,
            report.newer_version_lines > 0,
            "{name}"
        );
        assert_eq!(report.unsafe_files, 0, "{name}");
        assert_eq!(presentation_order_violation(&report.records), None, "{name}");
    }
}

#[test]
fn the_read_seeds_reach_duplicates_several_boots_and_a_newer_generation() {
    let reports: Vec<ReadReport> = seeds("journal_read")
        .iter()
        .map(|(_, data)| read_sections(&sections(data)))
        .collect();
    assert!(reports.iter().any(|report| report.duplicates_removed > 0));
    assert!(reports.iter().any(|report| report.unsupported_version));
    assert!(reports.iter().any(|report| report.torn_frames > 0));
    assert!(reports.iter().any(|report| report.unknown_kind_lines > 0));
    assert!(reports.iter().any(|report| {
        let mut boots: Vec<&str> = report.records.iter().map(|record| record.boot.as_str()).collect();
        boots.dedup();
        boots.len() > 1
    }));
}

#[test]
fn a_record_copied_into_two_files_by_a_late_writer_is_read_once() {
    let seed = seed("journal_read", "rotation-overlap-copy");
    let report = read_sections(&sections(&seed));
    assert_eq!(report.duplicates_removed, 1);
    assert_eq!(
        report
            .records
            .iter()
            .filter(|record| record.session_id == "late")
            .count(),
        1
    );

    let seed = journal_seed("same-record-three-times");
    let report = read_sections(&sections(&seed));
    assert_eq!(report.records.len(), 1);
    assert_eq!(report.duplicates_removed, 3);
}

fn journal_seed(name: &str) -> Vec<u8> {
    seed("journal_read", name)
}

#[test]
fn a_torn_frame_ends_one_file_and_does_not_reach_into_the_next() {
    let report = read_sections(&sections(&journal_seed("torn-tail-in-a-generation")));
    assert_eq!(report.torn_frames, 1);
    assert!(report.truncated_last_line);
    assert_eq!(report.records.len(), 3);
}

#[test]
fn a_newer_generation_over_the_cap_is_counted_and_the_other_files_still_read() {
    let report = read_sections(&sections(&journal_seed("newer-version-over-cap-in-a-generation")));
    assert_eq!(report.newer_version_lines, 1);
    assert!(report.unsupported_version);
    assert_eq!(report.malformed_lines, 0);
    assert_eq!(report.records.len(), 2);
}
