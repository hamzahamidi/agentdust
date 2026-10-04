mod journal_strategies;
mod journal_support;
mod scratch;

use agentdust_core::journal::{JournalError, Record, decode, encode};
use journal_strategies::{arb_record, in_append_order};
use journal_support::{Scripted, Step, journal, rotate_by_rename};
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::sample::Index;
use scratch::TempDir;

fn frames_of(records: &[Record]) -> Vec<Vec<u8>> {
    records.iter().map(|record| encode(record).unwrap()).collect()
}

fn others(records: &[Record], skipped: &[usize]) -> Vec<Record> {
    records
        .iter()
        .enumerate()
        .filter(|(at, _)| !skipped.contains(at))
        .map(|(_, record)| record.clone())
        .collect()
}

fn stream_with_cuts(frames: &[Vec<u8>], kept: &[Option<usize>]) -> Vec<u8> {
    frames
        .iter()
        .zip(kept)
        .flat_map(|(frame, kept)| frame[..kept.unwrap_or(frame.len())].to_vec())
        .collect()
}

#[test]
fn a_cut_at_every_length_of_every_record_of_a_stream_loses_only_that_record() {
    let records = in_append_order(
        (0..3)
            .map(|n| {
                let mut record = journal_support::named(&format!("s{n}"));
                record.session_id.push_str(&"x".repeat(40 * n));
                record
            })
            .collect(),
    );
    let frames = frames_of(&records);
    for cut in 0..frames.len() {
        for kept in 0..frames[cut].len() {
            let mut cuts = vec![None; frames.len()];
            cuts[cut] = Some(kept);
            let report = decode(&stream_with_cuts(&frames, &cuts)[..]).unwrap();
            assert_eq!(
                report.records,
                others(&records, &[cut]),
                "record {cut} cut at {kept}"
            );
            assert_eq!(report.malformed_lines, 0, "record {cut} cut at {kept}");
            assert_eq!(
                report.torn_frames,
                usize::from(kept >= 2),
                "record {cut} cut at {kept}"
            );
            assert_eq!(
                report.truncated_last_line,
                kept >= 2 && cut == frames.len() - 1,
                "record {cut} cut at {kept}"
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn a_stream_with_one_record_truncated_still_yields_every_other_record(
        records in vec(arb_record(), 1..12),
        cut in any::<Index>(),
        kept in any::<Index>(),
    ) {
        let frames = frames_of(&records);
        let cut = cut.index(frames.len());
        let kept = kept.index(frames[cut].len());
        let mut cuts = vec![None; frames.len()];
        cuts[cut] = Some(kept);
        let report = decode(&stream_with_cuts(&frames, &cuts)[..]).unwrap();
        prop_assert_eq!(report.records, others(&records, &[cut]));
        prop_assert_eq!(report.malformed_lines, 0);
        prop_assert_eq!(report.newer_version_lines + report.unknown_kind_lines, 0);
        prop_assert_eq!(report.torn_frames, usize::from(kept >= 2));
        prop_assert_eq!(report.truncated_last_line, kept >= 2 && cut == frames.len() - 1);
    }

    #[test]
    fn a_stream_with_any_records_truncated_loses_those_and_no_other(
        records in vec((arb_record(), proptest::option::of(any::<Index>())), 1..14),
    ) {
        let records_only: Vec<Record> = records.iter().map(|(record, _)| record.clone()).collect();
        let frames = frames_of(&records_only);
        let kept: Vec<Option<usize>> = records
            .iter()
            .zip(&frames)
            .map(|((_, cut), frame)| cut.map(|cut| cut.index(frame.len())))
            .collect();
        let report = decode(&stream_with_cuts(&frames, &kept)[..]).unwrap();
        let cut_at: Vec<usize> = kept
            .iter()
            .enumerate()
            .filter(|(_, kept)| kept.is_some())
            .map(|(at, _)| at)
            .collect();
        prop_assert_eq!(report.records, others(&records_only, &cut_at));
        prop_assert_eq!(report.malformed_lines, 0);
        prop_assert_eq!(
            report.torn_frames,
            kept.iter().filter(|kept| kept.is_some_and(|kept| kept >= 2)).count()
        );
    }

    #[test]
    fn a_truncated_record_costs_nothing_when_it_is_followed_by_a_valid_frame_after_any_noise(
        before in arb_record(),
        cut in arb_record(),
        after in arb_record(),
        kept in any::<Index>(),
        noise in proptest::collection::vec(any::<u8>().prop_filter("no separator", |byte| *byte != b'\n' && *byte != 0x1e), 0..64),
    ) {
        let whole = encode(&cut).unwrap();
        let kept = kept.index(whole.len());
        let mut bytes = encode(&before).unwrap();
        bytes.extend_from_slice(&whole[..kept]);
        bytes.extend_from_slice(&noise);
        bytes.extend(encode(&after).unwrap());
        let report = decode(&bytes[..]).unwrap();
        prop_assert_eq!(report.records.first(), Some(&before));
        prop_assert_eq!(report.records.last(), Some(&after));
    }
}

#[derive(Debug, Clone, Copy)]
enum Fault {
    ShortWrite,
    LandThenFail,
}

fn fault() -> impl Strategy<Value = Fault> {
    prop_oneof![Just(Fault::ShortWrite), Just(Fault::LandThenFail)]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn a_failed_append_in_a_stream_of_real_appends_costs_that_record_and_no_other(
        records in vec(arb_record(), 1..8),
        cut in any::<Index>(),
        kept in any::<Index>(),
        fault in fault(),
        rotate_after in any::<bool>(),
    ) {
        let records = in_append_order(records);
        let cut = cut.index(records.len());
        let kept = kept.index(encode(&records[cut]).unwrap().len());
        let dir = TempDir::private("truncation-props");
        let journal = journal(&dir);
        for (at, record) in records.iter().enumerate() {
            if at != cut {
                prop_assert_eq!(journal.append(record).unwrap().attempts, 1);
                continue;
            }
            let step = match fault {
                Fault::ShortWrite => Step::Take(kept),
                Fault::LandThenFail => Step::LandThenFail(kept, libc::EIO),
            };
            let result = journal.append_with(record, &mut Scripted::new([step]));
            match (fault, result) {
                (Fault::ShortWrite, Err(JournalError::ShortWrite { written, .. })) => prop_assert_eq!(written, kept),
                (Fault::LandThenFail, Err(JournalError::Io(_))) => {}
                (_, other) => prop_assert!(false, "{other:?}"),
            }
            if rotate_after {
                rotate_by_rename(&dir, 1_000);
            }
        }
        let report = journal.read().unwrap();
        prop_assert_eq!(report.records, others(&records, &[cut]));
        prop_assert_eq!(report.malformed_lines, 0);
        prop_assert_eq!(report.torn_frames, usize::from(kept >= 2));
        prop_assert_eq!(
            report.truncated_last_line,
            kept >= 2 && (cut == records.len() - 1 || rotate_after)
        );
    }
}
