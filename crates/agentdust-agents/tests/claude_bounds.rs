use std::io::{self, Cursor, Read};

use agentdust_agents::claude::{EventError, MAX_ID_LEN, parse_event};

const FIELDS: [&str; 5] = [
    "session_id",
    "hook_event_name",
    "tool_name",
    "tool_use_id",
    "agent_id",
];

struct Counting<R> {
    inner: R,
    served: usize,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.served += n;
        Ok(n)
    }
}

fn counting(head: String, filler: u64, tail: String) -> Counting<impl Read> {
    Counting {
        inner: Cursor::new(head)
            .chain(io::repeat(b's').take(filler))
            .chain(Cursor::new(tail)),
        served: 0,
    }
}

fn long_string_field(field: &str, len: u64) -> Counting<impl Read> {
    let required: String = ["session_id", "hook_event_name"]
        .into_iter()
        .filter(|name| *name != field)
        .map(|name| format!(r#","{name}":"x""#))
        .collect();
    counting(format!(r#"{{"{field}":""#), len, format!(r#""{required}}}"#))
}

#[test]
fn a_50_mb_session_id_is_rejected_after_a_bounded_read() {
    let mut reader = long_string_field("session_id", 50_000_000);
    let result = parse_event(&mut reader);
    assert!(matches!(result, Err(EventError::FieldTooLong)), "{result:?}");
    assert!(reader.served <= 8 * MAX_ID_LEN, "read {} bytes", reader.served);
}

#[test]
fn every_kept_string_field_is_bounded_before_it_is_stored() {
    for field in FIELDS {
        let mut reader = long_string_field(field, 10_000_000);
        let result = parse_event(&mut reader);
        assert!(
            matches!(result, Err(EventError::FieldTooLong)),
            "{field}: {result:?}"
        );
        assert!(
            reader.served <= 8 * MAX_ID_LEN,
            "{field}: read {} bytes",
            reader.served
        );
    }
}

#[test]
fn an_oversized_field_name_is_rejected_after_a_bounded_read() {
    let mut reader = counting(
        r#"{"session_id":"s","hook_event_name":"Stop",""#.into(),
        10_000_000,
        r#"":1}"#.into(),
    );
    let result = parse_event(&mut reader);
    assert!(matches!(result, Err(EventError::FieldTooLong)), "{result:?}");
    assert!(reader.served <= 8 * MAX_ID_LEN, "read {} bytes", reader.served);
}

#[test]
fn a_skipped_field_of_any_size_is_still_streamed_through() {
    let mut reader = counting(
        r#"{"session_id":"s","hook_event_name":"PostToolUse","tool_response":{"stdout":""#.into(),
        20_000_000,
        r#""},"tool_use_id":"toolu_1"}"#.into(),
    );
    let event = parse_event(&mut reader).unwrap();
    assert_eq!(event.tool_use_id.as_deref(), Some("toolu_1"));
    assert!(reader.served > 20_000_000);
}

#[test]
fn an_identifier_of_exactly_the_limit_is_accepted() {
    let id = "i".repeat(MAX_ID_LEN);
    let input = format!(r#"{{"session_id":"{id}","hook_event_name":"SessionStart"}}"#);
    assert_eq!(parse_event(input.as_bytes()).unwrap().session_id, id);
}

#[test]
fn escapes_count_by_decoded_length_not_source_length() {
    let escape = format!("{}u0073", char::from(0x5c));
    let escaped = |count: usize| {
        let id = escape.repeat(count);
        format!(r#"{{"session_id":"{id}","hook_event_name":"SessionStart"}}"#)
    };
    let event = parse_event(escaped(MAX_ID_LEN).as_bytes()).unwrap();
    assert_eq!(event.session_id, "s".repeat(MAX_ID_LEN));
    assert!(matches!(
        parse_event(escaped(MAX_ID_LEN + 1).as_bytes()),
        Err(EventError::FieldTooLong)
    ));
}

#[test]
fn whitespace_around_kept_fields_is_accepted() {
    let input = "{\n  \"session_id\"  :\t \"s1\" ,\r\n  \"hook_event_name\" : \"Stop\"\n}";
    assert_eq!(parse_event(input.as_bytes()).unwrap().session_id, "s1");
}
