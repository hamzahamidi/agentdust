use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::time::Duration;

use agentdust_testkit::spec::{MAX_SPAWN, ProcSpec, SpecError};

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn parse(args: &[&str]) -> Result<ProcSpec, SpecError> {
    ProcSpec::parse(os(args))
}

fn full() -> ProcSpec {
    ProcSpec::new()
        .seconds(20)
        .ignore_term()
        .exit_after_ms(500)
        .spawn(2)
        .setsid()
        .echo_env("TAG")
        .report_file("/tmp/r")
}

#[test]
fn an_empty_spec_has_no_arguments() {
    assert_eq!(ProcSpec::new().args(), Vec::<OsString>::new());
    assert_eq!(parse(&[]).unwrap(), ProcSpec::new());
}

#[test]
fn the_arguments_come_in_one_fixed_order() {
    assert_eq!(
        full().args(),
        os(&[
            "20",
            "--ignore-term",
            "--exit-after-ms",
            "500",
            "--spawn",
            "2",
            "--setsid",
            "--echo-env",
            "TAG",
            "--report-file",
            "/tmp/r",
        ])
    );
}

#[test]
fn the_environment_is_never_an_argument() {
    let spec = ProcSpec::new().env("SECRET", "value").env("OTHER", "x");
    assert_eq!(spec.args(), Vec::<OsString>::new());
    assert_eq!(
        spec.env,
        vec![("SECRET".into(), "value".into()), ("OTHER".into(), "x".into())]
    );
}

#[test]
fn every_combination_of_flags_round_trips() {
    for bits in 0..64u32 {
        let mut spec = ProcSpec::new();
        if bits & 1 != 0 {
            spec = spec.seconds(7);
        }
        if bits & 2 != 0 {
            spec = spec.ignore_term();
        }
        if bits & 4 != 0 {
            spec = spec.exit_after_ms(250);
        }
        if bits & 8 != 0 {
            spec = spec.spawn(3);
        }
        if bits & 16 != 0 {
            spec = spec.setsid();
        }
        if bits & 32 != 0 {
            spec = spec.report_file("/tmp/with space/r").echo_env("NAME");
        }
        assert_eq!(ProcSpec::parse(spec.args()).unwrap(), spec, "bits {bits:#08b}");
    }
}

#[test]
fn the_m0_invocation_with_only_a_number_of_seconds_still_parses() {
    assert_eq!(parse(&["30"]).unwrap(), ProcSpec::new().seconds(30));
    assert_eq!(parse(&["0"]).unwrap(), ProcSpec::new().seconds(0));
}

#[test]
fn flags_and_the_number_of_seconds_may_come_in_any_order() {
    assert_eq!(
        parse(&["--setsid", "--spawn", "1", "9", "--ignore-term"]).unwrap(),
        ProcSpec::new().seconds(9).setsid().spawn(1).ignore_term()
    );
}

#[test]
fn a_flag_that_is_not_known_is_refused() {
    assert!(matches!(parse(&["--bogus"]), Err(SpecError::UnknownFlag(f)) if f == "--bogus"));
    assert!(matches!(parse(&["-x"]), Err(SpecError::UnknownFlag(f)) if f == "-x"));
    assert!(matches!(
        parse(&["--ignore-term=1"]),
        Err(SpecError::UnknownFlag(_))
    ));
}

#[test]
fn a_flag_without_its_value_is_refused() {
    for flag in ["--exit-after-ms", "--spawn", "--echo-env", "--report-file"] {
        assert!(
            matches!(parse(&[flag]), Err(SpecError::MissingValue(f)) if f == flag),
            "{flag}"
        );
    }
}

#[test]
fn a_number_that_is_not_a_number_is_refused() {
    for value in ["abc", "-1", "1.5", "", "+3", " 3", "99999999999999999999999"] {
        assert!(
            matches!(parse(&["--spawn", value]), Err(SpecError::BadNumber { flag, .. }) if flag == "--spawn"),
            "{value:?}"
        );
        assert!(
            matches!(parse(&["--exit-after-ms", value]), Err(SpecError::BadNumber { flag, .. }) if flag == "--exit-after-ms"),
            "{value:?}"
        );
    }
    for value in ["abc", "1.5", "", "+3", " 3", "99999999999999999999999"] {
        assert!(
            matches!(parse(&[value]), Err(SpecError::BadNumber { flag, .. }) if flag == "seconds"),
            "{value:?}"
        );
    }
}

