use agentdust_core::acl::has_allow_entry;

const HEADER: &str = "!#acl 1\n";
const EVERYONE: &str = "group:ABCDEFAB-CDEF-ABCD-EFAB-CDEF0000000C:everyone:12";

fn text(entries: &[&str]) -> String {
    let mut text = HEADER.to_owned();
    for entry in entries {
        text.push_str(&format!("{EVERYONE}:{entry}\n"));
    }
    text
}

#[test]
fn the_stock_deny_entry_of_a_home_library_is_not_an_allow_entry() {
    assert!(!has_allow_entry(&text(&["deny:delete"])));
}

#[test]
fn an_allow_entry_is_one() {
    assert!(has_allow_entry(&text(&["allow:read"])));
}

#[test]
fn flags_after_the_decision_do_not_hide_it() {
    for flags in [
        "allow,file_inherit",
        "allow,inherited",
        "allow,only_inherit,file_inherit,directory_inherit",
    ] {
        assert!(has_allow_entry(&text(&[&format!("{flags}:read")])), "{flags}");
    }
    for flags in ["deny,file_inherit", "deny,inherited"] {
        assert!(!has_allow_entry(&text(&[&format!("{flags}:delete")])), "{flags}");
    }
}

#[test]
fn one_allow_entry_among_deny_entries_is_enough() {
    assert!(!has_allow_entry(&text(&["deny:delete", "deny:write"])));
    assert!(has_allow_entry(&text(&[
        "deny:delete",
        "allow:read",
        "deny:write"
    ])));
}

#[test]
fn an_empty_acl_and_a_header_alone_hold_no_allow_entry() {
    assert!(!has_allow_entry(""));
    assert!(!has_allow_entry(HEADER));
    assert!(!has_allow_entry("\n\n"));
}

#[test]
fn a_name_that_holds_colons_does_not_shift_the_decision() {
    let allow = "user:ABCDEFAB-CDEF-ABCD-EFAB-CDEF00000001:odd:name:501:allow:read\n";
    let deny = "user:ABCDEFAB-CDEF-ABCD-EFAB-CDEF00000001:odd:name:501:deny:read\n";
    assert!(has_allow_entry(&format!("{HEADER}{allow}")));
    assert!(!has_allow_entry(&format!("{HEADER}{deny}")));
}

#[test]
fn an_entry_that_is_not_a_known_decision_counts_as_allowing() {
    assert!(has_allow_entry(&text(&["maybe:read"])));
    assert!(has_allow_entry(&format!("{HEADER}garbage\n")));
    assert!(has_allow_entry(&format!("{HEADER}{EVERYONE}\n")));
}
