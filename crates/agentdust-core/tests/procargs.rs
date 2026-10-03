use agentdust_core::procargs::{ParseError, env_value, parse};

fn buffer(argc: i32, exec_path: &[u8], padding: usize, strings: &[&[u8]]) -> Vec<u8> {
    let mut buf = argc.to_ne_bytes().to_vec();
    buf.extend_from_slice(exec_path);
    buf.push(0);
    buf.extend(std::iter::repeat_n(0, padding));
    for s in strings {
        buf.extend_from_slice(s);
        buf.push(0);
    }
    buf
}

#[test]
fn parses_exec_path_args_and_env_after_padding() {
    let buf = buffer(
        2,
        b"/usr/bin/node",
        3,
        &[b"node", b"server.js", b"HOME=/Users/a", b"AGENTDUST_SESSION=abc"],
    );
    let parsed = parse(&buf).unwrap();
    assert_eq!(parsed.exec_path, b"/usr/bin/node");
    assert_eq!(parsed.args, vec![&b"node"[..], &b"server.js"[..]]);
    assert_eq!(
        parsed.env,
        vec![&b"HOME=/Users/a"[..], &b"AGENTDUST_SESSION=abc"[..]]
    );
}

#[test]
fn env_value_matches_the_exact_name_only() {
    let buf = buffer(
        0,
        b"/bin/x",
        0,
        &[b"AGENTDUST_SESSION_OLD=no", b"AGENTDUST_SESSION=yes"],
    );
    let parsed = parse(&buf).unwrap();
    assert_eq!(env_value(&parsed, "AGENTDUST_SESSION"), Some(&b"yes"[..]));
    assert_eq!(env_value(&parsed, "AGENTDUST"), None);
}

#[test]
fn stops_at_the_first_empty_env_entry() {
    let mut buf = buffer(0, b"/bin/x", 0, &[b"A=1"]);
    buf.push(0);
    buf.extend_from_slice(b"B=2\0");
    assert_eq!(parse(&buf).unwrap().env, vec![&b"A=1"[..]]);
}

#[test]
fn ignores_a_final_entry_without_nul() {
    let mut buf = buffer(0, b"/bin/x", 0, &[b"A=1"]);
    buf.extend_from_slice(b"B=2");
    assert_eq!(parse(&buf).unwrap().env, vec![&b"A=1"[..]]);
}

#[test]
fn rejects_a_buffer_shorter_than_the_header() {
    assert_eq!(parse(&[1, 0]), Err(ParseError::TooShort));
    assert_eq!(parse(&[]), Err(ParseError::TooShort));
}

#[test]
fn rejects_negative_argc() {
    let buf = buffer(-1, b"/bin/x", 0, &[]);
    assert_eq!(parse(&buf), Err(ParseError::NegativeArgc));
}

#[test]
fn rejects_argc_larger_than_the_arguments_present() {
    let buf = buffer(5, b"/bin/x", 0, &[b"x"]);
    assert_eq!(parse(&buf), Err(ParseError::Truncated));
}

#[test]
fn rejects_an_exec_path_without_nul() {
    let mut buf = 0i32.to_ne_bytes().to_vec();
    buf.extend_from_slice(b"/bin/x");
    assert_eq!(parse(&buf), Err(ParseError::Truncated));
}

#[test]
fn huge_argc_does_not_allocate_unbounded_memory() {
    let buf = buffer(i32::MAX, b"/bin/x", 0, &[b"a"]);
    assert_eq!(parse(&buf), Err(ParseError::Truncated));
}
