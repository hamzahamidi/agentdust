use agentdust_core::procargs::{ParseError, script_argument};

fn buffer(argc: i32, exec_path: &[u8], strings: &[&[u8]]) -> Vec<u8> {
    let mut buf = argc.to_ne_bytes().to_vec();
    buf.extend_from_slice(exec_path);
    buf.push(0);
    buf.extend([0, 0, 0]);
    for s in strings {
        buf.extend_from_slice(s);
        buf.push(0);
    }
    buf
}

fn script_of(args: &[&[u8]]) -> Option<Vec<u8>> {
    let mut strings: Vec<&[u8]> = args.to_vec();
    strings.push(b"HOME=/Users/user-sentinel");
    strings.push(b"AGENTDUST_SESSION=tag-sentinel");
    let buf = buffer(args.len() as i32, b"/usr/local/bin/node", &strings);
    script_argument(&buf).unwrap().map(<[u8]>::to_vec)
}

#[test]
fn the_first_argument_after_the_program_name_is_the_script() {
    assert_eq!(script_of(&[b"node", b"/a/cli.js"]), Some(b"/a/cli.js".to_vec()));
    assert_eq!(
        script_of(&[b"node", b"/a/cli.js", b"/b/other.js", b"--flag"]),
        Some(b"/a/cli.js".to_vec())
    );
}

#[test]
fn options_before_the_script_are_skipped() {
    assert_eq!(
        script_of(&[
            b"node",
            b"--max-old-space-size=4096",
            b"--no-warnings",
            b"/a/cli.js"
        ]),
        Some(b"/a/cli.js".to_vec())
    );
}

#[test]
fn the_value_of_an_option_that_takes_a_separate_value_is_not_the_script() {
    for option in [
        &b"-r"[..],
        b"--require",
        b"--import",
        b"--loader",
        b"--experimental-loader",
        b"-C",
        b"--conditions",
    ] {
        assert_eq!(
            script_of(&[b"node", option, b"/preload/claude-hook.js", b"/a/server.js"]),
            Some(b"/a/server.js".to_vec()),
            "{}",
            String::from_utf8_lossy(option)
        );
    }
}

#[test]
fn an_option_with_its_value_attached_is_one_argument() {
    assert_eq!(
        script_of(&[b"node", b"--require=/preload/claude-hook.js", b"/a/server.js"]),
        Some(b"/a/server.js".to_vec())
    );
}

#[test]
fn a_double_dash_ends_the_options() {
    assert_eq!(
        script_of(&[b"node", b"--", b"/a/cli.js"]),
        Some(b"/a/cli.js".to_vec())
    );
    assert_eq!(script_of(&[b"node", b"--"]), None);
}

#[test]
fn code_given_on_the_command_line_is_not_a_script() {
    for option in [&b"-e"[..], b"--eval", b"-p", b"--print"] {
        assert_eq!(
            script_of(&[b"node", option, b"console.log('claude')"]),
            None,
            "{}",
            String::from_utf8_lossy(option)
        );
    }
}

#[test]
fn a_program_without_arguments_has_no_script() {
    assert_eq!(script_of(&[b"node"]), None);
    assert_eq!(script_of(&[b"node", b"--version"]), None);
    assert_eq!(script_of(&[b"node", b"-"]), None);
    let none = buffer(0, b"/usr/local/bin/node", &[b"HOME=/x"]);
    assert_eq!(script_argument(&none).unwrap(), None);
}

#[test]
fn an_environment_entry_is_never_taken_for_the_script() {
    let buf = buffer(
        1,
        b"/usr/local/bin/node",
        &[b"node", b"CLAUDE_SCRIPT=/x/claude/cli.js"],
    );
    assert_eq!(script_argument(&buf).unwrap(), None);
}

#[test]
fn the_environment_is_never_read() {
    let mut buf = buffer(2, b"/usr/local/bin/node", &[b"node", b"/a/cli.js"]);
    buf.extend_from_slice(b"HALF=an entry with no end");
    assert_eq!(script_argument(&buf).unwrap(), Some(&b"/a/cli.js"[..]));
}

#[test]
fn a_buffer_cut_inside_the_arguments_is_an_error() {
    let mut buf = buffer(2, b"/usr/local/bin/node", &[b"node"]);
    assert_eq!(script_argument(&buf), Err(ParseError::Truncated));
    buf.truncate(3);
    assert_eq!(script_argument(&buf), Err(ParseError::TooShort));
    let negative = buffer(-1, b"/usr/local/bin/node", &[]);
    assert_eq!(script_argument(&negative), Err(ParseError::NegativeArgc));
}
