use std::collections::BTreeSet;

use agentdust_core::inventory::{ListError, parse_launchctl_list};

const SAMPLE: &str = "PID\tStatus\tLabel\n\
-\t0\tcom.apple.SafariHistoryServiceAgent\n\
-\t0\tcom.apple.progressd\n\
88111\t-9\tcom.apple.cloudphotod\n\
412\t0\thomebrew.mxcl.postgresql@16\n\
5\t0\tapplication.com.example.App.1234.5678\n\
-\t-15\tcom.example.stopped\n";

#[test]
fn rows_with_a_pid_give_the_pids_and_dashes_are_skipped() {
    assert_eq!(
        parse_launchctl_list(SAMPLE).unwrap(),
        BTreeSet::from([5, 412, 88111])
    );
}

#[test]
fn a_header_alone_is_an_empty_list() {
    assert_eq!(
        parse_launchctl_list("PID\tStatus\tLabel\n").unwrap(),
        BTreeSet::new()
    );
}

#[test]
fn empty_output_is_an_error_and_not_an_empty_list() {
    assert_eq!(parse_launchctl_list(""), Err(ListError::NoHeader));
    assert_eq!(parse_launchctl_list("\n"), Err(ListError::NoHeader));
}

#[test]
fn another_header_is_an_error() {
    assert_eq!(
        parse_launchctl_list("Label\tPID\n1\tx\n"),
        Err(ListError::NoHeader)
    );
}

#[test]
fn a_row_that_is_not_three_columns_is_an_error() {
    for text in [
        "PID\tStatus\tLabel\n12\t0\n",
        "PID\tStatus\tLabel\n12\n",
        "PID\tStatus\tLabel\n12\t0\ta\tb\n",
        "PID\tStatus\tLabel\n12 0 a\n",
    ] {
        assert_eq!(
            parse_launchctl_list(text),
            Err(ListError::BadRow { line: 2 }),
            "{text:?}"
        );
    }
}

#[test]
fn a_pid_that_is_not_a_positive_number_or_a_dash_is_an_error() {
    for pid in ["abc", "0", "-5", "1x", "+7", "99999999999", ""] {
        let text = format!("PID\tStatus\tLabel\n{pid}\t0\tlabel\n");
        assert_eq!(
            parse_launchctl_list(&text),
            Err(ListError::BadRow { line: 2 }),
            "{pid:?}"
        );
    }
}

#[test]
fn the_status_column_must_be_a_number_or_a_dash() {
    assert_eq!(
        parse_launchctl_list("PID\tStatus\tLabel\n12\tx\tlabel\n"),
        Err(ListError::BadRow { line: 2 })
    );
    assert_eq!(
        parse_launchctl_list("PID\tStatus\tLabel\n12\t-\tlabel\n").unwrap(),
        BTreeSet::from([12])
    );
}

#[test]
fn the_error_names_the_line_of_the_first_bad_row() {
    assert_eq!(
        parse_launchctl_list("PID\tStatus\tLabel\n1\t0\ta\n2\t0\tb\nbad\n"),
        Err(ListError::BadRow { line: 4 })
    );
}

#[test]
fn a_blank_line_between_rows_is_ignored_and_a_missing_final_newline_is_fine() {
    assert_eq!(
        parse_launchctl_list("PID\tStatus\tLabel\n1\t0\ta\n\n2\t0\tb").unwrap(),
        BTreeSet::from([1, 2])
    );
}
