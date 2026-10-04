use std::io::{self, Cursor, Read};

use agentdust_agents::claude::{EventError, MAX_ID_LEN, parse_event};

const EVENT: &str = r#"{"session_id":"s1","hook_event_name":"SessionStart"}"#;

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

fn counting(head: &str, filler: u8, len: u64, tail: &str) -> Counting<impl Read> {
    Counting {
        inner: Cursor::new(head.to_owned())
            .chain(io::repeat(filler).take(len))
            .chain(Cursor::new(tail.to_owned())),
        served: 0,
    }
}

#[test]
fn a_10_mb_top_level_string_is_rejected_after_a_bounded_read() {
    let mut reader = counting("\"", b's', 10_000_000, "\"");
    let result = parse_event(&mut reader);
    assert!(reader.served <= 8 * MAX_ID_LEN, "read {} bytes", reader.served);
    assert!(matches!(result, Err(EventError::FieldTooLong)));
}

#[test]
fn a_huge_run_of_whitespace_before_the_object_is_rejected_after_a_bounded_read() {
    let mut reader = counting("", b' ', 10_000_000, EVENT);
    let result = parse_event(&mut reader);
    assert!(reader.served <= 8 * MAX_ID_LEN, "read {} bytes", reader.served);
    assert!(matches!(result, Err(EventError::FieldTooLong)));
}

#[test]
fn whitespace_before_the_object_is_accepted_within_the_budget() {
    let input = format!("{}{EVENT}", " \n\t\r".repeat(250));
    let event = parse_event(input.as_bytes()).unwrap();
    assert_eq!(event.session_id, "s1");
}

#[test]
fn a_short_top_level_string_is_a_json_error() {
    let result = parse_event(&b"\"short\""[..]);
    assert!(matches!(result, Err(EventError::Json(_))), "{result:?}");
}

#[test]
fn other_top_level_values_are_json_errors() {
    for input in [
        "",
        " ",
        "12",
        "-1.5e3",
        "true",
        "null",
        "[]",
        "[1,{\"a\":2}]",
        "}",
        "\u{feff}{}",
    ] {
        let result = parse_event(input.as_bytes());
        assert!(
            matches!(result, Err(EventError::Json(_))),
            "{input:?}: {result:?}"
        );
    }
}

#[test]
fn the_object_itself_is_unaffected_by_the_leading_budget() {
    let padded = format!(
        r#"{{"x":"{}","session_id":"s1","hook_event_name":"SessionEnd"}}"#,
        "y".repeat(100_000)
    );
    let event = parse_event(padded.as_bytes()).unwrap();
    assert_eq!(event.hook_event_name, "SessionEnd");
}

const LEADING_BUDGET: usize = 1_600;

#[test]
fn whitespace_before_the_object_is_accepted_up_to_the_budget_and_not_beyond() {
    let at = format!("{}{EVENT}", " ".repeat(LEADING_BUDGET - 1));
    assert!(parse_event(at.as_bytes()).is_ok());
    let over = format!("{}{EVENT}", " ".repeat(LEADING_BUDGET));
    let result = parse_event(over.as_bytes());
    assert!(matches!(result, Err(EventError::FieldTooLong)), "{result:?}");
}

#[test]
fn a_top_level_string_is_rejected_as_too_long_from_the_budget_and_a_json_error_below_it() {
    let below = format!("\"{}\"", "s".repeat(LEADING_BUDGET - 3));
    assert!(matches!(parse_event(below.as_bytes()), Err(EventError::Json(_))));
    let over = format!("\"{}\"", "s".repeat(LEADING_BUDGET));
    assert!(matches!(
        parse_event(over.as_bytes()),
        Err(EventError::FieldTooLong)
    ));
}
