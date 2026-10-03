use agentdust_mcp::probe::{CODE_ALPHABET, CODE_LEN, Outcome, check_code, new_code};

#[test]
fn codes_use_the_unambiguous_alphabet() {
    for _ in 0..200 {
        let code = new_code().unwrap();
        assert_eq!(code.len(), CODE_LEN);
        assert!(code.bytes().all(|b| CODE_ALPHABET.contains(&b)), "{code}");
    }
}

#[test]
fn only_an_exact_code_approves() {
    assert_eq!(check_code(Some("AC3K"), "AC3K"), Outcome::Approved);
    assert_eq!(check_code(Some("ac3k"), "AC3K"), Outcome::WrongCode);
    assert_eq!(check_code(Some(" AC3K"), "AC3K"), Outcome::WrongCode);
    assert_eq!(check_code(Some(""), "AC3K"), Outcome::Empty);
    assert_eq!(check_code(None, "AC3K"), Outcome::Empty);
}
