mod journal_support;
mod scratch;
mod session_support;

use std::fs;
use std::os::unix::fs::PermissionsExt;

use agentdust_core::doctor::{Unavailable, load_sessions};
use agentdust_core::journal::Kind;
use agentdust_core::journal::volume::{FixedVolume, MNT_LOCAL, classify};
use agentdust_core::session::State;
use journal_support::{journal, plant};
use scratch::TempDir;
use session_support::{Edit, Table, rec};

fn apfs() -> FixedVolume {
    FixedVolume::apfs_local()
}

#[test]
fn no_journal_and_a_usable_secret_give_no_sessions_and_no_reason() {
    let dir = TempDir::private("doc-none");
    let found = load_sessions(&dir, &apfs(), true, &Table::new());
    assert_eq!(found.unavailable, None);
    assert!(found.scopes.is_empty());
    assert_eq!(found.records, 0);
    assert_eq!(found.skipped_lines, 0);
}

#[test]
fn a_missing_data_directory_is_not_created_by_the_report() {
    let dir = TempDir::absent("doc-absent");
    let found = load_sessions(&dir, &apfs(), true, &Table::new());
    assert_eq!(found.unavailable, None);
    assert!(!dir.exists());
}

#[test]
fn records_become_scopes_with_the_liveness_of_their_agent() {
    let dir = TempDir::private("doc-scopes");
    let journal = journal(&dir);
    for record in [
        rec(Kind::SessionStart).session("a").by(10).tagged(1),
        rec(Kind::SessionStart).session("b").by(20).tagged(2),
        rec(Kind::ShellStart).session("b").by(20),
    ] {
        journal.append(&record).unwrap();
    }
    let probe = Table::new().gone(10).alive(20);
    let found = load_sessions(&dir, &apfs(), true, &probe);
    assert_eq!(found.unavailable, None);
    assert_eq!(found.records, 3);
    assert_eq!(found.scopes.len(), 2);
    let states: Vec<State> = found.scopes.iter().map(|scope| scope.state).collect();
    assert_eq!(states, [State::Ended, State::Active]);
}

#[test]
fn a_session_without_a_found_agent_is_a_degraded_scope() {
    let dir = TempDir::private("doc-degraded");
    journal(&dir).append(&rec(Kind::SessionStart).tagged(1)).unwrap();
    let found = load_sessions(&dir, &apfs(), true, &Table::new());
    assert_eq!(found.unavailable, None);
    assert!(found.scopes[0].degraded());
}

#[test]
fn a_journal_of_a_newer_version_makes_owned_classes_unavailable() {
    let dir = TempDir::private("doc-version");
    plant(
        &dir,
        "journal.jsonl",
        b"\x1e{\"v\":3,\"kind\":\"session_start\"}\n",
    );
    let found = load_sessions(&dir, &apfs(), true, &Table::new());
    assert_eq!(found.unavailable, Some(Unavailable::UnsupportedVersion));
    assert!(found.scopes.is_empty());
}

#[test]
fn a_newer_line_among_good_ones_still_makes_owned_classes_unavailable() {
    let dir = TempDir::private("doc-version-mixed");
    journal(&dir)
        .append(&rec(Kind::SessionStart).by(10).tagged(1))
        .unwrap();
    journal_support::append_raw(&dir, "journal.jsonl", b"\x1e{\"v\":9}\n");
    let found = load_sessions(&dir, &apfs(), true, &Table::new().gone(10));
    assert_eq!(found.unavailable, Some(Unavailable::UnsupportedVersion));
    assert!(found.scopes.is_empty());
}

