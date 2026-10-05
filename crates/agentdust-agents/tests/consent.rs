use std::io::Cursor;

use agentdust_agents::claude_setup::ask_yes_no;

fn answer(input: &str) -> (bool, String) {
    let mut output = Vec::new();
    let said = ask_yes_no(
        &mut Cursor::new(input.as_bytes().to_vec()),
        &mut output,
        "Apply? [y/N] ",
    )
    .unwrap();
    (said, String::from_utf8(output).unwrap())
}

#[test]
fn only_y_and_yes_in_any_case_approve() {
    for input in ["y\n", "Y\n", "yes\n", "YES\n", "Yes\r\n", "  y  \n", "y"] {
        assert!(answer(input).0, "{input:?}");
    }
}

#[test]
fn anything_else_declines() {
    for input in [
        "", "\n", "n\n", "N\n", "no\n", "yep\n", "y es\n", "yy\n", "ok\n", "1\n", "\u{0}y\n",
    ] {
        assert!(!answer(input).0, "{input:?}");
    }
}

#[test]
fn the_prompt_is_written_and_only_one_line_is_read() {
    let (_, output) = answer("n\n");
    assert_eq!(output, "Apply? [y/N] ");
    let mut input = Cursor::new(b"y\nn\n".to_vec());
    let mut sink = Vec::new();
    assert!(ask_yes_no(&mut input, &mut sink, "?").unwrap());
    assert_eq!(input.position(), 2);
}
