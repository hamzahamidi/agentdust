use agentdust_mcp::ProbeServer;
use agentdust_mcp::probe::{INPUT_KEY, Outcome, ProbeReport, TOOL_NAME};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ClientCapabilities, ClientConfig,
    ElicitRequestParams, ElicitResult, ElicitationAction, Implementation, InputRequest, InputResponses,
    ProtocolVersion,
};
use rmcp::service::{ClientLifecycleMode, ClientServiceExt, RequestContext, RoleClient, RunningService};
use rmcp::{ClientHandler, ErrorData, ServiceExt};
use serde_json::json;

#[derive(Clone)]
enum Reply {
    EchoCode,
    Code(&'static str),
    Decline,
    Cancel,
}

#[derive(Clone)]
struct ScriptedClient {
    reply: Reply,
    protocol: ProtocolVersion,
    elicitation: bool,
}

impl ScriptedClient {
    fn new(protocol: ProtocolVersion, reply: Reply) -> Self {
        Self {
            reply,
            protocol,
            elicitation: true,
        }
    }
}

fn code_in(message: &str) -> String {
    message.split_whitespace().nth(1).unwrap().to_owned()
}

impl ClientHandler for ScriptedClient {
    fn get_info(&self) -> ClientConfig {
        let capabilities = if self.elicitation {
            ClientCapabilities::builder().enable_elicitation().build()
        } else {
            ClientCapabilities::default()
        };
        ClientConfig::new(capabilities, Implementation::new("scripted-client", "0.0.0"))
            .with_protocol_version(self.protocol.clone())
    }

    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        let ElicitRequestParams::FormElicitationParams { message, .. } = &request else {
            return Ok(ElicitResult::new(ElicitationAction::Decline));
        };
        Ok(match &self.reply {
            Reply::EchoCode => {
                ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "code": code_in(message) }))
            }
            Reply::Code(code) => {
                ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "code": code }))
            }
            Reply::Decline => ElicitResult::new(ElicitationAction::Decline),
            Reply::Cancel => ElicitResult::new(ElicitationAction::Cancel),
        })
    }
}

async fn connect(client: ScriptedClient) -> RunningService<RoleClient, ScriptedClient> {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let server = ProbeServer::default().serve(server_io).await.unwrap();
        let _ = server.waiting().await;
    });
    if client.protocol == ProtocolVersion::V_2026_07_28 {
        let lifecycle = ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        };
        client.serve_with_lifecycle(client_io, lifecycle).await.unwrap()
    } else {
        client.serve(client_io).await.unwrap()
    }
}

