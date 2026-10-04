use agentdust_core::digest::{Domain, hmac_sha256, keyed_digest};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn key() -> Vec<u8> {
    (0..32).collect()
}

struct Vector {
    case: u8,
    key: Vec<u8>,
    data: Vec<u8>,
    expected: &'static str,
}

fn rfc4231() -> Vec<Vector> {
    vec![
        Vector {
            case: 1,
            key: vec![0x0b; 20],
            data: b"Hi There".to_vec(),
            expected: "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7",
        },
        Vector {
            case: 2,
            key: b"Jefe".to_vec(),
            data: b"what do ya want for nothing?".to_vec(),
            expected: "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
        },
        Vector {
            case: 3,
            key: vec![0xaa; 20],
            data: vec![0xdd; 50],
            expected: "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe",
        },
        Vector {
            case: 4,
            key: (1..=25).collect(),
            data: vec![0xcd; 50],
            expected: "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b",
        },
        Vector {
            case: 6,
            key: vec![0xaa; 131],
            data: b"Test Using Larger Than Block-Size Key - Hash Key First".to_vec(),
            expected: "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54",
        },
        Vector {
            case: 7,
            key: vec![0xaa; 131],
            data: b"This is a test using a larger than block-size key and a larger than block-size data. The key needs to be hashed before being used by the HMAC algorithm."
                .to_vec(),
            expected: "9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2",
        },
    ]
}

#[test]
fn hmac_sha256_matches_the_rfc_4231_vectors() {
    for vector in rfc4231() {
        assert_eq!(
            hex(&hmac_sha256(&vector.key, &vector.data)),
            vector.expected,
            "test case {}",
            vector.case
        );
    }
}

#[test]
fn hmac_sha256_matches_the_rfc_4231_truncation_vector() {
    let output = hmac_sha256(&[0x0c; 20], b"Test With Truncation");
    assert_eq!(hex(&output[..16]), "a3b6167473100ee06e0c796c2955552b");
}

#[test]
fn hmac_sha256_accepts_an_empty_key_and_empty_data() {
    assert_eq!(hmac_sha256(b"", b"").len(), 32);
    assert_ne!(hmac_sha256(b"", b""), hmac_sha256(b"", b"x"));
}

#[test]
fn the_domain_labels_are_the_spec_strings() {
    assert_eq!(Domain::Session.label(), "AGENTDUST-SESSION-v1");
    assert_eq!(Domain::Cwd.label(), "AGENTDUST-CWD-v1");
}

#[test]
fn keyed_digest_matches_a_known_answer_for_the_cwd_domain() {
    assert_eq!(
        keyed_digest(&key(), Domain::Cwd, b"/Users/dev/project"),
        "3f9f280269f6f22c8a424db31710d7f24117f82909ec019df31ed570626a8110"
    );
}

#[test]
fn keyed_digest_matches_a_known_answer_for_the_session_domain() {
    assert_eq!(
        keyed_digest(&key(), Domain::Session, b"/Users/dev/project"),
        "fea751315cbcd9a175fa16d53febfa7ec8e201403d09538d9abe8703dd482e5f"
    );
}

#[test]
fn keyed_digest_of_empty_data_is_the_digest_of_the_domain_and_a_nul() {
    assert_eq!(
        keyed_digest(&key(), Domain::Cwd, b""),
        "04702efaf92f8a3e869cb9b2a352b9f622a989aa9c7c3cf68e61191b70973df6"
    );
}

#[test]
fn keyed_digest_is_hmac_over_the_domain_a_nul_byte_and_the_data() {
    let data = b"/Users/dev/project";
    assert_eq!(
        keyed_digest(&key(), Domain::Cwd, data),
        hex(&hmac_sha256(&key(), b"AGENTDUST-CWD-v1\0/Users/dev/project"))
    );
    assert_ne!(
        keyed_digest(&key(), Domain::Cwd, data),
        hex(&hmac_sha256(&key(), b"AGENTDUST-CWD-v1/Users/dev/project"))
    );
}

#[test]
fn the_same_data_under_the_two_domains_differs() {
    for data in [
        &b""[..],
        b"x",
        b"/Users/dev/project",
        b"\0",
        b"AGENTDUST-CWD-v1\0",
    ] {
        assert_ne!(
            keyed_digest(&key(), Domain::Session, data),
            keyed_digest(&key(), Domain::Cwd, data),
            "{data:?}"
        );
    }
}

#[test]
fn different_secrets_give_different_digests() {
    let other: Vec<u8> = (1..33).collect();
    assert_ne!(
        keyed_digest(&key(), Domain::Cwd, b"/Users/dev/project"),
        keyed_digest(&other, Domain::Cwd, b"/Users/dev/project")
    );
}

#[test]
fn a_one_bit_difference_in_the_secret_changes_the_digest() {
    let mut other = key();
    other[31] ^= 1;
    assert_ne!(
        keyed_digest(&key(), Domain::Cwd, b"/a"),
        keyed_digest(&other, Domain::Cwd, b"/a")
    );
}

#[test]
fn different_data_gives_different_digests() {
    let digests = [
        keyed_digest(&key(), Domain::Cwd, b"/a"),
        keyed_digest(&key(), Domain::Cwd, b"/a/"),
        keyed_digest(&key(), Domain::Cwd, b"/a\0"),
        keyed_digest(&key(), Domain::Cwd, b"/A"),
        keyed_digest(&key(), Domain::Cwd, b""),
    ];
    for (i, left) in digests.iter().enumerate() {
        for right in &digests[i + 1..] {
            assert_ne!(left, right);
        }
    }
}

#[test]
fn the_same_inputs_give_the_same_digest() {
    assert_eq!(
        keyed_digest(&key(), Domain::Cwd, b"/Users/dev/project"),
        keyed_digest(&key(), Domain::Cwd, b"/Users/dev/project")
    );
}

#[test]
fn the_output_is_64_lowercase_hex_characters() {
    for domain in [Domain::Session, Domain::Cwd] {
        let digest = keyed_digest(&key(), domain, b"/Users/dev/project");
        assert_eq!(digest.len(), 64);
        assert!(
            digest
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        );
    }
}

#[test]
fn the_digest_does_not_contain_the_data_or_the_secret() {
    let digest = keyed_digest(&key(), Domain::Cwd, b"sentinel-project-name");
    assert!(!digest.contains("sentinel"));
    assert!(!digest.contains(&hex(&key())));
}