#[test]
fn a_volume_that_is_not_local_apfs_makes_owned_classes_unavailable_and_names_it() {
    let dir = TempDir::private("doc-volume");
    let nfs = FixedVolume::new(classify("nfs", 0));
    let found = load_sessions(&dir, &nfs, true, &Table::new());
    assert_eq!(
        found.unavailable,
        Some(Unavailable::UnsupportedFilesystem("nfs".to_owned()))
    );
    assert!(found.scopes.is_empty());
    let local_hfs = FixedVolume::new(classify("hfs", MNT_LOCAL));
    let found = load_sessions(&dir, &local_hfs, true, &Table::new());
    assert_eq!(
        found.unavailable,
        Some(Unavailable::UnsupportedFilesystem("hfs".to_owned()))
    );
}

#[test]
fn a_secret_that_cannot_be_used_makes_owned_classes_unavailable() {
    let dir = TempDir::private("doc-secret");
    journal(&dir)
        .append(&rec(Kind::SessionStart).by(10).tagged(1))
        .unwrap();
    let found = load_sessions(&dir, &apfs(), false, &Table::new().gone(10));
    assert_eq!(found.unavailable, Some(Unavailable::SecretUnavailable));
    assert!(found.scopes.is_empty());
}

#[test]
fn an_unsupported_version_is_reported_before_a_missing_secret() {
    let dir = TempDir::private("doc-order");
    plant(&dir, "journal.jsonl", b"\x1e{\"v\":3}\n");
    let found = load_sessions(&dir, &apfs(), false, &Table::new());
    assert_eq!(found.unavailable, Some(Unavailable::UnsupportedVersion));
}

#[test]
fn an_unsafe_data_directory_is_refused_and_not_repaired() {
    let dir = TempDir::private("doc-unsafe");
    journal(&dir).append(&rec(Kind::SessionStart)).unwrap();
    fs::set_permissions(&*dir, fs::Permissions::from_mode(0o755)).unwrap();
    let found = load_sessions(&dir, &apfs(), true, &Table::new());
    assert_eq!(found.unavailable, Some(Unavailable::JournalRefused));
    assert!(found.scopes.is_empty());
    assert_eq!(fs::metadata(&*dir).unwrap().permissions().mode() & 0o7777, 0o755);
    fs::set_permissions(&*dir, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn a_corrupt_line_costs_that_record_and_is_counted_but_owned_classes_stay_available() {
    let dir = TempDir::private("doc-corrupt");
    let journal = journal(&dir);
    journal.append(&rec(Kind::SessionStart).by(10).tagged(1)).unwrap();
    journal_support::append_raw(&dir, "journal.jsonl", b"\x1enot json at all\n");
    journal.append(&rec(Kind::ShellStart).by(10)).unwrap();
    let found = load_sessions(&dir, &apfs(), true, &Table::new().gone(10));
    assert_eq!(found.unavailable, None);
    assert_eq!(found.records, 2);
    assert_eq!(found.skipped_lines, 1);
    assert_eq!(found.scopes.len(), 1);
}

#[test]
fn the_reasons_have_fixed_codes_and_descriptions_without_paths() {
    let cases = [
        (Unavailable::UnsupportedVersion, "unsupported_version"),
        (
            Unavailable::UnsupportedFilesystem("nfs".to_owned()),
            "unsupported_filesystem",
        ),
        (Unavailable::SecretUnavailable, "secret_unavailable"),
        (Unavailable::JournalRefused, "journal_refused"),
        (Unavailable::JournalUnreadable, "journal_unreadable"),
    ];
    for (reason, code) in cases {
        assert_eq!(reason.code(), code);
        assert!(!reason.describe().contains('/'), "{}", reason.describe());
        assert!(!reason.describe().is_empty());
    }
    assert!(
        Unavailable::UnsupportedFilesystem("nfs".to_owned())
            .describe()
            .contains("nfs")
    );
}

#[test]
fn a_filesystem_name_is_escaped_in_the_description() {
    let hostile = Unavailable::UnsupportedFilesystem("a\u{1b}[31m\nb".to_owned());
    assert!(!hostile.describe().contains('\u{1b}'));
    assert!(!hostile.describe().contains('\n'));
}
