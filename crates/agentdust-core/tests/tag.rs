use std::io;

use agentdust_core::digest::{Domain, hmac_sha256, keyed_digest};
use agentdust_core::journal::SessionTagKey;
use agentdust_core::secret::Secret;
use agentdust_core::tag::{ENV_NAME, SessionTag, TAG_BYTES, TAG_LEN};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn secret(byte: u8) -> Secret {
    Secret::from_bytes([byte; 32])
}

fn tag(byte: u8) -> SessionTag {
    SessionTag::from_bytes([byte; TAG_BYTES])
}

#[test]
fn a_tag_is_128_bits_written_as_32_lowercase_hex_characters() {
    assert_eq!(TAG_BYTES, 16);
    assert_eq!(TAG_LEN, 32);
    let text = SessionTag::from_bytes([
        0x00, 0x01, 0x0a, 0xff, 0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xa0, 0xb0, 0xc0,
    ])
    .as_str()
    .to_owned();
    assert_eq!(text, "00010aff102030405060708090a0b0c0");
}

#[test]
fn a_generated_tag_has_the_shape_of_a_tag() {
    let generated = SessionTag::generate().unwrap();
    assert_eq!(generated.as_str().len(), TAG_LEN);
    assert!(
        generated
            .as_str()
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    );
}

#[test]
fn generated_tags_differ() {
    let tags: Vec<String> = (0..16)
        .map(|_| SessionTag::generate().unwrap().as_str().to_owned())
        .collect();
    let mut unique = tags.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), tags.len());
}

#[test]
fn a_tag_comes_from_the_bytes_the_source_gives() {
    let from_source = SessionTag::generate_from(|| Ok([0x42; TAG_BYTES])).unwrap();
    assert_eq!(from_source.as_str(), "42".repeat(TAG_BYTES));
}

#[test]
fn a_source_that_fails_gives_no_tag() {
    let failed = SessionTag::generate_from(|| Err(io::Error::from(io::ErrorKind::PermissionDenied)));
    assert_eq!(failed.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
}

#[test]
fn the_debug_form_of_a_tag_does_not_show_it() {
    let shown = format!("{:?}", tag(0xab));
    assert!(!shown.contains("abab"), "{shown}");
    assert!(shown.contains("redacted"));
}

#[test]
fn the_export_line_names_the_variable_and_ends_in_one_newline() {
    assert_eq!(ENV_NAME, "AGENTDUST_SESSION");
    let t = tag(0x0c);
    assert_eq!(
        t.export_line(),
        format!("export AGENTDUST_SESSION={}\n", "0c".repeat(TAG_BYTES))
    );
}

#[test]
fn the_key_is_the_hmac_of_the_tag_under_the_session_domain() {
    let t = tag(0x5a);
    let s = secret(7);
    let mut message = b"AGENTDUST-SESSION-v1\0".to_vec();
    message.extend_from_slice(t.as_str().as_bytes());
    let expected = hex(&hmac_sha256(&[7; 32], &message));
    assert_eq!(t.key(&s).as_str(), expected);
    assert_eq!(SessionTagKey::try_from(expected.clone()).unwrap(), t.key(&s));
}

#[test]
fn the_key_is_the_digest_a_sampled_process_would_produce() {
    let t = tag(0x33);
    let seen_in_a_process_environment = t.as_str().as_bytes();
    assert_eq!(
        t.key(&secret(9)).as_str(),
        keyed_digest(&[9; 32], Domain::Session, seen_in_a_process_environment)
    );
}

#[test]
fn the_key_depends_on_the_secret_and_on_the_tag() {
    let base = tag(1).key(&secret(1));
    assert_ne!(base, tag(1).key(&secret(2)));
    assert_ne!(base, tag(2).key(&secret(1)));
    assert_eq!(base, tag(1).key(&secret(1)));
}

#[test]
fn the_key_is_not_the_working_directory_digest_of_the_same_text() {
    let t = tag(4);
    let cwd = keyed_digest(&[3; 32], Domain::Cwd, t.as_str().as_bytes());
    assert_ne!(t.key(&secret(3)).as_str(), cwd);
}

#[test]
fn the_key_does_not_contain_the_tag() {
    let t = tag(0x6e);
    assert!(!t.key(&secret(1)).as_str().contains(t.as_str()));
}
