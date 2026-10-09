mod setup_support;

use serde_json::Value;
use setup_support::{Sandbox, code};

#[test]
fn json_metrics_are_empty_without_creating_the_data_directory() {
    let sandbox = Sandbox::new("metrics-cli");
    let output = sandbox.run(&["metrics", "--json"]);
    assert_eq!(code(&output), 0);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["event_count"], 0);
    assert!(value["daily"].as_array().unwrap().is_empty());
    assert!(!sandbox.data.exists());
}

#[test]
fn human_metrics_explain_when_no_outcomes_exist() {
    let sandbox = Sandbox::new("metrics-human");
    let output = sandbox.run(&["metrics"]);
    assert_eq!(code(&output), 0);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "No cleanup outcomes recorded yet."
    );
}
