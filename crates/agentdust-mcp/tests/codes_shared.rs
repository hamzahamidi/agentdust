use agentdust_core::code;
use agentdust_mcp::probe::{CODE_ALPHABET, CODE_LEN, Outcome, check_code, new_code};

#[test]
fn the_probe_uses_the_shared_alphabet_and_length() {
    assert_eq!(CODE_ALPHABET, code::ALPHABET);
    assert_eq!(CODE_LEN, code::LEN);
}

#[test]
fn the_probe_decides_like_the_shared_check() {
    let answers = [
        Some("AC3K"),
        Some("ac3k"),
        Some(" AC3K"),
        Some(""),
        None,
        Some("AC3KK"),
    ];
    for answer in answers {
        let expected = match code::check(answer, "AC3K") {
            code::Check::Approved => Outcome::Approved,
            code::Check::Wrong => Outcome::WrongCode,
            code::Check::Empty => Outcome::Empty,
        };
        assert_eq!(check_code(answer, "AC3K"), expected, "{answer:?}");
    }
}

#[test]
fn the_probe_draws_codes_the_shared_check_accepts() {
    for _ in 0..50 {
        let drawn = new_code().unwrap();
        assert_eq!(code::check(Some(&drawn), &drawn), code::Check::Approved);
    }
}
