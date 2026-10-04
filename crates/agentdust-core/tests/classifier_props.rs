mod classifier_support;
mod inventory_support;

use agentdust_core::class::Class;
use agentdust_core::classifier::Provenance;
use classifier_support::specs::{
    FIXED_PROTECTED, arb_spec, class_at, expected, managed, pid_of, run, scopes,
};
use proptest::collection::vec;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn protected_input_is_never_actionable_and_free_input_follows_the_rules(
        specs in vec(arb_spec(), 1..24),
        provenance in any::<bool>(),
        launchd_known in any::<bool>(),
    ) {
        let scopes = scopes();
        let available = Provenance::Available(&scopes);
        let findings = run(
            &specs,
            if provenance { &available } else { &Provenance::Unavailable },
            launchd_known,
        );
        for pid in FIXED_PROTECTED {
            prop_assert_eq!(class_at(&findings, pid), Class::Managed, "pid {}", pid);
        }
        for (index, spec) in specs.iter().enumerate() {
            let class = class_at(&findings, pid_of(index));
            prop_assert_ne!(class, Class::LikelyOwned);
            prop_assert_eq!(class, expected(spec, provenance, launchd_known), "{:?}", spec);
            if managed(spec, launchd_known) {
                prop_assert!(!matches!(class, Class::OwnedEnded | Class::Suspect), "{:?}", spec);
            }
        }
    }

    #[test]
    fn nothing_is_actionable_without_the_launchd_list(specs in vec(arb_spec(), 1..24)) {
        let scopes = scopes();
        let findings = run(&specs, &Provenance::Available(&scopes), false);
        prop_assert!(findings
            .iter()
            .all(|finding| !matches!(finding.class, Class::OwnedEnded | Class::Suspect)));
    }

    #[test]
    fn no_owned_class_exists_without_provenance(
        specs in vec(arb_spec(), 1..24),
        launchd_known in any::<bool>(),
    ) {
        let findings = run(&specs, &Provenance::Unavailable, launchd_known);
        prop_assert!(findings
            .iter()
            .all(|finding| !matches!(finding.class, Class::OwnedEnded | Class::OwnedLive)));
    }
}
