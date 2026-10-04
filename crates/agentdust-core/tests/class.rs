use agentdust_core::class::{Actionability, Class, actionable_as};

const SPEC_TABLE: [(Class, &str); 6] = [
    (Class::Managed, "managed"),
    (Class::OwnedLive, "owned-live"),
    (Class::OwnedEnded, "owned-ended"),
    (Class::LikelyOwned, "likely-owned"),
    (Class::Suspect, "suspect"),
    (Class::Unknown, "unknown"),
];

#[test]
fn managed_is_never_actionable() {
    assert_eq!(actionable_as(Class::Managed), Actionability::Never);
}

#[test]
fn owned_live_is_never_actionable() {
    assert_eq!(actionable_as(Class::OwnedLive), Actionability::Never);
}

#[test]
fn owned_ended_is_actionable_with_one_batch_code() {
    assert_eq!(actionable_as(Class::OwnedEnded), Actionability::BatchCode);
}

#[test]
fn likely_owned_is_report_only() {
    assert_eq!(actionable_as(Class::LikelyOwned), Actionability::ReportOnly);
}

#[test]
fn suspect_is_actionable_with_one_code_per_item() {
    assert_eq!(actionable_as(Class::Suspect), Actionability::PerItemCode);
}

#[test]
fn unknown_is_never_actionable() {
    assert_eq!(actionable_as(Class::Unknown), Actionability::Never);
}

#[test]
fn only_owned_ended_and_suspect_can_ever_be_signalled() {
    for class in Class::ALL {
        let signalled = matches!(
            actionable_as(class),
            Actionability::BatchCode | Actionability::PerItemCode
        );
        assert_eq!(
            signalled,
            matches!(class, Class::OwnedEnded | Class::Suspect),
            "{class}"
        );
    }
}

#[test]
fn all_lists_the_classes_in_the_order_the_spec_checks_them() {
    let names: Vec<&str> = SPEC_TABLE.iter().map(|(_, name)| *name).collect();
    let listed: Vec<&str> = Class::ALL.iter().map(|class| class.as_str()).collect();
    assert_eq!(listed, names);
    let classes: Vec<Class> = SPEC_TABLE.iter().map(|(class, _)| *class).collect();
    assert_eq!(Class::ALL.to_vec(), classes);
}

#[test]
fn each_class_serialises_to_the_name_in_the_spec_table() {
    for (class, name) in SPEC_TABLE {
        assert_eq!(serde_json::to_string(&class).unwrap(), format!("\"{name}\""));
    }
}

#[test]
fn each_name_in_the_spec_table_deserialises_to_its_class() {
    for (class, name) in SPEC_TABLE {
        let parsed: Class = serde_json::from_str(&format!("\"{name}\"")).unwrap();
        assert_eq!(parsed, class);
    }
}

#[test]
fn display_and_as_str_use_the_serialised_name() {
    for (class, name) in SPEC_TABLE {
        assert_eq!(class.as_str(), name);
        assert_eq!(class.to_string(), name);
    }
}

#[test]
fn names_that_are_not_in_the_spec_table_are_refused() {
    for name in [
        "owned_live",
        "ownedLive",
        "OwnedLive",
        "Managed",
        "SUSPECT",
        "owned",
        "likely",
        "",
        " suspect",
        "suspect ",
    ] {
        let parsed = serde_json::from_str::<Class>(&format!("\"{name}\""));
        assert!(parsed.is_err(), "{name:?} was accepted");
    }
}
