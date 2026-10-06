mod common;

use agentdust_bench::frame::{Class, EncodeError, Framing, MAX_FRAME_LEN, RS, decode, encode, scan};
use agentdust_bench::payload::record_with;
use agentdust_core::journal::Record;
use common::named;

fn rec(n: u64) -> Record {
    named(&format!("s{n}"), n)
}

fn sessions(records: &[Record]) -> Vec<&str> {
    records.iter().map(|record| record.session_id.as_str()).collect()
}

fn newer_version_line(pad: usize) -> Vec<u8> {
    format!(
        "{{\"v\":3,\"kind\":\"future\",\"pad\":\"{}\"}}\n",
        "a".repeat(pad)
    )
    .into_bytes()
}

fn after_a_cut(framing: Framing, kept: usize) -> agentdust_bench::frame::Decoded {
    let first = encode(&rec(1), framing).unwrap();
    let second = encode(&rec(2), framing).unwrap();
    let mut bytes = first[..kept.min(first.len())].to_vec();
    bytes.extend(&second);
    decode(&bytes[..]).unwrap()
}

#[test]
fn a_record_frame_is_rs_then_compact_json_then_lf() {
    let frame = encode(&rec(1), Framing::Record).unwrap();
    let json = serde_json::to_vec(&rec(1)).unwrap();
    assert_eq!(frame[0], RS);
    assert_eq!(&frame[1..frame.len() - 1], &json[..]);
    assert_eq!(frame[frame.len() - 1], b'\n');
}

#[test]
fn a_line_frame_is_compact_json_then_lf() {
    let frame = encode(&rec(1), Framing::Line).unwrap();
    let mut expected = serde_json::to_vec(&rec(1)).unwrap();
    expected.push(b'\n');
    assert_eq!(frame, expected);
}

#[test]
fn json_text_never_holds_a_raw_separator_so_a_frame_has_exactly_one_rs_and_one_lf() {
    let mut record = rec(1);
    record.session_id = "a\u{1e}b\nc\rd\u{1f}".to_owned();
    for framing in [Framing::Record, Framing::Line] {
        let frame = encode(&record, framing).unwrap();
        assert_eq!(
            frame.iter().filter(|byte| **byte == RS).count(),
            usize::from(framing == Framing::Record)
        );
        assert_eq!(frame.iter().filter(|byte| **byte == b'\n').count(), 1);
        let decoded = decode(&frame[..]).unwrap();
        assert_eq!(decoded.records, vec![record.clone()]);
    }
}

#[test]
fn both_framings_decode_back_to_the_records() {
    for framing in [Framing::Record, Framing::Line] {
        let bytes: Vec<u8> = (1..=3).flat_map(|n| encode(&rec(n), framing).unwrap()).collect();
        let decoded = decode(&bytes[..]).unwrap();
        assert_eq!(sessions(&decoded.records), ["s1", "s2", "s3"]);
        assert_eq!(
            (
                decoded.malformed,
                decoded.torn,
                decoded.newer_version,
                decoded.unknown_kind
            ),
            (0, 0, 0, 0)
        );
    }
}

#[test]
fn the_frame_cap_is_65536_bytes_including_the_delimiters() {
    assert_eq!(MAX_FRAME_LEN, 65_536);
    let fits = record_with(0, 0, MAX_FRAME_LEN - 1, 1_800_000_000_000, 1);
    assert_eq!(encode(&fits, Framing::Record).unwrap().len(), MAX_FRAME_LEN);
    let too_big = record_with(0, 0, MAX_FRAME_LEN, 1_800_000_000_000, 1);
    assert!(matches!(
        encode(&too_big, Framing::Record),
        Err(EncodeError::TooLarge { len, max }) if len == MAX_FRAME_LEN + 1 && max == MAX_FRAME_LEN
    ));
    assert_eq!(encode(&too_big, Framing::Line).unwrap().len(), MAX_FRAME_LEN);
}

#[test]
fn a_frame_of_exactly_the_cap_decodes() {
    let record = record_with(0, 0, MAX_FRAME_LEN - 1, 1_800_000_000_000, 1);
    let bytes = encode(&record, Framing::Record).unwrap();
    assert_eq!(decode(&bytes[..]).unwrap().records, vec![record]);
}

#[test]
fn after_a_cut_in_the_middle_of_a_frame_rs_framing_still_recovers_the_next_record() {
    let length = encode(&rec(1), Framing::Record).unwrap().len();
    for kept in [0, 1, 2, length / 2, length - 2, length - 1] {
        let decoded = after_a_cut(Framing::Record, kept);
        assert_eq!(sessions(&decoded.records), ["s2"], "kept {kept}");
        assert_eq!(decoded.malformed, 0, "kept {kept}");
        let expected_torn = usize::from(kept > 1);
        assert_eq!(decoded.torn, expected_torn, "kept {kept}");
    }
}

#[test]
fn a_cut_that_keeps_all_but_the_newline_is_torn_and_not_accepted_as_a_record() {
    let length = encode(&rec(1), Framing::Record).unwrap().len();
    let decoded = after_a_cut(Framing::Record, length - 1);
    assert_eq!(sessions(&decoded.records), ["s2"]);
    assert_eq!(decoded.torn, 1);
}

