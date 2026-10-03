use std::fs;
use std::path::Path;

use agentdust_core::procargs;

#[test]
fn every_corpus_entry_parses_without_panicking() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/procargs");
    let mut seen = 0;
    for entry in fs::read_dir(&dir).unwrap() {
        let data = fs::read(entry.unwrap().path()).unwrap();
        if let Ok(parsed) = procargs::parse(&data) {
            let _ = procargs::env_value(&parsed, "AGENTDUST_SESSION");
        }
        seen += 1;
    }
    assert!(seen >= 5, "expected the seeds in {}", dir.display());
}
