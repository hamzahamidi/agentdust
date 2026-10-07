#![cfg(target_os = "macos")]

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use agentdust_core::journal::{Agent, AgentIdentity, Kind, Record, SCHEMA_VERSION, append};
use agentdust_core::revalidate::Revalidation;
use agentdust_core::secret::load_or_create;
use agentdust_core::tag::SessionTag;
use agentdust_testkit::harness::Harness;
use agentdust_testkit::spec::ProcSpec;
use agentdust_testkit::wait_until;
use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientConfig, ElicitRequestParams, ElicitResult,
    ElicitationAction, Implementation, ProtocolVersion,
};
use rmcp::service::{ClientLifecycleMode, ClientServiceExt, RequestContext, RoleClient};
use rmcp::transport::TokioChildProcess;
use rmcp::{ClientHandler, ErrorData};
use serde_json::{Value, json};
use tokio::process::Command;

struct ApproveCode;

impl ClientHandler for ApproveCode {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::new(
            ClientCapabilities::builder().enable_elicitation().build(),
            Implementation::new("agentdust-brew-acceptance", "0.0.0"),
        )
        .with_protocol_version(ProtocolVersion::V_2026_07_28)
    }

    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        let ElicitRequestParams::FormElicitationParams { message, .. } = request else {
            return Ok(ElicitResult::new(ElicitationAction::Decline));
        };
        let code = message
            .rsplit_once("Type ")
            .unwrap()
            .1
            .split_whitespace()
            .next()
            .unwrap();
        Ok(ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "code": code })))
    }
}

fn text_json(result: &rmcp::model::CallToolResult) -> Value {
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[tokio::test]
#[ignore = "runs the Homebrew binary against harness created processes in release dry run"]
async fn homebrew_binary_stops_only_the_approved_harness_process() {
    let binary = PathBuf::from(std::env::var_os("AGENTDUST_TEST_BINARY").unwrap());
    let fixture = PathBuf::from(std::env::var_os("AGENTDUST_FIXTURE_SLEEPER").unwrap());
    let data_dir = PathBuf::from(std::env::var_os("AGENTDUST_DATA_DIR").unwrap());
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let claude_config = PathBuf::from(std::env::var_os("CLAUDE_CONFIG_DIR").unwrap());
    let secret = load_or_create(&data_dir).unwrap();
    let tag = SessionTag::from_bytes([0xd4; 16]);
    let mut harness = Harness::new(fixture).unwrap();
    let tree = harness
        .spawn_tree(
            &ProcSpec::new()
                .seconds(120)
                .spawn(2)
                .env("AGENTDUST_SESSION", tag.as_str()),
        )
        .unwrap();
    let identity = harness.identity(tree.parent).clone();
    append(
        &data_dir,
        &Record {
            v: SCHEMA_VERSION,
            kind: Kind::SessionStart,
            agent: Agent::Claude,
            session_id: "homebrew-acceptance".to_owned(),
            subagent_id: None,
            agent_identity: Some(AgentIdentity::from_process(&identity).unwrap()),
            tool_use_id: None,
            wall_ts: 1,
            mono_ts: 1,
            boot: identity.kernel.boot_session_uuid.clone(),
            session_tag_key: Some(tag.key(&secret)),
            cwd_key: None,
            exe_base: None,
        },
    )
    .unwrap();
    harness.orphan(tree.parent).unwrap();

    let target = tree.children[0];
    let target_pid = harness.pid(target);
    let sibling = tree.children[1];
    let sibling_pid = harness.pid(sibling);
    let mut mcp = Command::new(binary);
    mcp.arg("mcp")
        .env("HOME", home)
        .env("CLAUDE_CONFIG_DIR", claude_config)
        .env("AGENTDUST_DATA_DIR", &data_dir);
    let service = ApproveCode
        .serve_with_lifecycle(
            TokioChildProcess::new(mcp).unwrap(),
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .unwrap();

    let plan = service
        .call_tool(CallToolRequestParams::new("agentdust_plan"))
        .await
        .unwrap();
    let plan = text_json(&plan);
    let owned_pids = HashSet::from([target_pid, sibling_pid]);
    let item_ids: Vec<String> = plan["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| owned_pids.contains(&(item["pid"].as_i64().unwrap() as i32)))
        .filter(|item| item["pid"].as_i64().unwrap() == target_pid as i64)
        .map(|item| item["item_id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(item_ids.len(), 1, "{plan}");

    let args = json!({ "plan_id": plan["plan_id"], "item_ids": item_ids });
    let applied = service
        .call_tool(
            CallToolRequestParams::new("agentdust_apply").with_arguments(args.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    let report = text_json(&applied);
    assert_eq!(report["items"][0]["result"], "terminated", "{report}");
    service.cancel().await.unwrap();

    assert!(wait_until(
        || harness.revalidate(target) == Revalidation::Gone,
        Duration::from_secs(30)
    ));
    assert_eq!(harness.revalidate(sibling), Revalidation::Match);
    harness.shutdown().unwrap();
}
