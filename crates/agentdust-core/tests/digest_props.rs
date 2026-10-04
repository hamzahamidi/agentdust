use agentdust_core::digest::{Domain, hmac_sha256, keyed_digest};
use proptest::prelude::*;

fn domain() -> impl Strategy<Value = Domain> {
    prop_oneof![Just(Domain::Session), Just(Domain::Cwd)]
}

fn key() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..160)
}

fn secret() -> impl Strategy<Value = [u8; 32]> {
    prop::array::uniform32(any::<u8>())
}

fn data() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..200)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn the_output_is_always_64_lowercase_hex_characters(key in key(), domain in domain(), data in data()) {
        let digest = keyed_digest(&key, domain, &data);
        prop_assert_eq!(digest.len(), 64);
        prop_assert!(digest.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')));
    }

    #[test]
    fn the_digest_is_hmac_over_label_nul_and_data(key in key(), domain in domain(), data in data()) {
        let mut message = domain.label().as_bytes().to_vec();
        message.push(0);
        message.extend_from_slice(&data);
        prop_assert_eq!(keyed_digest(&key, domain, &data), hex(&hmac_sha256(&key, &message)));
    }

    #[test]
    fn the_domains_never_collide_on_the_same_input(key in key(), data in data()) {
        prop_assert_ne!(
            keyed_digest(&key, Domain::Session, &data),
            keyed_digest(&key, Domain::Cwd, &data)
        );
    }

    #[test]
    fn different_data_gives_different_digests(key in key(), domain in domain(), left in data(), right in data()) {
        prop_assume!(left != right);
        prop_assert_ne!(keyed_digest(&key, domain, &left), keyed_digest(&key, domain, &right));
    }

    #[test]
    fn different_install_secrets_give_different_digests(left in secret(), right in secret(), domain in domain(), data in data()) {
        prop_assume!(left != right);
        prop_assert_ne!(keyed_digest(&left, domain, &data), keyed_digest(&right, domain, &data));
    }
}
