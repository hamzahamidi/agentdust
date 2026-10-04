mod support;

use std::fs;
use std::process::Command;
use std::time::Duration;

use agentdust_agents::native_cli::{
    CliError, CliOutput, CliRunner, McpError, McpLookup, McpServer, SERVER_NAME, SystemRunner, add_command,
    lookup, matches_desired, mcp_hash, parse_get, register, remove_command, unregister_if_ours,
};
use agentdust_core::digest::sha256_hex;
use support::{FakeClaude, Mode, Registered, TempDir, render, script};

const EXE: &str = "/opt/homebrew/bin/agentdust";

fn output(code: i32, stdout: &str, stderr: &str) -> CliOutput {
    CliOutput {
        code: Some(code),
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
    }
}

fn runner(program: &std::path::Path) -> SystemRunner {
    SystemRunner::new(program.to_path_buf(), None, Duration::from_secs(60))
}

fn run_retrying(runner: &SystemRunner, args: &[&str]) -> Result<CliOutput, CliError> {
    for _ in 0..100 {
        match runner.run(args) {
            Err(CliError::Spawn(reason)) if reason.contains("busy") => {
                std::thread::sleep(Duration::from_millis(20))
            }
            other => return other,
        }
    }
    runner.run(args)
}

const REAL_GET: &str = "agentdust:\n  Scope: User config (available in all your projects)\n  Status: \u{2718} Failed to connect\n  Issue: ENOENT: ENOENT: no such file or directory, posix_spawn '/tmp/some dir/agentdust'\n  Type: stdio\n  Command: /tmp/some dir/agentdust\n  Args: mcp\n  Environment:\n\nTo remove this server, run: claude mcp remove agentdust -s user\n";

const REAL_GET_WITH_ENV: &str = "envtest:\n  Scope: User config (available in all your projects)\n  Status: \u{2718} Failed to connect\n  Issue: CONNECTION_CLOSED: Connection closed\n  Type: stdio\n  Command: /usr/bin/true\n  Args: a b c d\n  Environment:\n    API_KEY=abc\n    B=c\n\nTo remove this server, run: claude mcp remove envtest -s user\n";

const REAL_GET_HTTP: &str = "h:\n  Scope: User config (available in all your projects)\n  Status: \u{2718} Failed to connect\n  Issue: ENOTFOUND: getaddrinfo ENOTFOUND example.invalid\n  Type: http\n  URL: https://example.invalid/mcp\n\nTo remove this server, run: claude mcp remove h -s user\n";

const REAL_NOT_FOUND: &str = "No MCP server named \"agentdust\". Run `claude mcp add` to add one.\n";

#[test]
fn the_server_name_and_native_commands_are_the_documented_ones() {
    assert_eq!(SERVER_NAME, "agentdust");
    assert_eq!(
        add_command(EXE),
        ["mcp", "add", "--scope", "user", "agentdust", "--", EXE, "mcp"]
    );
    assert_eq!(
        remove_command(),
        ["mcp", "remove", "agentdust", "--scope", "user"]
    );
}

#[test]
fn the_real_get_output_is_read_field_by_field() {
    let found = parse_get(&output(0, REAL_GET, ""));
    assert_eq!(
        found,
        McpLookup::Found(McpServer {
            scope: Some("User config (available in all your projects)".to_owned()),
            transport: Some("stdio".to_owned()),
            command: Some("/tmp/some dir/agentdust".to_owned()),
            args: "mcp".to_owned(),
            env_entries: 0,
        })
    );
}

#[test]
fn environment_entries_and_other_transports_are_read() {
    let McpLookup::Found(with_env) = parse_get(&output(0, REAL_GET_WITH_ENV, "")) else {
        panic!("not found");
    };
    assert_eq!(with_env.env_entries, 2);
    assert_eq!(with_env.args, "a b c d");
    let McpLookup::Found(http) = parse_get(&output(0, REAL_GET_HTTP, "")) else {
        panic!("not found");
    };
    assert_eq!(http.transport.as_deref(), Some("http"));
    assert_eq!(http.command, None);
}

#[test]
fn a_missing_server_is_recognised_on_either_stream() {
    assert_eq!(parse_get(&output(1, "", REAL_NOT_FOUND)), McpLookup::NotFound);
    assert_eq!(parse_get(&output(1, REAL_NOT_FOUND, "")), McpLookup::NotFound);
}

#[test]
fn anything_else_is_unknown_and_never_taken_for_missing() {
    for case in [
        output(1, "", "Error: configuration is corrupt\n"),
        output(0, "", ""),
        output(0, "Something entirely different\n", ""),
        output(2, "", REAL_NOT_FOUND),
        CliOutput {
            code: None,
            stdout: String::new(),
            stderr: REAL_NOT_FOUND.to_owned(),
        },
    ] {
        assert!(matches!(parse_get(&case), McpLookup::Unknown(_)), "{case:?}");
    }
}

