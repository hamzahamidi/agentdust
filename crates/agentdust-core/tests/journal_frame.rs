mod journal_support;

use agentdust_core::journal::{
    Class, JournalError, MAX_FRAME_LEN, RS, ReadReport, Record, decode, encode, scan,
};
use journal_support::{bare_line, edited_frame, frame, join, named, padded_to_frame_len, sessions};

fn rec(n: u64) -> Record {
    named(&format!("s{n}"))
}

fn newer_version_line(pad: usize) -> Vec<u8> {
    format!(
        "{{\"v\":3,\"kind\":\"future\",\"pad\":\"{}\"}}\n",
        "a".repeat(pad)
    )
    .into_bytes()
}

fn after_a_cut(kept: usize) -> ReadReport {
    let first = frame(&rec(1));
    let mut bytes = first[..kept.min(first.len())].to_vec();
    bytes.extend(frame(&rec(2)));
    decode(&bytes[..]).unwrap()
}

#[test]
fn a_frame_is_rs_then_compact_json_then_lf() {
    let framed = encode(&rec(1)).unwrap();
    let json = serde_json::to_vec(&rec(1)).unwrap();
    assert_eq!(framed[0], RS);
    assert_eq!(RS, 0x1e);
    assert_eq!(&framed[1..framed.len() - 1], &json[..]);
    assert_eq!(framed[framed.len() - 1], b'\n');
    assert_eq!(framed, frame(&rec(1)));
}

#[test]
fn json_text_never_holds_a_raw_separator_so_a_frame_has_one_rs_and_one_lf() {
    let mut record = rec(1);
    record.session_id = "a\u{1e}b\nc\rd\u{1f}".to_owned();
    let framed = encode(&record).unwrap();
    assert_eq!(framed.iter().filter(|byte| **byte == RS).count(), 1);
    assert_eq!(framed.iter().filter(|byte| **byte == b'\n').count(), 1);
    assert_eq!(decode(&framed[..]).unwrap().records, vec![record]);
}