#[test]
fn a_flag_given_twice_is_refused() {
    for args in [
        &["--ignore-term", "--ignore-term"][..],
        &["--setsid", "--setsid"],
        &["--spawn", "1", "--spawn", "2"],
        &["--exit-after-ms", "1", "--exit-after-ms", "2"],
        &["--echo-env", "A", "--echo-env", "B", "--report-file", "/r"],
        &["--report-file", "/a", "--report-file", "/b"],
        &["5", "6"],
    ] {
        assert!(matches!(parse(args), Err(SpecError::Duplicate(_))), "{args:?}");
    }
}

#[test]
fn the_number_of_children_is_capped() {
    let at_cap = MAX_SPAWN.to_string();
    let over = (MAX_SPAWN + 1).to_string();
    assert_eq!(parse(&["--spawn", &at_cap]).unwrap().spawn, MAX_SPAWN);
    assert!(matches!(
        parse(&["--spawn", &over]),
        Err(SpecError::TooManyChildren { requested, max }) if requested == MAX_SPAWN + 1 && max == MAX_SPAWN
    ));
    assert_eq!(parse(&["--spawn", "0"]).unwrap().spawn, 0);
}

#[test]
fn echoing_a_variable_needs_a_report_file() {
    assert!(matches!(
        parse(&["--echo-env", "TAG"]),
        Err(SpecError::EchoWithoutReport)
    ));
    assert!(parse(&["--echo-env", "TAG", "--report-file", "/r"]).is_ok());
}

#[test]
fn an_argument_that_is_not_unicode_is_refused() {
    let bad = OsString::from_vec(vec![b'-', b'-', 0xff]);
    assert!(matches!(ProcSpec::parse([bad]), Err(SpecError::NotUnicode)));
    let value = OsString::from_vec(vec![0xff, 0xfe]);
    assert!(matches!(
        ProcSpec::parse([OsString::from("--echo-env"), value]),
        Err(SpecError::NotUnicode)
    ));
}

#[test]
fn a_child_inherits_every_flag_except_spawn_and_gets_its_own_report_file() {
    let spec = full().env("TAG", "x");
    let child = spec.child(2);
    assert_eq!(child.spawn, 0);
    assert_eq!(child.report_file, Some(PathBuf::from("/tmp/r.2")));
    assert_eq!(child.seconds, spec.seconds);
    assert!(child.ignore_term);
    assert_eq!(child.exit_after_ms, spec.exit_after_ms);
    assert!(child.setsid);
    assert_eq!(child.echo_env, spec.echo_env);
    assert_eq!(child.env, spec.env);
}

#[test]
fn a_child_of_a_spec_without_a_report_file_has_none() {
    let child = ProcSpec::new().spawn(1).child(1);
    assert_eq!(child.report_file, None);
    assert_eq!(child.spawn, 0);
}

#[test]
fn the_children_of_one_parent_have_distinct_report_files() {
    let spec = ProcSpec::new().spawn(3).report_file("/tmp/p");
    let files: Vec<_> = (1..=3).map(|i| spec.child(i).report_file.unwrap()).collect();
    assert_eq!(
        files,
        vec![
            PathBuf::from("/tmp/p.1"),
            PathBuf::from("/tmp/p.2"),
            PathBuf::from("/tmp/p.3")
        ]
    );
}

#[test]
fn the_lifetime_defaults_to_thirty_seconds() {
    assert_eq!(ProcSpec::new().lifetime(), Duration::from_secs(30));
}

#[test]
fn the_lifetime_is_the_shorter_of_the_seconds_and_the_exit_time() {
    assert_eq!(ProcSpec::new().seconds(2).lifetime(), Duration::from_secs(2));
    assert_eq!(
        ProcSpec::new().exit_after_ms(500).lifetime(),
        Duration::from_millis(500)
    );
    assert_eq!(
        ProcSpec::new().seconds(2).exit_after_ms(500).lifetime(),
        Duration::from_millis(500)
    );
    assert_eq!(
        ProcSpec::new().seconds(1).exit_after_ms(60_000).lifetime(),
        Duration::from_secs(1)
    );
    assert_eq!(ProcSpec::new().seconds(0).lifetime(), Duration::ZERO);
    assert_eq!(
        ProcSpec::new().seconds(u64::MAX).lifetime(),
        Duration::from_millis(u64::MAX)
    );
}
