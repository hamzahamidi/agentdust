use std::fs;
use std::path::Path;

use agentdust_core::procargs;

#[test]
fn every_corpus_entry_parses_without_panicking() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/procargs");
    let mut seen = 0;
    for entry in fs::read_dir(&dir).unwrap() {
        let data = fs::read(entry.unwrap().path()).unwrap();
        let script = procargs::script_argument(&data);
        match procargs::parse(&data) {
            Ok(parsed) => {
                let _ = procargs::env_value(&parsed, "AGENTDUST_SESSION");
                let found = script.unwrap();
                if let Some(argument) = found {
                    assert!(parsed.args.iter().skip(1).any(|arg| *arg == argument));
                }
            }
            Err(error) => assert_eq!(script, Err(error)),
        }
        seen += 1;
    }
    assert!(seen >= 9, "expected the seeds in {}", dir.display());
}
