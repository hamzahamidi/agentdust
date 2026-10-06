mod journal_support;

use agentdust_core::journal::{ReadReport, Record, decode};
use journal_support::{edited_frame, frame, frames, join, record, sessions};
use serde_json::json;

fn base() -> Record {
    record("a", "b", 1, 1)
}

fn decoded(bytes: &[u8]) -> ReadReport {
    decode(bytes).unwrap()
}

#[test]
fn an_empty_journal_is_an_empty_report() {
    assert_eq!(decoded(b""), ReadReport::default());
}

#[test]
fn valid_frames_decode_in_file_order_with_clean_counters() {
    let (a, b) = (record("a", "b", 1, 9), record("b", "b", 2, 1));
    let report = decoded(&frames(&[a.clone(), b.clone()]));
    assert_eq!(report.records, vec![a, b]);
    assert_eq!(
        report,
        ReadReport {
            records: report.records.clone(),
            ..ReadReport::default()
        }
    );
}

#[test]
fn a_frame_that_is_not_utf8_is_counted_and_the_read_goes_on() {
    let (a, b) = (record("a", "b", 1, 1), record("b", "b", 2, 2));
    let report = decoded(&join(&[&frame(&a), b"\x1e\xff\xfe\n", &frame(&b)]));
    assert_eq!(sessions(&report.records), ["a", "b"]);
    assert_eq!(report.malformed_lines, 1);
}

#[test]
fn invalid_utf8_inside_a_json_string_is_malformed() {
    let bad = b"\x1e{\"v\":1,\"kind\":\"session_start\",\"agent\":\"claude\",\"session_id\":\"\xc3\x28\",\"wall_ts\":1,\"mono_ts\":2,\"boot\":\"b\"}\n";
    let report = decoded(&join(&[bad, &frame(&base())]));
    assert_eq!(sessions(&report.records), ["a"]);
    assert_eq!(report.malformed_lines, 1);
}

#[test]
fn garbage_and_incomplete_objects_are_malformed() {
    let lines: [&[u8]; 9] = [
        b"not json\n",
        b"{}\n",
        b"[]\n",
        b"null\n",
        b"{\"v\":1}\n",
        b"{\"v\":1,\"kind\":\"shell_start\"}\n",
        b"{\"v\":1,\"kind\":\"shell_start\",\"agent\":\"claude\",\"session_id\":\"s\"}\n",
        b"\0\0\0\n",
        b"{\"v\":1,\"kind\":\"shell_start\",\"agent\":\"claude\",\"session_id\":\"s\",\"wall_ts\":1,\"mono_ts\":2}\n",
    ];
    for bad in lines {
        let report = decoded(bad);
        assert_eq!(report.malformed_lines, 1, "{bad:?}");
        assert!(report.records.is_empty(), "{bad:?}");
        assert_eq!(
            report.newer_version_lines + report.unknown_kind_lines + report.torn_frames,
            0,
            "{bad:?}"
        );
        assert!(!report.unsupported_version, "{bad:?}");
    }
}

#[test]
fn wrong_field_types_and_unknown_agents_are_malformed() {
    let edits: [(&str, serde_json::Value); 7] = [
        ("session_id", json!(5)),
        ("agent", json!("gemini")),
        ("wall_ts", json!(-1)),
        ("mono_ts", json!("2")),
        ("boot", json!(null)),
        ("tool_use_id", json!(7)),
        ("subagent_id", json!(["x"])),
    ];
    for (field, value) in edits {
        let report = decoded(&edited_frame(&base(), |v| v[field] = value));
        assert_eq!(report.malformed_lines, 1, "{field}");
        assert!(report.records.is_empty(), "{field}");
    }
}

#[test]
fn a_repeated_field_name_is_malformed() {
    let text = br#"{"v":1,"kind":"session_start","agent":"claude","session_id":"a","session_id":"b","wall_ts":1,"mono_ts":2,"boot":"b"}"#;
    let report = decoded(&join(&[b"\x1e", text, b"\n"]));
    assert_eq!(report.malformed_lines, 1);
    assert!(report.records.is_empty());
}

#[test]
fn an_invalid_executable_name_or_directory_key_makes_the_line_malformed() {
    let bad_fields = [
        ("exe_base", json!("a\u{1b}[0m")),
        ("exe_base", json!("n".repeat(65))),
        ("cwd_key", json!("NOTHEX")),
        ("cwd_key", json!("a".repeat(65))),
        ("cwd_key", json!("")),
    ];
    for (field, value) in bad_fields {
        let report = decoded(&edited_frame(&base(), |v| v[field] = value.clone()));
        assert_eq!(report.malformed_lines, 1, "{field} {value}");
        assert!(report.records.is_empty());
    }
}

#[test]
fn the_optional_fields_round_trip_through_a_frame() {
    let key = "0123456789abcdef";
    let full = edited_frame(&base(), |v| {
        v["cwd_key"] = json!(key);
        v["exe_base"] = json!("node");
        v["subagent_id"] = json!("agent-7");
        v["tool_use_id"] = json!("toolu_1");
    });
    let report = decoded(&full);
    let record = &report.records[0];
    assert_eq!(record.cwd_key.as_ref().unwrap().as_str(), key);
    assert_eq!(record.exe_base.as_ref().unwrap().as_str(), "node");
    assert_eq!(record.subagent_id.as_deref(), Some("agent-7"));
    assert_eq!(record.tool_use_id.as_deref(), Some("toolu_1"));
}

