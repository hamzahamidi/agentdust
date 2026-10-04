use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fuzz_dir() -> PathBuf {
    repository().join("fuzz")
}

fn declared_targets() -> Vec<(String, String)> {
    let manifest = fs::read_to_string(fuzz_dir().join("Cargo.toml")).unwrap();
    let mut targets = Vec::new();
    let mut name = None;
    let mut in_bin = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_bin = line == "[[bin]]";
            name = None;
        } else if in_bin {
            if let Some(value) = line.strip_prefix("name = ") {
                name = Some(value.trim_matches('"').to_owned());
            } else if let (Some(value), Some(name)) = (line.strip_prefix("path = "), name.take()) {
                targets.push((name, value.trim_matches('"').to_owned()));
            }
        }
    }
    targets
}

fn seed_dirs() -> BTreeSet<String> {
    fs::read_dir(fuzz_dir().join("seeds"))
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().unwrap().is_dir())
        .map(|entry| entry.file_name().into_string().unwrap())
        .collect()
}

#[test]
fn every_fuzz_target_has_a_seed_directory_and_every_seed_directory_a_target() {
    let targets: BTreeSet<String> = declared_targets().into_iter().map(|(name, _)| name).collect();
    assert_eq!(targets, seed_dirs());
}

#[test]
fn the_parsers_the_spec_names_are_fuzz_targets() {
    let targets: BTreeSet<String> = declared_targets().into_iter().map(|(name, _)| name).collect();
    for name in ["procargs", "journal_decode", "journal_read", "claude_payload"] {
        assert!(targets.contains(name), "no fuzz target {name}");
    }
}

#[test]
fn every_fuzz_target_is_a_libfuzzer_entry_point_in_an_existing_file() {
    for (name, path) in declared_targets() {
        let source =
            fs::read_to_string(fuzz_dir().join(&path)).unwrap_or_else(|err| panic!("{name}: {path}: {err}"));
        assert!(source.contains("fuzz_target!"), "{name}");
    }
}

#[test]
fn every_seed_directory_holds_enough_small_seeds() {
    for name in seed_dirs() {
        let sizes: Vec<u64> = fs::read_dir(fuzz_dir().join("seeds").join(&name))
            .unwrap()
            .map(|entry| entry.unwrap().metadata().unwrap().len())
            .collect();
        assert!(sizes.len() >= 5, "{name}: {} seeds", sizes.len());
        assert!(
            sizes.iter().all(|size| *size <= 128 * 1024),
            "{name}: a seed over 128 KiB"
        );
    }
}
