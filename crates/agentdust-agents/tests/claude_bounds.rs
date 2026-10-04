use std::io::{self, BufReader, Cursor, Read};

use agentdust_agents::claude::{EventError, MAX_CWD_LEN, MAX_ID_LEN, parse_event};

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

const HOOK_BUFFER: usize = 8 * 1024;

#[test]
fn through_the_hooks_buffered_reader_the_retained_bytes_stay_bounded() {
    let mut buffered = BufReader::new(long_string_field("session_id", 50_000_000));
    let result = parse_event(&mut buffered);
    assert!(matches!(result, Err(EventError::FieldTooLong)), "{result:?}");
    let served = buffered.get_ref().served;
    assert!(
        served <= 8 * MAX_ID_LEN + HOOK_BUFFER,
        "read {served} bytes from the source"
    );
}

#[test]
fn a_working_directory_of_exactly_the_limit_is_accepted() {
    let cwd = format!("/{}", "d".repeat(MAX_CWD_LEN - 1));
    let input = format!(r#"{{"session_id":"s","hook_event_name":"SessionStart","cwd":"{cwd}"}}"#);
    assert_eq!(parse_event(input.as_bytes()).unwrap().cwd, Some(cwd));
}

#[test]
fn a_working_directory_over_the_limit_is_rejected() {
    let cwd = format!("/{}", "d".repeat(MAX_CWD_LEN));
    let input = format!(r#"{{"session_id":"s","hook_event_name":"SessionStart","cwd":"{cwd}"}}"#);
    assert!(matches!(
        parse_event(input.as_bytes()),
        Err(EventError::FieldTooLong)
    ));
}

#[test]
fn the_working_directory_limit_counts_bytes_not_characters() {
    let accepted = format!("/a{}", "\u{e9}".repeat(MAX_CWD_LEN / 2 - 1));
    assert_eq!(accepted.len(), MAX_CWD_LEN);
    let input = format!(r#"{{"session_id":"s","hook_event_name":"SessionStart","cwd":"{accepted}"}}"#);
    assert_eq!(parse_event(input.as_bytes()).unwrap().cwd, Some(accepted));
    let rejected = format!("/a{}", "\u{e9}".repeat(MAX_CWD_LEN / 2));
    let input = format!(r#"{{"session_id":"s","hook_event_name":"SessionStart","cwd":"{rejected}"}}"#);
    assert!(matches!(
        parse_event(input.as_bytes()),
        Err(EventError::FieldTooLong)
    ));
}

#[test]
fn a_50_mb_working_directory_is_rejected_after_a_bounded_read() {
    let mut reader = long_string_field("cwd", 50_000_000);
    let result = parse_event(&mut reader);
    assert!(matches!(result, Err(EventError::FieldTooLong)), "{result:?}");
    assert!(reader.served <= 8 * MAX_CWD_LEN, "read {} bytes", reader.served);
}

#[test]
fn escapes_in_a_working_directory_count_by_decoded_length() {
    let escape = format!("{}u0064", char::from(0x5c));
    let escaped = |count: usize| {
        let cwd = escape.repeat(count);
        format!(r#"{{"session_id":"s","hook_event_name":"SessionStart","cwd":"{cwd}"}}"#)
    };
    let event = parse_event(escaped(MAX_CWD_LEN).as_bytes()).unwrap();
    assert_eq!(event.cwd, Some("d".repeat(MAX_CWD_LEN)));
    assert!(matches!(
        parse_event(escaped(MAX_CWD_LEN + 1).as_bytes()),
        Err(EventError::FieldTooLong)
    ));
}

#[test]
fn the_larger_working_directory_limit_does_not_loosen_the_identifier_limit() {
    let long = "i".repeat(MAX_ID_LEN + 1);
    for field in FIELDS {
        let others: String = ["session_id", "hook_event_name"]
            .into_iter()
            .filter(|name| *name != field)
            .map(|name| format!(r#","{name}":"x""#))
            .collect();
        let input = format!(r#"{{"cwd":"/a","{field}":"{long}"{others}}}"#);
        assert!(
            matches!(parse_event(input.as_bytes()), Err(EventError::FieldTooLong)),
            "{field}"
        );
    }
}

#[test]
fn a_working_directory_after_a_skipped_field_is_still_read() {
    let mut reader = counting(
        r#"{"session_id":"s","hook_event_name":"PostToolUse","tool_response":{"stdout":""#.into(),
        5_000_000,
        r#""},"cwd":"/Users/dev/project"}"#.into(),
    );
    let event = parse_event(&mut reader).unwrap();
    assert_eq!(event.cwd.as_deref(), Some("/Users/dev/project"));
}
