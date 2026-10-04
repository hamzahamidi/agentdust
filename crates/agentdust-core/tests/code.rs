use std::collections::HashSet;
use std::io;
use std::time::Duration;

use agentdust_core::code::{ALPHABET, Check, DRAW_BYTES, LEN, TTL, check, generate, generate_with};

#[test]
fn the_alphabet_has_25_unambiguous_symbols() {
    assert_eq!(ALPHABET.as_slice(), b"ACDEFGHJKMNPQRTUVWXY34679");
    let distinct: HashSet<u8> = ALPHABET.iter().copied().collect();
    assert_eq!(distinct.len(), 25);
    for ambiguous in b"01258BILOSZ" {
        assert!(!ALPHABET.contains(ambiguous), "{}", *ambiguous as char);
    }
}

#[test]
fn a_code_has_four_symbols_and_lives_two_minutes() {
    assert_eq!(LEN, 4);
    assert_eq!(TTL, Duration::from_secs(120));
    assert_eq!(DRAW_BYTES, 8);
}

#[test]
fn generated_codes_use_the_alphabet_and_differ() {
    let mut seen = HashSet::new();
    for _ in 0..200 {
        let code = generate().unwrap();
        assert_eq!(code.len(), LEN);
        assert!(code.bytes().all(|byte| ALPHABET.contains(&byte)), "{code}");
        seen.insert(code);
    }
    assert!(seen.len() >= 190, "{} distinct codes of 200", seen.len());
}

#[test]
fn every_symbol_appears_about_equally_often() {
    let mut counts = [0usize; 25];
    for _ in 0..2000 {
        for byte in generate().unwrap().bytes() {
            counts[ALPHABET.iter().position(|symbol| *symbol == byte).unwrap()] += 1;
        }
    }
    for (index, count) in counts.iter().enumerate() {
        assert!(
            (200..=440).contains(count),
            "symbol {index} appeared {count} times"
        );
    }
}

#[test]
fn bytes_of_250_and_above_are_discarded_so_the_draw_stays_uniform() {
    let mut draws = vec![[255, 250, 0, 24, 25, 49, 50, 100]].into_iter();
    let code = generate_with(&mut || Ok(draws.next().unwrap())).unwrap();
    assert_eq!(code, "A9A9");
}

#[test]
fn a_draw_that_yields_too_few_symbols_is_followed_by_another() {
    let mut draws = vec![[251; DRAW_BYTES], [1, 2, 251, 3, 252, 4, 5, 6]].into_iter();
    let code = generate_with(&mut || Ok(draws.next().unwrap())).unwrap();
    assert_eq!(code, "CDEF");
}

#[test]
fn a_failing_source_fails_the_generation() {
    let failed = generate_with(&mut || Err(io::Error::other("no entropy")));
    assert!(failed.is_err());
}

#[test]
fn only_an_exact_code_approves() {
    assert_eq!(check(Some("AC3K"), "AC3K"), Check::Approved);
    assert_eq!(check(Some("ac3k"), "AC3K"), Check::Wrong);
    assert_eq!(check(Some(" AC3K"), "AC3K"), Check::Wrong);
    assert_eq!(check(Some("AC3K\n"), "AC3K"), Check::Wrong);
    assert_eq!(check(Some("AC3KA"), "AC3K"), Check::Wrong);
    assert_eq!(check(Some("AC3"), "AC3K"), Check::Wrong);
    assert_eq!(check(Some("   "), "AC3K"), Check::Wrong);
}

#[test]
fn a_missing_or_empty_answer_is_empty() {
    assert_eq!(check(Some(""), "AC3K"), Check::Empty);
    assert_eq!(check(None, "AC3K"), Check::Empty);
}

#[test]
fn an_empty_expected_code_never_approves() {
    assert_eq!(check(Some(""), ""), Check::Empty);
    assert_eq!(check(None, ""), Check::Empty);
}