#[test]
fn after_a_cut_without_rs_framing_the_next_record_is_glued_on_and_lost() {
    let length = encode(&rec(1), Framing::Line).unwrap().len();
    let untouched = after_a_cut(Framing::Line, 0);
    assert_eq!(sessions(&untouched.records), ["s2"]);
    for kept in [1, length / 2, length - 1] {
        let decoded = after_a_cut(Framing::Line, kept);
        assert!(decoded.records.is_empty(), "kept {kept}");
        assert_eq!(decoded.malformed, 1, "kept {kept}");
    }
}

#[test]
fn an_unterminated_tail_is_torn_and_never_parsed() {
    let mut bytes = encode(&rec(1), Framing::Record).unwrap();
    bytes.extend(b"\x1e{\"v\":1,\"ki");
    let decoded = decode(&bytes[..]).unwrap();
    assert_eq!(sessions(&decoded.records), ["s1"]);
    assert_eq!((decoded.torn, decoded.malformed), (1, 0));
}

#[test]
fn blank_segments_are_ignored() {
    let decoded = decode(&b"\n\n\x1e\x1e\n"[..]).unwrap();
    assert!(decoded.records.is_empty());
    assert_eq!((decoded.malformed, decoded.torn), (0, 0));
}

#[test]
fn bytes_that_are_not_utf8_or_not_json_are_malformed() {
    let decoded = decode(&b"\xff\xfe\xfd\nnot json\n[1,2,3]\n"[..]).unwrap();
    assert!(decoded.records.is_empty());
    assert_eq!(decoded.malformed, 3);
}

#[test]
fn a_line_of_a_newer_version_is_counted_and_never_interpreted() {
    let mut line = serde_json::to_value(rec(1)).unwrap();
    line["v"] = serde_json::json!(3);
    let decoded = decode(format!("{line}\n").as_bytes()).unwrap();
    assert!(decoded.records.is_empty());
    assert_eq!((decoded.newer_version, decoded.malformed), (1, 0));
}

#[test]
fn a_version_of_zero_or_a_version_that_is_not_an_integer_is_malformed() {
    for text in [
        "{\"v\":0,\"kind\":\"shell_start\"}\n",
        "{\"v\":\"2\",\"kind\":\"shell_start\"}\n",
        "{\"v\":2.5}\n",
        "{\"v\":-1}\n",
    ] {
        let decoded = decode(text.as_bytes()).unwrap();
        assert_eq!((decoded.newer_version, decoded.malformed), (0, 1), "{text}");
    }
}

#[test]
fn a_newer_version_line_longer_than_the_cap_is_still_a_newer_version_line() {
    let decoded = decode(&newer_version_line(MAX_FRAME_LEN + 4_000)[..]).unwrap();
    assert!(decoded.records.is_empty());
    assert_eq!(
        (decoded.newer_version, decoded.malformed, decoded.torn),
        (1, 0, 0)
    );
}

#[test]
fn a_newer_version_line_of_a_megabyte_does_not_hide_the_record_after_it() {
    let mut bytes = b"\x1e".to_vec();
    bytes.extend(newer_version_line(1 << 20));
    bytes.extend(encode(&rec(9), Framing::Record).unwrap());
    let decoded = decode(&bytes[..]).unwrap();
    assert_eq!(sessions(&decoded.records), ["s9"]);
    assert_eq!(decoded.newer_version, 1);
}

#[test]
fn a_long_line_of_version_one_is_malformed_and_a_long_line_without_a_version_is_too() {
    let long = format!("{{\"v\":1,\"pad\":\"{}\"}}\n", "a".repeat(MAX_FRAME_LEN));
    let anonymous = format!("{{\"pad\":\"{}\",\"v\":2}}\n", "a".repeat(MAX_FRAME_LEN));
    for text in [long, anonymous] {
        let decoded = decode(text.as_bytes()).unwrap();
        assert_eq!((decoded.newer_version, decoded.malformed), (0, 1));
    }
}

#[test]
fn a_line_with_a_kind_this_build_does_not_know_is_counted_apart_and_handed_over_raw() {
    let text = "{\"v\":1,\"kind\":\"future_kind\",\"agent\":\"claude\",\"session_id\":\"x\",\"wall_ts\":1,\"mono_ts\":2,\"boot\":\"b\"}\n";
    let decoded = decode(text.as_bytes()).unwrap();
    assert_eq!((decoded.unknown_kind, decoded.malformed), (1, 0));
    let mut seen = Vec::new();
    scan(text.as_bytes(), |raw, class| {
        seen.push((raw.to_vec(), matches!(class, Class::UnknownKind)));
    })
    .unwrap();
    assert_eq!(seen, vec![(text.trim_end().as_bytes().to_vec(), true)]);
}

#[test]
fn scan_hands_every_segment_to_the_callback_in_file_order() {
    let mut bytes = encode(&rec(1), Framing::Record).unwrap();
    bytes.extend(b"\x1e{broken\n");
    bytes.extend(encode(&rec(2), Framing::Record).unwrap());
    bytes.extend(b"\x1e{\"v\":1");
    let mut classes = Vec::new();
    scan(&bytes[..], |_, class| {
        classes.push(match class {
            Class::Record(record) => record.session_id,
            Class::Malformed => "malformed".to_owned(),
            Class::Torn => "torn".to_owned(),
            Class::NewerVersion => "newer".to_owned(),
            Class::UnknownKind => "unknown".to_owned(),
        });
    })
    .unwrap();
    assert_eq!(classes, ["s1", "malformed", "s2", "torn"]);
}
