mod journal_strategies;
mod journal_support;
mod scratch;

use std::collections::BTreeSet;

use agentdust_core::journal::{MAX_FRAME_LEN, ReadReport, Record, decode, encode};
use journal_strategies::{CHUNKS, arb_record, small_record};
use journal_support::{Dribble, journal, non_empty_segments, plant, presentation_order_violation};
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::sample::select;
use scratch::TempDir;

const RS: u8 = 0x1e;
const FILLERS: [u8; 8] = *b"x{[\"\\\r0 ";
const JUNK_LENGTHS: [usize; 10] = [
    0,
    1,
    2,
    100,
    MAX_FRAME_LEN - 2,
    MAX_FRAME_LEN - 1,
    MAX_FRAME_LEN,
    MAX_FRAME_LEN + 1,
    2 * MAX_FRAME_LEN,
    3 * MAX_FRAME_LEN + 7,
];

fn accounted(report: &ReadReport) -> usize {
    report.records.len() + report.skipped_lines()
}

#[derive(Debug, Clone)]
enum Item {
    Valid(Record),
    Junk { filler: u8, len: usize, framed: bool },
}

fn arb_item() -> impl Strategy<Value = Item> {
    prop_oneof![
        3 => arb_record().prop_map(Item::Valid),
        2 => (select(FILLERS.to_vec()), select(JUNK_LENGTHS.to_vec()), any::<bool>())
            .prop_map(|(filler, len, framed)| Item::Junk { filler, len, framed }),
    ]
}

fn render(item: &Item) -> Vec<u8> {
    match item {
        Item::Valid(record) => encode(record).unwrap(),
        Item::Junk { filler, len, framed } => {
            let mut bytes = Vec::new();
            if *framed {
                bytes.push(RS);
            }
            bytes.extend(std::iter::repeat_n(*filler, *len));
            bytes.push(b'\n');
            bytes
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Edit {
    Flip(usize, u8),
    Delete(usize),
    Insert(usize, u8),
    Truncate(usize),
    Repeat(usize, usize),
}

fn arb_edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        (any::<usize>(), any::<u8>()).prop_map(|(at, byte)| Edit::Flip(at, byte)),
        any::<usize>().prop_map(Edit::Delete),
        (any::<usize>(), any::<u8>()).prop_map(|(at, byte)| Edit::Insert(at, byte)),
        any::<usize>().prop_map(Edit::Truncate),
        (any::<usize>(), 1usize..=40).prop_map(|(at, len)| Edit::Repeat(at, len)),
    ]
}

fn apply(bytes: &mut Vec<u8>, edit: Edit) {
    if bytes.is_empty() {
        return;
    }
    match edit {
        Edit::Flip(at, byte) => {
            let at = at % bytes.len();
            bytes[at] = byte;
        }
        Edit::Delete(at) => {
            bytes.remove(at % bytes.len());
        }
        Edit::Insert(at, byte) => bytes.insert(at % bytes.len(), byte),
        Edit::Truncate(at) => bytes.truncate(at % bytes.len()),
        Edit::Repeat(at, len) => {
            let at = at % bytes.len();
            let end = (at + len).min(bytes.len());
            let piece = bytes[at..end].to_vec();
            bytes.splice(at..at, piece);
        }
    }
}

fn damaged_frames() -> impl Strategy<Value = Vec<u8>> {
    let one = (arb_record(), vec(arb_edit(), 0..4)).prop_map(|(record, edits)| {
        let mut bytes = encode(&record).unwrap();
        for edit in edits {
            apply(&mut bytes, edit);
        }
        bytes
    });
    vec(one, 0..6).prop_map(|frames| frames.concat())
}

fn arbitrary_bytes() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        1 => vec(prop_oneof![8 => any::<u8>(), 1 => Just(b'\n'), 1 => Just(RS)], 0..2048),
        3 => damaged_frames(),
    ]
}

fn padded_to(total: usize) -> Record {
    let mut record = journal_support::named("");
    let base = encode(&record).unwrap().len();
    record.session_id = "x".repeat(total - base);
    record
}

