mod scratch;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

use agentdust_core::cwd::{canonical_cwd, normalize};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use scratch::private_dir;

const MISSING_ROOT: &str = "/agentdust-no-such-root";

fn any_text() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-c./]{0,16}",
        "/[a-c./]{0,16}",
        "/[a-c./\\x00]{0,16}",
        any::<String>(),
    ]
}

fn absolute_text() -> impl Strategy<Value = String> {
    prop_oneof!["/[a-c./]{0,16}", "/[^\\x00]{0,12}"]
}

fn under_a_missing_root() -> impl Strategy<Value = String> {
    (
        prop::collection::vec(
            prop_oneof![Just("a"), Just("b"), Just("c d"), Just("."), Just(".."), Just("")],
            0..8,
        ),
        any::<bool>(),
    )
        .prop_map(|(segments, trailing)| {
            let mut path = format!("{MISSING_ROOT}/{}", segments.join("/"));
            if trailing {
                path.push('/');
            }
            path
        })
}

fn model(path: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                kept.pop();
            }
            name => kept.push(name),
        }
    }
    format!("/{}", kept.join("/"))
}

fn is_normal_form(path: &str) -> bool {
    path.starts_with('/')
        && !path.contains('\0')
        && !path.contains("//")
        && (path == "/" || !path.ends_with('/'))
        && path.split('/').all(|segment| segment != "." && segment != "..")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn normalize_is_idempotent(input in any_text()) {
        if let Some(once) = normalize(&input) {
            prop_assert_eq!(normalize(&once), Some(once));
        }
    }

    #[test]
    fn normalize_refuses_exactly_the_inputs_that_are_not_absolute_or_hold_a_nul(input in any_text()) {
        let refused = !input.starts_with('/') || input.contains('\0');
        prop_assert_eq!(normalize(&input).is_none(), refused);
    }

    #[test]
    fn normalize_gives_the_normal_form_of_the_model(input in absolute_text()) {
        prop_assert_eq!(normalize(&input), Some(model(&input)));
    }

    #[test]
    fn normalize_output_is_in_normal_form_and_never_longer_than_the_input(input in any_text()) {
        if let Some(path) = normalize(&input) {
            prop_assert!(is_normal_form(&path), "{path:?}");
            prop_assert!(path.len() <= input.len(), "{path:?}");
        }
    }

    #[test]
    fn a_path_that_does_not_exist_is_only_normalised(input in under_a_missing_root()) {
        prop_assert_eq!(canonical_cwd(&input), normalize(&input));
    }

    #[test]
    fn canonical_cwd_is_idempotent_and_never_holds_a_nul(input in under_a_missing_root()) {
        let once = canonical_cwd(&input).unwrap();
        prop_assert!(!once.contains('\0'));
        prop_assert!(once.len() <= input.len());
        prop_assert_eq!(canonical_cwd(&once), Some(once));
    }
}

fn build_tree(root: &Path) {
    fs::create_dir_all(root.join("real/sub")).unwrap();
    fs::write(root.join("file"), b"x").unwrap();
    symlink(root.join("real"), root.join("link")).unwrap();
    symlink(root.join("loop-b"), root.join("loop-a")).unwrap();
    symlink(root.join("loop-a"), root.join("loop-b")).unwrap();
}

#[test]
fn canonical_cwd_over_a_real_tree_is_idempotent_and_in_normal_form() {
    let root = private_dir("cwd-prop-tree");
    build_tree(&root);
    let names = prop_oneof![
        Just("real"),
        Just("link"),
        Just("sub"),
        Just("file"),
        Just("loop-a"),
        Just("missing"),
        Just(".."),
        Just("."),
        Just(""),
    ];
    let strategy = (prop::collection::vec(names, 0..7), any::<bool>());
    let mut runner = TestRunner::new(Config::with_cases(512));
    let root_text = root.to_str().unwrap().to_owned();
    runner
        .run(&strategy, |(segments, trailing)| {
            let mut input = format!("{root_text}/{}", segments.join("/"));
            if trailing {
                input.push('/');
            }
            let once = canonical_cwd(&input).unwrap();
            prop_assert!(is_normal_form(&once), "{once:?}");
            let again = canonical_cwd(&once);
            prop_assert_eq!(again.as_deref(), Some(once.as_str()));
            Ok(())
        })
        .unwrap();
    fs::remove_dir_all(&root).unwrap();
}
