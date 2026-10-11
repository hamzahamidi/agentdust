use agentdust_core::procargs::{ParseError, env_value, parse};

fn buffer(argc: i32, exec_path: &[u8], strings: &[&[u8]]) -> Vec<u8> {
    let mut buf = argc.to_ne_bytes().to_vec();
    buf.extend_from_slice(exec_path);
    buf.push(0);
    let alignment = std::mem::size_of::<usize>();
    let padding = (alignment - (b"executable_path=".len() + exec_path.len() + 1) % alignment) % alignment;
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
        &[b"AGENTDUST_SESSION_OLD=no", b"AGENTDUST_SESSION=yes"],
    );
    let parsed = parse(&buf).unwrap();
    assert_eq!(env_value(&parsed, "AGENTDUST_SESSION"), Some(&b"yes"[..]));
    assert_eq!(env_value(&parsed, "AGENTDUST"), None);
}

#[test]
fn parses_an_empty_argv0_without_shifting_the_environment() {
    let buf = buffer(2, b"/bin/sleep", &[b"", b"5", b"AGENTDUST_SESSION=empty-argv0"]);
    let parsed = parse(&buf).unwrap();
    assert_eq!(parsed.args, vec![&b""[..], &b"5"[..]]);
    assert_eq!(env_value(&parsed, "AGENTDUST_SESSION"), Some(&b"empty-argv0"[..]));
}

#[cfg(target_os = "macos")]
#[test]
fn parses_empty_argv0_from_a_darwin_kernel_buffer() {
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};

    struct ChildGuard(Child);

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    let mut command = Command::new("/bin/sleep");
    command
        .arg0("")
        .arg("5")
        .env("AGENTDUST_SESSION", "kernel-empty-argv0");
    let child = ChildGuard(command.spawn().unwrap());
    let bytes = agentdust_core::darwin::procargs2(child.0.id() as i32)
        .unwrap()
        .unwrap();
    let parsed = parse(&bytes).unwrap();
    assert_eq!(parsed.args, vec![&b""[..], &b"5"[..]]);
    assert_eq!(
        env_value(&parsed, "AGENTDUST_SESSION"),
        Some(&b"kernel-empty-argv0"[..])
    );
}

#[test]
fn rejects_nonzero_executable_path_padding() {
    let mut buf = buffer(0, b"/bin/sleep", &[]);
    let padding_start = 4 + b"/bin/sleep".len() + 1;
    buf[padding_start] = b'x';
    assert_eq!(parse(&buf), Err(ParseError::InvalidPadding));
}

#[test]
fn stops_at_the_first_empty_env_entry() {
    let mut buf = buffer(0, b"/bin/x", &[b"A=1"]);
    buf.push(0);
    buf.extend_from_slice(b"B=2\0");
    assert_eq!(parse(&buf).unwrap().env, vec![&b"A=1"[..]]);
}

#[test]
fn ignores_a_final_entry_without_nul() {
    let mut buf = buffer(0, b"/bin/x", &[b"A=1"]);
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
    let buf = buffer(-1, b"/bin/x", &[]);
    assert_eq!(parse(&buf), Err(ParseError::NegativeArgc));
}

#[test]
fn rejects_argc_larger_than_the_arguments_present() {
    let buf = buffer(5, b"/bin/x", &[b"x"]);
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
    let buf = buffer(i32::MAX, b"/bin/x", &[b"a"]);
    assert_eq!(parse(&buf), Err(ParseError::Truncated));
}
