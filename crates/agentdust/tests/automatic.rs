mod setup_support;

use serde_json::Value;
use setup_support::{Sandbox, code};

#[test]
fn policy_mutations_refuse_non_interactive_callers_without_writing_state() {
    for args in [
        vec!["auto", "enable", "/tmp"],
        vec!["auto", "pause"],
        vec!["auto", "resume"],
        vec!["auto", "disable"],
        vec!["auto", "keep", "1"],
        vec!["auto", "unkeep", "1"],
    ] {
        let sandbox = Sandbox::new("auto-no-tty");
        let output = sandbox.run(&args);
        assert_eq!(code(&output), 1);
        assert!(String::from_utf8_lossy(&output.stderr).contains("human foreground terminal"));
        assert!(!sandbox.data.exists());
        assert!(
            !sandbox
                .home
                .join("Library/LaunchAgents/com.agentdust.automatic.plist")
                .exists()
        );
    }
}

#[test]
fn status_and_a_disabled_worker_create_no_state() {
    let sandbox = Sandbox::new("auto-read-only");
    let status = sandbox.run(&["auto", "status"]);
    assert_eq!(code(&status), 0);
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["enabled"], false);
    assert_eq!(status["worker_running"], false);
    assert_eq!(code(&sandbox.run(&["auto", "worker"])), 0);
    assert!(!sandbox.data.exists());
}

#[tokio::test]
async fn mcp_exposes_read_only_status_and_no_policy_mutation() {
    use rmcp::ServiceExt;
    use rmcp::model::CallToolRequestParams;
    use rmcp::transport::TokioChildProcess;
    let sandbox = Sandbox::new("auto-mcp");
    let mut command = tokio::process::Command::new(setup_support::BIN);
    command
        .args(["mcp"])
        .env("HOME", &sandbox.home)
        .env("AGENTDUST_DATA_DIR", &sandbox.data);
    let service = ().serve(TokioChildProcess::new(command).unwrap()).await.unwrap();
    let listed = service.list_all_tools().await.unwrap();
    let status_tool = listed
        .iter()
        .find(|tool| tool.name == "agentdust_auto_status")
        .unwrap();
    let metrics_tool = listed
        .iter()
        .find(|tool| tool.name == "agentdust_metrics")
        .unwrap();
    assert_eq!(
        status_tool.annotations.as_ref().unwrap().read_only_hint,
        Some(true)
    );
    assert_eq!(
        metrics_tool.annotations.as_ref().unwrap().read_only_hint,
        Some(true)
    );
    assert!(listed.iter().all(|tool| {
        ![
            "agentdust_auto_enable",
            "agentdust_auto_resume",
            "agentdust_auto_unkeep",
        ]
        .contains(&tool.name.as_ref())
    }));
    let result = service
        .call_tool(CallToolRequestParams::new("agentdust_auto_status"))
        .await
        .unwrap();
    let value: Value = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(value["enabled"], false);
    let metrics = service
        .call_tool(CallToolRequestParams::new("agentdust_metrics"))
        .await
        .unwrap();
    let metrics: Value = serde_json::from_str(&metrics.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(metrics["event_count"], 0);
    service.cancel().await.unwrap();
    assert!(!sandbox.data.join("automatic").exists());
}

#[test]
fn unattended_enable_validates_all_paths_without_writing_state() {
    let sandbox = Sandbox::new("auto-invalid-batch");
    for args in [
        vec!["auto", "enable", "--yes", "/tmp", "/"],
        vec!["auto", "enable", "--yes", "/tmp", "/agentdust-missing-project"],
        vec!["auto", "enable", "--yes", "--projects", "/"],
        vec!["auto", "enable", "--yes"],
        vec!["auto", "pause", "--yes"],
        vec!["auto", "unkeep", "--yes", "1"],
    ] {
        assert_eq!(code(&sandbox.run(&args)), 1);
        assert!(!sandbox.data.exists());
        assert!(
            !sandbox
                .home
                .join("Library/LaunchAgents/com.agentdust.automatic.plist")
                .exists()
        );
    }
}