#[test]
fn only_the_exact_command_and_arguments_in_user_scope_match() {
    let server =
        |command: &str, args: &str, scope: Option<&str>, env: usize, transport: Option<&str>| McpServer {
            scope: scope.map(str::to_owned),
            transport: transport.map(str::to_owned),
            command: Some(command.to_owned()),
            args: args.to_owned(),
            env_entries: env,
        };
    let user = Some("User config (available in all your projects)");
    assert!(matches_desired(
        &server(EXE, "mcp", user, 0, Some("stdio")),
        EXE,
        "mcp"
    ));
    assert!(matches_desired(&server(EXE, "mcp", None, 0, None), EXE, "mcp"));
    assert!(!matches_desired(
        &server("/other", "mcp", user, 0, Some("stdio")),
        EXE,
        "mcp"
    ));
    assert!(!matches_desired(
        &server(EXE, "mcp --extra", user, 0, Some("stdio")),
        EXE,
        "mcp"
    ));
    assert!(!matches_desired(
        &server(EXE, "", user, 0, Some("stdio")),
        EXE,
        "mcp"
    ));
    assert!(!matches_desired(
        &server(EXE, "mcp", user, 1, Some("stdio")),
        EXE,
        "mcp"
    ));
    assert!(!matches_desired(
        &server(EXE, "mcp", user, 0, Some("http")),
        EXE,
        "mcp"
    ));
    assert!(!matches_desired(
        &server(
            EXE,
            "mcp",
            Some("Project config (shared via .mcp.json)"),
            0,
            Some("stdio")
        ),
        EXE,
        "mcp"
    ));
    assert!(!matches_desired(
        &server(
            EXE,
            "mcp",
            Some("Local config (private to you in this project)"),
            0,
            Some("stdio")
        ),
        EXE,
        "mcp"
    ));
}

#[test]
fn the_registration_hash_covers_the_name_command_and_arguments() {
    assert_eq!(mcp_hash(EXE, "mcp"), mcp_hash(EXE, "mcp"));
    assert_ne!(mcp_hash(EXE, "mcp"), mcp_hash("/other", "mcp"));
    assert_ne!(mcp_hash(EXE, "mcp"), mcp_hash(EXE, "mcp x"));
    assert_eq!(mcp_hash(EXE, "mcp").len(), sha256_hex(b"").len());
}

#[test]
fn an_honest_cli_registers_and_the_listing_proves_it() {
    let fake = FakeClaude::new(Mode::Honest);
    register(&fake, EXE).unwrap();
    assert_eq!(
        fake.calls(),
        [
            format!("mcp add --scope user agentdust -- {EXE} mcp"),
            "mcp get agentdust".to_owned()
        ]
    );
    assert!(matches!(lookup(&fake), McpLookup::Found(server) if matches_desired(&server, EXE, "mcp")));
}

