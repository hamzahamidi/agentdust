use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ElicitRequest,
    ElicitRequestParams, ElicitResult, ElicitationAction, ElicitationSchema, Implementation, InputRequest,
    InputRequests, InputRequiredResult, JsonObject, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
    ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
};
use rmcp::schemars::JsonSchema;
use rmcp::service::{RequestContext, RoleServer, ServiceError};
use rmcp::{ErrorData, ServerHandler};
use serde::{Deserialize, Serialize};

pub const TOOL_NAME: &str = "agentdust_probe_approval";
pub const INPUT_KEY: &str = "approval";
pub const CODE_ALPHABET: &[u8; 25] = b"ACDEFGHJKMNPQRTUVWXY34679";
pub const CODE_LEN: usize = 4;
const CODE_TTL: Duration = Duration::from_secs(120);

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ApprovalCode {
    #[schemars(description = "The code shown in the message")]
    pub code: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Approved,
    WrongCode,
    Empty,
    Declined,
    Cancelled,
    Expired,
    Unsupported,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeReport {
    pub outcome: Outcome,
    pub protocol: String,
    pub path: String,
}

struct Pending {
    code: String,
    expires: Instant,
}

#[derive(Clone, Default)]
pub struct ProbeServer {
    pending: Arc<Mutex<HashMap<String, Pending>>>,
}

impl ServerHandler for ProbeServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("agentdust", env!("CARGO_PKG_VERSION")))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(vec![probe_tool()])
            .with_ttl_ms(0)
            .with_cache_scope(CacheScope::Private))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name != TOOL_NAME {
            return Err(ErrorData::invalid_params(
                format!("unknown tool {}", request.name),
                None,
            ));
        }
        let protocol = context.protocol_version();
        let supports_form = context
            .client_capabilities()
            .and_then(|caps| caps.elicitation)
            .is_some_and(|e| e.form.is_some() || e.url.is_none());
        if !supports_form {
            return Ok(report(Outcome::Unsupported, protocol.as_ref(), "none"));
        }
        let retry_based = protocol
            .as_ref()
            .is_some_and(|v| v.as_str() >= ProtocolVersion::V_2026_07_28.as_str());
        if retry_based {
            self.call_retry_based(request, protocol.as_ref())
        } else {
            Ok(self.call_legacy(&context, protocol.as_ref()).await)
        }
    }
}

impl ProbeServer {
    fn call_retry_based(
        &self,
        request: CallToolRequestParams,
        protocol: Option<&ProtocolVersion>,
    ) -> Result<CallToolResponse, ErrorData> {
        let Some(nonce) = request.request_state else {
            let code = new_code().map_err(internal)?;
            let nonce = hex(&random_bytes::<16>().map_err(internal)?);
            let message = approval_message(&code);
            self.pending.lock().expect("pending map lock").insert(
                nonce.clone(),
                Pending {
                    code,
                    expires: Instant::now() + CODE_TTL,
                },
            );
            let schema = approval_schema().map_err(internal)?;
            let mut requests = InputRequests::new();
            requests.insert(
                INPUT_KEY.to_owned(),
                InputRequest::Elicitation(ElicitRequest::new(ElicitRequestParams::FormElicitationParams {
                    meta: None,
                    message,
                    requested_schema: schema,
                })),
            );
            return Ok(InputRequiredResult::new(Some(requests), Some(nonce)).into());
        };
        let pending = self
            .pending
            .lock()
            .expect("pending map lock")
            .remove(&nonce)
            .ok_or_else(|| ErrorData::invalid_params("unknown or already used request state", None))?;
        if Instant::now() > pending.expires {
            return Ok(report(Outcome::Expired, protocol, "retry"));
        }
        let response = request
            .input_responses
            .as_ref()
            .and_then(|responses| responses.get(INPUT_KEY))
            .ok_or_else(|| ErrorData::invalid_params("missing approval response", None))?;
        let response = ElicitResult::deserialize(response)
            .map_err(|_| ErrorData::invalid_params("invalid approval response", None))?;
        Ok(report(outcome_of(&response, &pending.code), protocol, "retry"))
    }

    async fn call_legacy(
        &self,
        context: &RequestContext<RoleServer>,
        protocol: Option<&ProtocolVersion>,
    ) -> CallToolResponse {
        let (Ok(code), Ok(schema)) = (new_code(), approval_schema()) else {
            return report(Outcome::Failed, protocol, "legacy");
        };
        let request = ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: approval_message(&code),
            requested_schema: schema,
        };
        let outcome = match context
            .peer
            .create_elicitation_with_timeout(request, Some(CODE_TTL))
            .await
        {
            Ok(response) => outcome_of(&response, &code),
            Err(ServiceError::Timeout { .. }) => Outcome::Expired,
            Err(_) => Outcome::Failed,
        };
        report(outcome, protocol, "legacy")
    }
}

pub fn approval_message(code: &str) -> String {
    format!("Type {code} to approve this agentdust probe. Nothing will be changed.")
}

fn approval_schema() -> Result<ElicitationSchema, serde_json::Error> {
    let mut schema = ElicitationSchema::from_type::<ApprovalCode>()?;
    schema.title = None;
    schema.description = None;
    Ok(schema)
}

fn outcome_of(response: &ElicitResult, expected: &str) -> Outcome {
    match response.action {
        ElicitationAction::Accept => check_code(
            response
                .content
                .as_ref()
                .and_then(|content| content.get("code"))
                .and_then(|code| code.as_str()),
            expected,
        ),
        ElicitationAction::Decline => Outcome::Declined,
        ElicitationAction::Cancel => Outcome::Cancelled,
        _ => Outcome::Failed,
    }
}

pub fn check_code(answer: Option<&str>, expected: &str) -> Outcome {
    match answer {
        None | Some("") => Outcome::Empty,
        Some(answer) if answer == expected => Outcome::Approved,
        Some(_) => Outcome::WrongCode,
    }
}

fn report(outcome: Outcome, protocol: Option<&ProtocolVersion>, path: &str) -> CallToolResponse {
    let report = ProbeReport {
        outcome,
        protocol: protocol.map(|p| p.as_str().to_owned()).unwrap_or_default(),
        path: path.to_owned(),
    };
    let text = serde_json::to_string(&report).expect("probe report serializes");
    CallToolResult::success(vec![ContentBlock::text(text)]).into()
}

fn probe_tool() -> Tool {
    let mut schema = JsonObject::new();
    schema.insert("type".into(), "object".into());
    schema.insert("properties".into(), JsonObject::new().into());
    let mut tool = Tool::new(
        TOOL_NAME,
        "Asks the user to approve with a typed code. Changes nothing; used to test approval prompts.",
        Arc::new(schema),
    );
    tool.annotations = Some(ToolAnnotations::new().read_only(true).destructive(false));
    tool
}

pub fn new_code() -> io::Result<String> {
    let mut code = String::with_capacity(CODE_LEN);
    while code.len() < CODE_LEN {
        for byte in random_bytes::<8>()? {
            if code.len() < CODE_LEN && byte < 250 {
                code.push(CODE_ALPHABET[usize::from(byte) % CODE_ALPHABET.len()] as char);
            }
        }
    }
    Ok(code)
}

fn random_bytes<const N: usize>() -> io::Result<[u8; N]> {
    let mut bytes = [0u8; N];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn internal(err: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(err.to_string(), None)
}