#[test]
fn a_record_that_is_not_the_current_version_is_refused() {
    for v in [0, 1, 3, u32::MAX] {
        let mut record = rec(1);
        record.v = v;
        match encode(&record) {
            Err(JournalError::WrongVersion { found }) => assert_eq!(found, v),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn frames_decode_back_to_their_records_with_clean_counters() {
    let bytes: Vec<u8> = (1..=3).flat_map(|n| encode(&rec(n)).unwrap()).collect();
    let report = decode(&bytes[..]).unwrap();
    assert_eq!(sessions(&report.records), ["s1", "s2", "s3"]);
    assert_eq!(
        report,
        ReadReport {
            records: report.records.clone(),
            ..ReadReport::default()
        }
    );
}

#[test]
fn a_line_without_rs_is_still_a_record() {
    let report = decode(&join(&[&bare_line(&rec(1)), &frame(&rec(2))])[..]).unwrap();
    assert_eq!(sessions(&report.records), ["s1", "s2"]);
    assert_eq!(report.malformed_lines + report.torn_frames, 0);
}

#[test]
fn the_frame_cap_is_65536_bytes_including_both_delimiters() {
    assert_eq!(MAX_FRAME_LEN, 65_536);
    let fits = padded_to_frame_len(MAX_FRAME_LEN, "fits");
    assert_eq!(encode(&fits).unwrap().len(), MAX_FRAME_LEN);
    let too_big = padded_to_frame_len(MAX_FRAME_LEN + 1, "big");
    match encode(&too_big) {
        Err(JournalError::TooLarge { len, max }) => {
            assert_eq!((len, max), (MAX_FRAME_LEN + 1, MAX_FRAME_LEN))
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_frame_of_exactly_the_cap_decodes() {
    let record = padded_to_frame_len(MAX_FRAME_LEN, "edge");
    let report = decode(&encode(&record).unwrap()[..]).unwrap();
    assert_eq!(report.records, vec![record]);
}

#[test]
fn a_frame_one_byte_over_the_cap_is_malformed_even_though_it_parses() {
    let report = decode(&frame(&padded_to_frame_len(MAX_FRAME_LEN + 1, "over"))[..]).unwrap();
    assert!(report.records.is_empty());
    assert_eq!((report.malformed_lines, report.newer_version_lines), (1, 0));
}

#[test]
fn after_a_cut_anywhere_in_a_frame_the_next_record_is_recovered() {
    let length = frame(&rec(1)).len();
    for kept in [0, 1, 2, length / 2, length - 2, length - 1] {
        let report = after_a_cut(kept);
        assert_eq!(sessions(&report.records), ["s2"], "kept {kept}");
        assert_eq!(report.malformed_lines, 0, "kept {kept}");
        assert_eq!(report.torn_frames, usize::from(kept > 1), "kept {kept}");
        assert!(!report.truncated_last_line, "kept {kept}");
    }
}

#[test]
fn a_cut_that_keeps_all_but_the_newline_is_torn_and_never_accepted_as_a_record() {
    let length = frame(&rec(1)).len();
    let report = after_a_cut(length - 1);
    assert_eq!(sessions(&report.records), ["s2"]);
    assert_eq!(report.torn_frames, 1);
}

#[test]
fn a_lone_separator_left_by_a_one_byte_write_leaves_no_trace() {
    let report = after_a_cut(1);
    assert_eq!(sessions(&report.records), ["s2"]);
    assert_eq!(report.skipped_lines(), 0);
}

#[test]
fn without_the_separator_a_cut_glues_the_next_record_on_and_loses_both() {
    let first = bare_line(&rec(1));
    for kept in [1, first.len() / 2, first.len() - 1] {
        let mut bytes = first[..kept].to_vec();
        bytes.extend(bare_line(&rec(2)));
        let report = decode(&bytes[..]).unwrap();
        assert!(report.records.is_empty(), "kept {kept}");
        assert_eq!(report.malformed_lines, 1, "kept {kept}");
    }
}

#[test]
fn an_unterminated_tail_is_torn_flagged_and_never_parsed() {
    let mut bytes = frame(&rec(1));
    bytes.extend(b"\x1e{\"v\":1,\"ki");
    let report = decode(&bytes[..]).unwrap();
    assert_eq!(sessions(&report.records), ["s1"]);
    assert_eq!((report.torn_frames, report.malformed_lines), (1, 0));
    assert!(report.truncated_last_line);

    let mut complete_but_unterminated = vec![0x1e];
    complete_but_unterminated.extend(serde_json::to_vec(&rec(2)).unwrap());
    let report = decode(&complete_but_unterminated[..]).unwrap();
    assert!(report.records.is_empty());
    assert_eq!((report.torn_frames, report.malformed_lines), (1, 0));
    assert!(report.truncated_last_line);
}

#[test]
fn a_torn_frame_in_the_middle_is_counted_and_is_not_the_last_line() {
    let mut bytes = frame(&rec(1));
    bytes.extend(b"\x1e{\"v\":1");
    bytes.extend(frame(&rec(2)));
    let report = decode(&bytes[..]).unwrap();
    assert_eq!(sessions(&report.records), ["s1", "s2"]);
    assert_eq!(report.torn_frames, 1);
    assert!(!report.truncated_last_line);
}

#[test]
fn a_journal_that_ends_in_a_newline_is_not_truncated() {
    assert!(!decode(&frame(&rec(1))[..]).unwrap().truncated_last_line);
    assert!(!decode(&b"junk\n"[..]).unwrap().truncated_last_line);
}

#[test]
fn blank_segments_are_ignored() {
    let report = decode(&b"\n\n\x1e\x1e\n\x1e"[..]).unwrap();
    assert_eq!(report, ReadReport::default());
}

#[test]
fn bytes_that_are_not_utf8_or_not_json_are_malformed() {
    let report = decode(&b"\xff\xfe\xfd\nnot json\n[1,2,3]\n"[..]).unwrap();
    assert!(report.records.is_empty());
    assert_eq!(report.malformed_lines, 3);
}

#[test]
fn a_line_of_a_newer_version_is_counted_and_never_interpreted() {
    let report = decode(&edited_frame(&rec(1), |v| v["v"] = serde_json::json!(3))[..]).unwrap();
    assert!(report.records.is_empty());
    assert_eq!((report.newer_version_lines, report.malformed_lines), (1, 0));
    assert!(report.unsupported_version);
}

#[test]
fn a_version_of_zero_or_one_that_is_not_an_integer_is_malformed() {
    for text in [
        "{\"v\":0,\"kind\":\"shell_start\"}\n",
        "{\"v\":\"2\",\"kind\":\"shell_start\"}\n",
        "{\"v\":2.5}\n",
        "{\"v\":-1}\n",
    ] {
        let report = decode(text.as_bytes()).unwrap();
        assert_eq!(
            (report.newer_version_lines, report.malformed_lines),
            (0, 1),
            "{text}"
        );
        assert!(!report.unsupported_version, "{text}");
    }
}

#[test]
fn a_newer_version_line_longer_than_the_cap_is_still_a_newer_version_line() {
    let report = decode(&newer_version_line(MAX_FRAME_LEN + 4_000)[..]).unwrap();
    assert!(report.records.is_empty());
    assert_eq!(
        (
            report.newer_version_lines,
            report.malformed_lines,
            report.torn_frames
        ),
        (1, 0, 0)
    );
    assert!(report.unsupported_version);
}

#[test]
fn a_newer_version_frame_longer_than_the_cap_is_still_a_newer_version_line() {
    let mut bytes = vec![0x1e];
    bytes.extend(newer_version_line(MAX_FRAME_LEN * 2));
    let report = decode(&bytes[..]).unwrap();
    assert_eq!((report.newer_version_lines, report.malformed_lines), (1, 0));
    assert!(report.unsupported_version);
}

#[test]
fn a_newer_version_line_of_a_megabyte_does_not_hide_the_record_after_it() {
    let mut bytes = vec![0x1e];
    bytes.extend(newer_version_line(1 << 20));
    bytes.extend(frame(&rec(9)));
    let report = decode(&bytes[..]).unwrap();
    assert_eq!(sessions(&report.records), ["s9"]);
    assert_eq!(report.newer_version_lines, 1);
}

#[test]
fn a_version_prefix_must_be_a_canonical_number_followed_by_a_delimiter() {
    let long = |version: &str| format!("{{\"v\":{version}{}\n", "a".repeat(MAX_FRAME_LEN + 10)).into_bytes();
    for version in ["02,", "2x", "-2,", "+2,", "2.0,", " 2,"] {
        let report = decode(&long(version)[..]).unwrap();
        assert_eq!(
            (report.newer_version_lines, report.malformed_lines),
            (0, 1),
            "{version}"
        );
    }
    for version in ["3,", "3}", "10,", "18446744073709551615,"] {
        let report = decode(&long(version)[..]).unwrap();
        assert_eq!(
            (report.newer_version_lines, report.malformed_lines),
            (1, 0),
            "{version}"
        );
    }
}

#[test]
fn a_long_line_of_version_one_is_malformed_and_so_is_one_whose_version_is_not_first() {
    let long = format!("{{\"v\":1,\"pad\":\"{}\"}}\n", "a".repeat(MAX_FRAME_LEN));
    let anonymous = format!("{{\"pad\":\"{}\",\"v\":2}}\n", "a".repeat(MAX_FRAME_LEN));
    for text in [long, anonymous] {
        let report = decode(text.as_bytes()).unwrap();
        assert_eq!((report.newer_version_lines, report.malformed_lines), (0, 1));
    }
}

#[test]
fn a_line_with_a_kind_this_build_does_not_know_is_counted_apart_and_handed_over_raw() {
    let text = "{\"v\":1,\"kind\":\"future_kind\",\"agent\":\"claude\",\"session_id\":\"x\",\"wall_ts\":1,\"mono_ts\":2,\"boot\":\"b\"}\n";
    let report = decode(text.as_bytes()).unwrap();
    assert_eq!((report.unknown_kind_lines, report.malformed_lines), (1, 0));
    let mut seen = Vec::new();
    scan(text.as_bytes(), |raw, class| {
        seen.push((raw.to_vec(), matches!(class, Class::UnknownKind)));
    })
    .unwrap();
    assert_eq!(seen, vec![(text.trim_end().as_bytes().to_vec(), true)]);
}

#[test]
fn scan_hands_every_segment_to_the_callback_in_file_order() {
    let mut bytes = frame(&rec(1));
    bytes.extend(b"\x1e{broken\n");
    bytes.extend(frame(&rec(2)));
    bytes.extend(b"\x1e{\"v\":1");
    let mut classes = Vec::new();
    scan(&bytes[..], |_, class| {
        classes.push(match class {
            Class::Record(record) => record.session_id,
            Class::Malformed => "malformed".to_owned(),
            Class::Torn { tail: false } => "torn".to_owned(),
            Class::Torn { tail: true } => "torn tail".to_owned(),
            Class::NewerVersion => "newer".to_owned(),
            Class::UnknownKind => "unknown".to_owned(),
        });
    })
    .unwrap();
    assert_eq!(classes, ["s1", "malformed", "s2", "torn tail"]);
}

#[test]
fn a_json_array_is_malformed_whatever_it_holds() {
    for text in [
        "[2]\n",
        "[1]\n",
        "[1,\"session_start\",\"claude\",\"s\",null,null,1,2,\"b\",null,null]\n",
    ] {
        let report = decode(text.as_bytes()).unwrap();
        assert!(report.records.is_empty(), "{text}");
        assert_eq!(
            (report.malformed_lines, report.newer_version_lines),
            (1, 0),
            "{text}"
        );
        assert!(!report.unsupported_version, "{text}");
    }
}