#[test]
fn a_cli_that_exits_zero_without_changing_anything_is_detected() {
    let fake = FakeClaude::new(Mode::Liar);
    match register(&fake, EXE) {
        Err(McpError::Unverified(reason)) => assert!(reason.contains("no such server"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert!(fake.state.borrow().is_none());
}

#[test]
fn a_cli_that_registers_something_else_is_detected_and_named() {
    let fake = FakeClaude::new(Mode::AddsSomethingElse);
    match register(&fake, EXE) {
        Err(McpError::Unverified(reason)) => assert!(reason.contains("/somewhere/else"), "{reason}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_listing_that_cannot_be_read_back_is_not_a_success() {
    for mode in [Mode::GetBroken, Mode::UnreadableGet] {
        let fake = FakeClaude::new(mode);
        assert!(
            matches!(register(&fake, EXE), Err(McpError::Unverified(_))),
            "{mode:?}"
        );
    }
}

#[test]
fn a_failing_add_is_reported_with_the_cli_message() {
    let fake = FakeClaude::new(Mode::AddFails);
    match register(&fake, EXE) {
        Err(McpError::AddFailed(reason)) => {
            assert!(reason.contains("cannot write the configuration"), "{reason}")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(fake.calls().len(), 1);
}

#[test]
fn an_existing_server_is_never_overwritten_by_register() {
    let fake = FakeClaude::with(Mode::Honest, "/theirs/agentdust", &["serve"]);
    assert!(matches!(register(&fake, EXE), Err(McpError::AddFailed(_))));
    assert_eq!(fake.state.borrow().as_ref().unwrap().command, "/theirs/agentdust");
}

#[test]
fn unregister_removes_only_a_registration_that_matches_and_checks_it_is_gone() {
    let ours = FakeClaude::with(Mode::Honest, EXE, &["mcp"]);
    unregister_if_ours(&ours, EXE).unwrap();
    assert!(ours.state.borrow().is_none());
    assert_eq!(
        ours.calls(),
        [
            "mcp get agentdust".to_owned(),
            "mcp remove agentdust --scope user".to_owned(),
            "mcp get agentdust".to_owned()
        ]
    );

    let theirs = FakeClaude::with(Mode::Honest, "/theirs/agentdust", &["mcp"]);
    assert!(unregister_if_ours(&theirs, EXE).is_err());
    assert!(theirs.state.borrow().is_some());
    assert_eq!(theirs.calls(), ["mcp get agentdust"]);

    let absent = FakeClaude::new(Mode::Honest);
    unregister_if_ours(&absent, EXE).unwrap();
    assert_eq!(absent.calls(), ["mcp get agentdust"]);
}

#[test]
fn a_cli_that_ignores_remove_is_detected() {
    struct Stubborn(FakeClaude);
    impl CliRunner for Stubborn {
        fn run(&self, args: &[&str]) -> Result<CliOutput, CliError> {
            if args.get(1) == Some(&"remove") {
                return Ok(output(0, "Removed\n", ""));
            }
            self.0.run(args)
        }
    }
    let stubborn = Stubborn(FakeClaude::with(Mode::Honest, EXE, &["mcp"]));
    assert!(unregister_if_ours(&stubborn, EXE).is_err());
}

#[test]
fn the_fake_prints_what_the_real_cli_prints() {
    let registered = Registered {
        command: "/tmp/some dir/agentdust".to_owned(),
        args: vec!["mcp".to_owned()],
        scope: "User config (available in all your projects)",
    };
    let text = render(&registered);
    assert_eq!(
        parse_get(&output(0, &text, "")),
        parse_get(&output(0, REAL_GET, ""))
    );
}

#[test]
fn a_runner_captures_output_streams_and_the_exit_code() {
    let dir = TempDir::new("cli-capture");
    let program = script(&dir, "cli", "echo out; echo err >&2; exit 3");
    let result = run_retrying(&runner(&program), &[]).unwrap();
    assert_eq!(result, output(3, "out\n", "err\n"));
}

#[test]
fn a_runner_passes_arguments_exactly() {
    let dir = TempDir::new("cli-args");
    let program = script(&dir, "cli", "for arg in \"$@\"; do echo \"[$arg]\"; done");
    let result = run_retrying(&runner(&program), &["mcp", "add", "a b", "", "--", "$HOME", "*"]).unwrap();
    assert_eq!(result.stdout, "[mcp]\n[add]\n[a b]\n[]\n[--]\n[$HOME]\n[*]\n");
}

#[test]
fn a_runner_gives_the_child_the_config_directory_and_no_input() {
    let dir = TempDir::new("cli-env");
    let program = script(&dir, "cli", "cat; echo \"dir=$CLAUDE_CONFIG_DIR\"");
    let with = SystemRunner::new(
        program.clone(),
        Some("/work/claude".into()),
        Duration::from_secs(60),
    );
    assert_eq!(run_retrying(&with, &[]).unwrap().stdout, "dir=/work/claude\n");
}

#[test]
fn a_runner_reads_large_output_on_both_streams_without_blocking() {
    let dir = TempDir::new("cli-large");
    let program = script(
        &dir,
        "cli",
        "head -c 2000000 /dev/zero | tr '\\0' 'x'; head -c 2000000 /dev/zero | tr '\\0' 'y' >&2",
    );
    let result = run_retrying(&runner(&program), &[]).unwrap();
    assert_eq!(result.stdout.len(), 2_000_000);
    assert_eq!(result.stderr.len(), 2_000_000);
}

#[test]
fn a_runner_accepts_output_that_is_not_utf8() {
    let dir = TempDir::new("cli-binary");
    let program = script(&dir, "cli", "printf 'a\\377b'");
    let result = run_retrying(&runner(&program), &[]).unwrap();
    assert!(result.stdout.starts_with('a') && result.stdout.ends_with('b'));
}

#[test]
fn a_runner_reports_a_program_that_cannot_start() {
    let dir = TempDir::new("cli-missing");
    let result = runner(&dir.join("nope")).run(&[]);
    assert!(matches!(result, Err(CliError::Spawn(_))), "{result:?}");
}

#[test]
fn a_runner_kills_a_program_that_does_not_finish() {
    let dir = TempDir::new("cli-timeout");
    let pid_file = dir.join("pid");
    let program = script(
        &dir,
        "cli",
        &format!("echo $$ > '{}'; exec sleep 30", pid_file.display()),
    );
    let result = run_retrying(&SystemRunner::new(program, None, Duration::from_secs(4)), &[]);
    assert!(matches!(result, Err(CliError::TimedOut(_))), "{result:?}");
    let pid = fs::read_to_string(&pid_file).unwrap();
    let alive = Command::new("kill")
        .args(["-0", pid.trim()])
        .output()
        .unwrap()
        .status
        .success();
    assert!(!alive, "the child {pid} is still running");
}

#[test]
fn a_runner_does_not_wait_for_a_background_child_that_keeps_the_pipes_open() {
    let dir = TempDir::new("cli-background");
    let pid_file = dir.join("pid");
    let program = script(
        &dir,
        "cli",
        &format!(
            "echo visible; sleep 5 & echo $! > '{}'; exit 0",
            pid_file.display()
        ),
    );
    let result = run_retrying(&runner(&program), &[]).unwrap();
    assert_eq!(result.code, Some(0));
    assert!(result.stdout.starts_with("visible"));
    let pid = fs::read_to_string(&pid_file).unwrap();
    let _ = Command::new("kill").arg(pid.trim()).output();
}