#[test]
fn the_encoder_and_the_decoder_agree_on_the_last_accepted_byte() {
    let first = padded_to(150);
    for total in [MAX_FRAME_LEN - 1, MAX_FRAME_LEN] {
        let fits = padded_to(total);
        assert_eq!(encode(&fits).unwrap().len(), total);
        let mut bytes = encode(&first).unwrap();
        bytes.extend(encode(&fits).unwrap());
        bytes.extend(encode(&first).unwrap());
        let report = decode(&bytes[..]).unwrap();
        assert_eq!(report.records, [first.clone(), fits, first.clone()]);
        assert_eq!(report.skipped_lines(), 0);
    }
    let over = padded_to(MAX_FRAME_LEN + 1);
    assert!(encode(&over).is_err());
    let mut forced = encode(&first).unwrap();
    forced.push(RS);
    forced.extend(serde_json::to_vec(&over).unwrap());
    forced.push(b'\n');
    forced.extend(encode(&first).unwrap());
    let report = decode(&forced[..]).unwrap();
    assert_eq!(report.records, [first.clone(), first]);
    assert_eq!(report.malformed_lines, 1);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn every_frame_holds_one_separator_one_newline_and_decodes_to_its_record(record in arb_record()) {
        let frame = encode(&record).unwrap();
        prop_assert_eq!(frame.iter().filter(|byte| **byte == RS).count(), 1);
        prop_assert_eq!(frame.iter().filter(|byte| **byte == b'\n').count(), 1);
        prop_assert!(frame.len() <= MAX_FRAME_LEN);
        let report = decode(&frame[..]).unwrap();
        prop_assert_eq!(report, ReadReport { records: vec![record], ..ReadReport::default() });
    }

    #[test]
    fn the_report_does_not_depend_on_where_a_read_call_ends(
        records in vec(arb_record(), 0..6),
        chunk in select(CHUNKS.to_vec()),
    ) {
        let bytes: Vec<u8> = records.iter().flat_map(|record| encode(record).unwrap()).collect();
        let report = decode(Dribble { rest: &bytes, chunk }).unwrap();
        prop_assert_eq!(report.skipped_lines(), 0);
        prop_assert_eq!(report.records, records);
    }

    #[test]
    fn a_frame_of_any_text_that_the_encoder_accepts_is_one_the_decoder_accepts(
        mut record in arb_record(),
        filler in vec(select(vec!['a', '\n', 'é', '😀', '\u{1}', '\u{1e}']), 12_000..=26_000),
    ) {
        record.session_id = String::from_iter(filler);
        match encode(&record) {
            Ok(frame) => {
                prop_assert!(frame.len() <= MAX_FRAME_LEN);
                prop_assert_eq!(decode(&frame[..]).unwrap().records, vec![record]);
            }
            Err(agentdust_core::journal::JournalError::TooLarge { len, max }) => {
                prop_assert_eq!(max, MAX_FRAME_LEN);
                prop_assert!(len > max);
                prop_assert_eq!(len, serde_json::to_vec(&record).unwrap().len() + 2);
            }
            Err(other) => prop_assert!(false, "{other}"),
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn a_segment_of_any_length_never_swallows_the_records_around_it(
        items in vec(arb_item(), 0..6),
        tail in proptest::option::of((select(FILLERS.to_vec()), select(JUNK_LENGTHS[1..].to_vec()))),
    ) {
        let mut bytes: Vec<u8> = items.iter().flat_map(render).collect();
        if let Some((filler, len)) = tail {
            bytes.push(RS);
            bytes.extend(std::iter::repeat_n(filler, len));
        }
        let report = decode(&bytes[..]).unwrap();
        let valid: Vec<Record> = items
            .iter()
            .filter_map(|item| match item {
                Item::Valid(record) => Some(record.clone()),
                Item::Junk { .. } => None,
            })
            .collect();
        let junk = items
            .iter()
            .filter(|item| matches!(item, Item::Junk { len, .. } if *len > 0))
            .count();
        prop_assert_eq!(report.records, valid);
        prop_assert_eq!(report.malformed_lines, junk);
        prop_assert_eq!(report.torn_frames, usize::from(tail.is_some()));
        prop_assert_eq!(report.truncated_last_line, tail.is_some());
        prop_assert_eq!(report.newer_version_lines + report.unknown_kind_lines, 0);
    }

    #[test]
    fn a_segment_split_across_any_chunking_is_still_one_segment(
        items in vec(arb_item(), 0..4),
        chunk in select(CHUNKS.to_vec()),
    ) {
        let bytes: Vec<u8> = items.iter().flat_map(render).collect();
        let whole = decode(&bytes[..]).unwrap();
        let dribbled = decode(Dribble { rest: &bytes, chunk }).unwrap();
        prop_assert_eq!(whole, dribbled);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn damaged_frames_never_panic_and_every_segment_is_counted_once(bytes in arbitrary_bytes()) {
        let report = decode(&bytes[..]).unwrap();
        prop_assert_eq!(accounted(&report), non_empty_segments(&bytes));
        prop_assert_eq!(report.unsupported_version, report.newer_version_lines > 0);
        prop_assert!(report.torn_frames >= usize::from(report.truncated_last_line));
    }

    #[test]
    fn a_record_the_decoder_accepts_survives_being_written_again(bytes in arbitrary_bytes()) {
        let report = decode(&bytes[..]).unwrap();
        for record in &report.records {
            let again = encode(record).unwrap();
            let back = decode(&again[..]).unwrap();
            prop_assert_eq!(&back.records, std::slice::from_ref(record));
        }
    }

    #[test]
    fn whatever_precedes_a_frame_the_frame_is_read(bytes in arbitrary_bytes(), next in arb_record()) {
        let before = decode(&bytes[..]).unwrap();
        let mut stream = bytes.clone();
        stream.extend(encode(&next).unwrap());
        let after = decode(&stream[..]).unwrap();
        let mut expected = before.records.clone();
        expected.push(next);
        prop_assert_eq!(after.records, expected);
        prop_assert_eq!(after.malformed_lines, before.malformed_lines);
        prop_assert_eq!(after.torn_frames, before.torn_frames);
        prop_assert!(!after.truncated_last_line);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn the_file_reader_counts_what_the_decoder_counts(bytes in arbitrary_bytes()) {
        let dir = TempDir::private("codec-props-file");
        plant(&dir, "journal.jsonl", &bytes);
        let from_file = journal(&dir).read().unwrap();
        let decoded = decode(&bytes[..]).unwrap();
        prop_assert_eq!(from_file.malformed_lines, decoded.malformed_lines);
        prop_assert_eq!(from_file.torn_frames, decoded.torn_frames);
        prop_assert_eq!(from_file.newer_version_lines, decoded.newer_version_lines);
        prop_assert_eq!(from_file.unknown_kind_lines, decoded.unknown_kind_lines);
        prop_assert_eq!(from_file.truncated_last_line, decoded.truncated_last_line);
        prop_assert_eq!(from_file.unsupported_version, decoded.unsupported_version);
        prop_assert_eq!(from_file.records.len() + from_file.duplicates_removed, decoded.records.len());
        prop_assert_eq!(presentation_order_violation(&from_file.records), None);
    }
}

fn distribute(records: &[Record], assignment: &[usize], files: usize) -> ReadReport {
    let dir = TempDir::private("codec-props-layout");
    let mut contents: Vec<Vec<u8>> = vec![Vec::new(); files];
    for (record, target) in records.iter().zip(assignment) {
        contents[target % files].extend(encode(record).unwrap());
    }
    let last = files - 1;
    for (index, bytes) in contents.iter().enumerate() {
        let name = if index == last {
            "journal.jsonl".to_owned()
        } else {
            format!("journal.{}.jsonl", 1_000 + index)
        };
        plant(&dir, &name, bytes);
    }
    journal(&dir).read().unwrap()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn records_spread_over_files_come_back_once_each_in_presentation_order(
        records in vec(small_record(), 0..16),
        assignment in vec(0usize..4, 16),
    ) {
        let report = distribute(&records, &assignment, 4);
        let distinct: BTreeSet<&Record> = records.iter().collect();
        let returned: BTreeSet<&Record> = report.records.iter().collect();
        prop_assert_eq!(returned.len(), report.records.len());
        prop_assert_eq!(&returned, &distinct);
        prop_assert_eq!(report.duplicates_removed, records.len() - distinct.len());
        prop_assert_eq!(presentation_order_violation(&report.records), None);
    }

    #[test]
    fn the_order_does_not_depend_on_which_file_holds_which_record(
        records in vec(small_record(), 0..16),
        first in vec(0usize..4, 16),
        second in vec(0usize..4, 16),
    ) {
        let one = distribute(&records, &first, 4);
        let other = distribute(&records, &second, 3);
        let reversed: Vec<Record> = records.iter().rev().cloned().collect();
        let third = distribute(&reversed, &first, 2);
        prop_assert_eq!(&one.records, &other.records);
        prop_assert_eq!(&one.records, &third.records);
    }
}
