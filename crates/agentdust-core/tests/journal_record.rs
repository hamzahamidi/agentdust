use agentdust_core::journal::{Agent, CwdKey, ExeBase, FieldError, Kind, Record, SCHEMA_VERSION};
use serde_json::json;

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn minimal() -> Record {
    Record {
        v: SCHEMA_VERSION,
        kind: Kind::SessionStart,
        agent: Agent::Claude,
        session_id: "s1".to_owned(),
        subagent_id: None,
        tool_use_id: None,
        wall_ts: 1_800_000_000_123,
        mono_ts: 42,
        boot: "b".to_owned(),
        cwd_key: None,
        exe_base: None,
    }
}

fn full() -> Record {
    Record {
        kind: Kind::ShellStart,
        subagent_id: Some("agent-7".to_owned()),
        tool_use_id: Some("toolu_1".to_owned()),
        cwd_key: Some(CwdKey::try_from(KEY).unwrap()),
        exe_base: Some(ExeBase::try_from("node").unwrap()),
        ..minimal()
    }
}

#[test]
fn the_schema_version_is_one() {
    assert_eq!(SCHEMA_VERSION, 1);
}

#[test]
fn a_minimal_record_serialises_to_the_spec_field_names_and_nothing_else() {
    assert_eq!(
        serde_json::to_string(&minimal()).unwrap(),
        r#"{"v":1,"kind":"session_start","agent":"claude","session_id":"s1","wall_ts":1800000000123,"mono_ts":42,"boot":"b"}"#
    );
}

#[test]
fn a_full_record_serialises_in_spec_order() {
    assert_eq!(
        serde_json::to_string(&full()).unwrap(),
        format!(
            r#"{{"v":1,"kind":"shell_start","agent":"claude","session_id":"s1","subagent_id":"agent-7","tool_use_id":"toolu_1","wall_ts":1800000000123,"mono_ts":42,"boot":"b","cwd_key":"{KEY}","exe_base":"node"}}"#
        )
    );
}

#[test]
fn a_full_record_round_trips() {
    let text = serde_json::to_string(&full()).unwrap();
    assert_eq!(serde_json::from_str::<Record>(&text).unwrap(), full());
}

#[test]
fn the_old_timestamp_names_are_not_read() {
    let old = json!({
        "v": 1, "kind": "session_start", "agent": "claude", "session_id": "s1",
        "wall_ts_ms": 1, "mono_ns": 2, "boot": "b"
    });
    assert!(serde_json::from_value::<Record>(old).is_err());
}

#[test]
fn the_timestamps_are_unsigned_64_bit_numbers() {
    let mut value = serde_json::to_value(minimal()).unwrap();
    value["wall_ts"] = json!(u64::MAX);
    value["mono_ts"] = json!(u64::MAX);
    let record: Record = serde_json::from_value(value).unwrap();
    assert_eq!((record.wall_ts, record.mono_ts), (u64::MAX, u64::MAX));
    for field in ["wall_ts", "mono_ts"] {
        for bad in [json!(-1), json!(1.5), json!("1")] {
            let mut value = serde_json::to_value(minimal()).unwrap();
            value[field] = bad;
            assert!(serde_json::from_value::<Record>(value).is_err(), "{field}");
        }
    }
}

#[test]
fn an_executable_name_of_64_bytes_is_accepted_and_65_is_not() {
    assert!(ExeBase::try_from("n".repeat(64)).is_ok());
    assert_eq!(
        ExeBase::try_from("n".repeat(65)).unwrap_err(),
        FieldError::TooLong { len: 65, max: 64 }
    );
}

#[test]
fn the_executable_name_limit_counts_bytes_not_characters() {
    assert!(ExeBase::try_from("é".repeat(32)).is_ok());
    assert_eq!(
        ExeBase::try_from("é".repeat(33)).unwrap_err(),
        FieldError::TooLong { len: 66, max: 64 }
    );
}

#[test]
fn ordinary_executable_names_are_accepted() {
    for name in ["node", "Code Helper (Renderer)", "python3.13", "café", "x"] {
        assert_eq!(ExeBase::try_from(name).unwrap().as_str(), name);
    }
}

#[test]
fn control_characters_are_rejected_in_an_executable_name() {
    for name in [
        "a\0b",
        "a\nb",
        "a\rb",
        "a\tb",
        "\u{1b}[31mred",
        "a\u{7f}",
        "a\u{85}",
        "a\u{9b}",
        "\u{202e}gnp.exe",
        "a\u{200f}",
        "a\u{2066}b",
        "a\u{2069}",
    ] {
        assert_eq!(
            ExeBase::try_from(name).unwrap_err(),
            FieldError::Control,
            "{name:?}"
        );
    }
}

#[test]
fn a_record_with_an_invalid_executable_name_does_not_deserialise() {
    for bad in ["bad\u{0}", "line\nbreak", "\u{202e}x"] {
        let mut value = serde_json::to_value(minimal()).unwrap();
        value["exe_base"] = json!(bad);
        assert!(serde_json::from_value::<Record>(value).is_err(), "{bad:?}");
    }
    let mut value = serde_json::to_value(minimal()).unwrap();
    value["exe_base"] = json!("n".repeat(65));
    assert!(serde_json::from_value::<Record>(value).is_err());
}

#[test]
fn an_executable_name_that_is_not_a_string_does_not_deserialise() {
    for bad in [json!(7), json!(["node"]), json!({"a": 1})] {
        let mut value = serde_json::to_value(minimal()).unwrap();
        value["exe_base"] = bad;
        assert!(serde_json::from_value::<Record>(value).is_err());
    }
}

#[test]
fn a_null_optional_field_reads_as_absent() {
    let mut value = serde_json::to_value(minimal()).unwrap();
    value["exe_base"] = json!(null);
    value["cwd_key"] = json!(null);
    assert_eq!(serde_json::from_value::<Record>(value).unwrap(), minimal());
}

#[test]
fn a_working_directory_key_is_lowercase_hex_up_to_64_characters() {
    assert!(CwdKey::try_from(KEY).is_ok());
    assert!(CwdKey::try_from("a").is_ok());
    assert!(CwdKey::try_from("0123456789abcdef").is_ok());
    assert_eq!(CwdKey::try_from(KEY).unwrap().as_str(), KEY);
}

#[test]
fn a_working_directory_key_outside_that_form_is_rejected() {
    assert_eq!(CwdKey::try_from("").unwrap_err(), FieldError::Empty);
    for bad in [
        "ABCDEF",
        "abcdeG",
        "ab cd",
        "0x12",
        "ab\n",
        "\u{661}\u{662}",
        "-1",
    ] {
        assert_eq!(CwdKey::try_from(bad).unwrap_err(), FieldError::NotHex, "{bad:?}");
    }
    assert_eq!(
        CwdKey::try_from("a".repeat(65)).unwrap_err(),
        FieldError::TooLong { len: 65, max: 64 }
    );
}

#[test]
fn a_record_with_an_invalid_working_directory_key_does_not_deserialise() {
    for bad in ["", "ZZ", "A1", "a".repeat(65).as_str()] {
        let mut value = serde_json::to_value(minimal()).unwrap();
        value["cwd_key"] = json!(bad);
        assert!(serde_json::from_value::<Record>(value).is_err(), "{bad:?}");
    }
}