#[test]
fn a_line_from_a_newer_schema_version_never_yields_a_record() {
    let shaped_like_v1 = edited_frame(&record("future", "b", 1, 1), |v| v["v"] = json!(3));
    let unrelated: &[u8] = b"\x1e{\"v\":3,\"payload\":[1,2,3]}\n";
    let max: &[u8] = b"\x1e{\"v\":18446744073709551615}\n";
    let unknown_kind_too = edited_frame(&record("future", "b", 1, 1), |v| {
        v["v"] = json!(4);
        v["kind"] = json!("hologram");
    });
    let report = decoded(&join(&[&shaped_like_v1, unrelated, max, &unknown_kind_too]));
    assert!(report.records.is_empty());
    assert_eq!(report.newer_version_lines, 4);
    assert_eq!((report.malformed_lines, report.unknown_kind_lines), (0, 0));
    assert!(report.unsupported_version);
}

#[test]
fn current_lines_still_read_beside_newer_ones_and_the_flag_tells_the_consumer() {
    let (a, b) = (record("a", "b", 1, 1), record("b", "b", 3, 3));
    let future = edited_frame(&record("future", "b", 2, 2), |v| v["v"] = json!(3));
    let report = decoded(&join(&[&frame(&a), &future, &frame(&b)]));
    assert_eq!(sessions(&report.records), ["a", "b"]);
    assert_eq!(report.newer_version_lines, 1);
    assert!(report.unsupported_version);
}

#[test]
fn versions_that_are_not_newer_are_malformed_and_do_not_raise_the_flag() {
    let versions = [
        json!(0),
        json!(-1),
        json!("2"),
        json!(1.5),
        json!(2.0),
        json!(null),
        json!(true),
        json!([2]),
    ];
    for version in versions {
        let report = decoded(&edited_frame(&base(), |v| v["v"] = version.clone()));
        assert_eq!(report.malformed_lines, 1, "{version}");
        assert_eq!(report.newer_version_lines, 0, "{version}");
        assert!(!report.unsupported_version, "{version}");
        assert!(report.records.is_empty(), "{version}");
    }
    let missing = decoded(&edited_frame(&base(), |v| {
        v.as_object_mut().unwrap().remove("v");
    }));
    assert_eq!((missing.malformed_lines, missing.records.len()), (1, 0));
    let too_big = decoded(b"\x1e{\"v\":18446744073709551616}\n");
    assert_eq!((too_big.malformed_lines, too_big.newer_version_lines), (1, 0));
}

#[test]
fn the_flag_stays_down_when_only_malformed_and_unknown_lines_are_present() {
    let unknown = edited_frame(&base(), |v| v["kind"] = json!("hologram"));
    let report = decoded(&join(&[b"\x1ejunk\n", &unknown]));
    assert!(!report.unsupported_version);
    assert_eq!(report.newer_version_lines, 0);
}

#[test]
fn an_unknown_kind_is_skipped_and_counted_apart_from_malformed_lines() {
    let unknown = edited_frame(&base(), |v| v["kind"] = json!("hologram"));
    let bare: &[u8] = b"\x1e{\"v\":1,\"kind\":\"hologram\"}\n";
    let report = decoded(&join(&[&unknown, bare, &frame(&record("kept", "b", 2, 2))]));
    assert_eq!(sessions(&report.records), ["kept"]);
    assert_eq!(report.unknown_kind_lines, 2);
    assert_eq!((report.malformed_lines, report.newer_version_lines), (0, 0));
    assert!(!report.unsupported_version);
}

#[test]
fn a_kind_that_is_not_a_string_is_malformed() {
    let report = decoded(&edited_frame(&base(), |v| v["kind"] = json!(7)));
    assert_eq!((report.malformed_lines, report.unknown_kind_lines), (1, 0));
}

#[test]
fn every_known_kind_and_agent_is_accepted() {
    let kinds = [
        "session_start",
        "session_end",
        "shell_start",
        "shell_end",
        "sample",
        "server_start",
    ];
    for kind in kinds {
        let report = decoded(&edited_frame(&base(), |v| v["kind"] = json!(kind)));
        assert_eq!(report.records.len(), 1, "{kind}");
    }
    for agent in ["claude", "codex", "cursor"] {
        let report = decoded(&edited_frame(&base(), |v| v["agent"] = json!(agent)));
        assert_eq!(report.records.len(), 1, "{agent}");
    }
}

#[test]
fn a_frame_larger_than_the_cap_does_not_hide_the_records_around_it() {
    let huge = vec![b'x'; 5 * 1024 * 1024];
    let (before, after) = (record("before", "b", 1, 1), record("after", "b", 2, 2));
    let report = decoded(&join(&[&frame(&before), b"\x1e", &huge, b"\n", &frame(&after)]));
    assert_eq!(sessions(&report.records), ["before", "after"]);
    assert_eq!(report.malformed_lines, 1);
    assert!(!report.truncated_last_line);
}

#[test]
fn an_oversized_unterminated_tail_is_torn_and_flagged_and_not_malformed() {
    let tail = vec![b'y'; 200 * 1024];
    let report = decoded(&join(&[&frame(&base()), b"\x1e", &tail]));
    assert_eq!(sessions(&report.records), ["a"]);
    assert_eq!((report.torn_frames, report.malformed_lines), (1, 0));
    assert!(report.truncated_last_line);
}

#[test]
fn skipped_lines_adds_every_kind_of_line_that_did_not_become_a_record() {
    let report = ReadReport {
        malformed_lines: 1,
        torn_frames: 2,
        newer_version_lines: 4,
        unknown_kind_lines: 8,
        ..ReadReport::default()
    };
    assert_eq!(report.skipped_lines(), 15);
    assert_eq!(ReadReport::default().skipped_lines(), 0);
}