fn report_of(result: &CallToolResult) -> ProbeReport {
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

async fn probe(client: ScriptedClient) -> ProbeReport {
    let client = connect(client).await;
    let result = client
        .call_tool(CallToolRequestParams::new(TOOL_NAME))
        .await
        .unwrap();
    client.cancel().await.unwrap();
    report_of(&result)
}

const RETRY: ProtocolVersion = ProtocolVersion::V_2026_07_28;
const LEGACY: ProtocolVersion = ProtocolVersion::V_2025_06_18;

#[tokio::test(flavor = "multi_thread")]
async fn retry_based_client_with_the_right_code_is_approved() {
    let report = probe(ScriptedClient::new(RETRY, Reply::EchoCode)).await;
    assert_eq!(report.outcome, Outcome::Approved);
    assert_eq!(report.path, "retry");
    assert_eq!(report.protocol, "2026-07-28");
}

#[tokio::test(flavor = "multi_thread")]
async fn retry_based_client_with_a_wrong_code_is_refused() {
    let report = probe(ScriptedClient::new(RETRY, Reply::Code("ZZZZ"))).await;
    assert_eq!(report.outcome, Outcome::WrongCode);
}

#[tokio::test(flavor = "multi_thread")]
async fn retry_based_client_with_an_empty_code_is_refused() {
    let report = probe(ScriptedClient::new(RETRY, Reply::Code(""))).await;
    assert_eq!(report.outcome, Outcome::Empty);
}

#[tokio::test(flavor = "multi_thread")]
async fn retry_based_decline_and_cancel_are_reported() {
    assert_eq!(
        probe(ScriptedClient::new(RETRY, Reply::Decline)).await.outcome,
        Outcome::Declined
    );
    assert_eq!(
        probe(ScriptedClient::new(RETRY, Reply::Cancel)).await.outcome,
        Outcome::Cancelled
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn legacy_client_with_the_right_code_is_approved() {
    let report = probe(ScriptedClient::new(LEGACY, Reply::EchoCode)).await;
    assert_eq!(report.outcome, Outcome::Approved);
    assert_eq!(report.path, "legacy");
}

#[tokio::test(flavor = "multi_thread")]
async fn legacy_decline_wrong_and_empty_codes_are_refused() {
    assert_eq!(
        probe(ScriptedClient::new(LEGACY, Reply::Decline)).await.outcome,
        Outcome::Declined
    );
    assert_eq!(
        probe(ScriptedClient::new(LEGACY, Reply::Code("ZZZZ")))
            .await
            .outcome,
        Outcome::WrongCode
    );
    assert_eq!(
        probe(ScriptedClient::new(LEGACY, Reply::Code(""))).await.outcome,
        Outcome::Empty
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_without_elicitation_is_unsupported() {
    for protocol in [RETRY, LEGACY] {
        let mut client = ScriptedClient::new(protocol, Reply::EchoCode);
        client.elicitation = false;
        assert_eq!(probe(client).await.outcome, Outcome::Unsupported);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replayed_request_state_is_rejected() {
    let client = connect(ScriptedClient::new(RETRY, Reply::EchoCode)).await;
    let first = client
        .call_tool_once(CallToolRequestParams::new(TOOL_NAME))
        .await
        .unwrap();
    let CallToolResponse::InputRequired(required) = first else {
        panic!("expected an input request");
    };
    let state = required.request_state.clone().unwrap();
    let Some(InputRequest::Elicitation(elicit)) = required.input_requests.unwrap().remove(INPUT_KEY) else {
        panic!("expected an elicitation");
    };
    let ElicitRequestParams::FormElicitationParams { message, .. } = elicit.params else {
        panic!("expected a form elicitation");
    };
    let answer =
        ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "code": code_in(&message) }));
    let responses: InputResponses = [(INPUT_KEY.to_owned(), serde_json::to_value(answer).unwrap())].into();
    let retry = CallToolRequestParams::new(TOOL_NAME)
        .with_request_state(state)
        .with_input_responses(responses);
    let CallToolResponse::Complete(result) = client.call_tool_once(retry.clone()).await.unwrap() else {
        panic!("expected a final result");
    };
    assert_eq!(report_of(&result).outcome, Outcome::Approved);
    assert!(client.call_tool_once(retry).await.is_err());
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_request_state_is_rejected() {
    let client = connect(ScriptedClient::new(RETRY, Reply::EchoCode)).await;
    let forged = CallToolRequestParams::new(TOOL_NAME).with_request_state("00ff");
    assert!(client.call_tool_once(forged).await.is_err());
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_tool_is_an_error() {
    let client = connect(ScriptedClient::new(RETRY, Reply::EchoCode)).await;
    assert!(
        client
            .call_tool_once(CallToolRequestParams::new("agentdust_apply"))
            .await
            .is_err()
    );
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_retry_without_the_approval_response_is_rejected() {
    let client = connect(ScriptedClient::new(RETRY, Reply::EchoCode)).await;
    let CallToolResponse::InputRequired(required) = client
        .call_tool_once(CallToolRequestParams::new(TOOL_NAME))
        .await
        .unwrap()
    else {
        panic!("expected an input request");
    };
    let state = required.request_state.unwrap();
    let wrong_key: InputResponses = [(
        "other".to_owned(),
        serde_json::to_value(ElicitResult::new(ElicitationAction::Accept)).unwrap(),
    )]
    .into();
    let retry = CallToolRequestParams::new(TOOL_NAME)
        .with_request_state(state)
        .with_input_responses(wrong_key);
    assert!(client.call_tool_once(retry).await.is_err());
    client.cancel().await.unwrap();
}
