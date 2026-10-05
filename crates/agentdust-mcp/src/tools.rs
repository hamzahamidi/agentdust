use std::sync::Arc;
use std::time::Duration;

use agentdust_core::apply::exec::{Deps, Settings};
use agentdust_core::apply::server::{ApplyError, Call, Challenge, Response, Server, Step};
use agentdust_core::apply::signal::KillSignaller;
use agentdust_core::apply::timer::SystemTimer;
use agentdust_core::classifier::Policy;
use agentdust_core::cwd::RelationContext;
use agentdust_core::darwin::{self, DarwinLive, DarwinProvider, DarwinSource};
use agentdust_core::doctor::{Components, run as diagnose};
use agentdust_core::finding::ModelFinding;
use agentdust_core::inventory::{LaunchctlList, SystemClock};
use agentdust_core::journal::volume::SystemVolume;
use agentdust_core::paths;
use agentdust_core::secret;
use agentdust_core::session::ProviderLiveness;
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
use serde_json::Value;

const DISK: &str = "agentdust_disk";
const DOCTOR: &str = "agentdust_doctor";
const PLAN: &str = "agentdust_plan";
const APPLY: &str = "agentdust_apply";
const INPUT_KEY: &str = "approval";
const CODE_TTL: Duration = Duration::from_secs(120);

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct ApplyArgs {
    plan_id: String,
    item_ids: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct EmptyArgs {}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[allow(dead_code)]
struct ApprovalCode {
    #[schemars(description = "The code shown in the approval prompt")]
    pub code: String,
}

#[derive(Serialize)]
struct PlanResult {
    plan_id: String,
    expires_in_seconds: u64,
    report: String,
    items: Vec<ModelFinding>,
}

#[derive(Serialize)]
struct ErrorResult {
    error: String,
    code: &'static str,
}

#[derive(Clone)]
pub struct AgentDustServer {
    apply: Arc<Server>,
    disk_slot: Arc<tokio::sync::Semaphore>,
}

impl AgentDustServer {
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let data_dir = paths::data_dir()?;
        let surveyor = Arc::new(agentdust_core::live::LiveSurveyor::new(&data_dir));
        let provider = DarwinProvider::new()?;
        let apply = Server::new(
            Deps {
                data_dir,
                surveyor: surveyor.clone(),
                provider: Box::new(provider),
                signaller: Box::new(KillSignaller),
                timer: Arc::new(SystemTimer::new()),
            },
            Settings::default(),
        );
        Ok(Self {
            apply: Arc::new(apply),
            disk_slot: Arc::new(tokio::sync::Semaphore::new(1)),
        })
    }
}

impl ServerHandler for AgentDustServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("agentdust", env!("CARGO_PKG_VERSION")))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(
            ListToolsResult::with_all_items(vec![doctor_tool(), plan_tool(), apply_tool(), disk_tool()])
                .with_ttl_ms(0)
                .with_cache_scope(CacheScope::Private),
        )
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        match request.name.as_ref() {
            DOCTOR => self.doctor(),
            DISK => self.disk(&request).await,
            PLAN => self.plan(),
            APPLY => self.apply(&request, &context).await,
            _ => Err(ErrorData::invalid_params(
                format!("unknown tool {}", request.name),
                None,
            )),
        }
    }
}

impl AgentDustServer {
    fn cancel_pending(&self, call: &Call, nonce: &str) {
        let mut step = self.apply.answer(call, nonce, Response::Cancel);
        while let Ok(Step::Ask(challenge)) = step {
            step = self.apply.answer(call, &challenge.nonce, Response::Cancel);
        }
    }

    async fn disk(&self, request: &CallToolRequestParams) -> Result<CallToolResponse, ErrorData> {
        if request.arguments.as_ref().is_some_and(|args| !args.is_empty()) {
            return Err(ErrorData::invalid_params("disk report takes no arguments", None));
        }
        let permit = self
            .disk_slot
            .clone()
            .try_acquire_owned()
            .map_err(|_| internal("disk report already running"))?;
        let _: EmptyArgs = arguments(request)?;
        let config =
            paths::claude_config_dir().map_err(|_| internal("Claude configuration root unavailable"))?;
        let project = std::env::current_dir().map_err(|_| internal("current project unavailable"))?;
        let report = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            agentdust_core::disk::report(&config, &project)
        })
        .await
        .map_err(|_| internal("disk report unavailable"))?;
        success(&report)
    }

    fn doctor(&self) -> Result<CallToolResponse, ErrorData> {
        let data_dir = paths::data_dir().map_err(internal)?;
        let install_secret = secret::load_existing(&data_dir).ok();
        let source = DarwinSource::new(install_secret.as_ref()).map_err(internal)?;
        let provider = DarwinProvider::new().map_err(internal)?;
        let live = DarwinLive::new().map_err(internal)?;
        let relation = RelationContext::system();
        let diagnosis = diagnose(&Components {
            data_dir: &data_dir,
            volume: &SystemVolume,
            secret_available: install_secret.is_some(),
            processes: &source,
            launchd: &LaunchctlList,
            clock: &SystemClock,
            liveness: &ProviderLiveness(&provider),
            live: &live,
            relation: &relation,
            policy: Policy::new(darwin::current_uid(), std::process::id() as i32),
        })
        .map_err(internal)?;
        let result = diagnosis.to_json();
        Ok(CallToolResult::success(vec![ContentBlock::text(result)]).into())
    }

    fn plan(&self) -> Result<CallToolResponse, ErrorData> {
        let plan = self.apply.plan().map_err(internal)?;
        success(&PlanResult {
            plan_id: plan.plan_id,
            expires_in_seconds: plan.expires_in.as_secs(),
            report: plan.report,
            items: plan.items,
        })
    }

    async fn apply(
        &self,
        request: &CallToolRequestParams,
        context: &RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args: ApplyArgs = arguments(request)?;
        let call = Call {
            plan_id: args.plan_id,
            item_ids: args.item_ids,
        };
        let protocol = context.protocol_version();
        let supports_form = context
            .client_capabilities()
            .and_then(|caps| caps.elicitation)
            .is_some_and(|e| e.form.is_some() || e.url.is_none());
        if !supports_form {
            return success(&ErrorResult {
                error: "Run `agentdust apply` in a terminal because this client cannot show approval forms."
                    .into(),
                code: "apply_not_supported",
            });
        }
        let retry_based = protocol
            .as_ref()
            .is_some_and(|v| v.as_str() >= ProtocolVersion::V_2026_07_28.as_str());
        if retry_based {
            let step = if let Some(nonce) = request.request_state.as_deref() {
                let Some(input) = request.input_responses.as_ref().and_then(|r| r.get(INPUT_KEY)) else {
                    self.cancel_pending(&call, nonce);
                    return Err(ErrorData::invalid_params("missing approval response", None));
                };
                let result = match ElicitResult::deserialize(input) {
                    Ok(result) => result,
                    Err(_) => {
                        self.cancel_pending(&call, nonce);
                        return Err(ErrorData::invalid_params("invalid approval response", None));
                    }
                };
                self.apply
                    .answer(&call, nonce, elicitation_response(result))
                    .map_err(apply_error)?
            } else {
                match self.apply.begin(&call).map_err(apply_error)? {
                    Step::Ask(challenge) => return input_required(&challenge),
                    Step::Done(report) => return success(&report),
                }
            };
            return match step {
                Step::Ask(challenge) => input_required(&challenge),
                Step::Done(report) => success(&report),
            };
        }
        let mut step = self.apply.begin(&call).map_err(apply_error)?;
        loop {
            let challenge = match step {
                Step::Done(report) => return success(&report),
                Step::Ask(challenge) => challenge,
            };
            let params = ElicitRequestParams::FormElicitationParams {
                meta: None,
                message: challenge.message.clone(),
                requested_schema: approval_schema().map_err(internal)?,
            };
            let decision = match context
                .peer
                .create_elicitation_with_timeout(params, Some(CODE_TTL))
                .await
            {
                Ok(result) => elicitation_response(result),
                Err(ServiceError::Timeout { .. }) => Response::Timeout,
                Err(_) => {
                    self.cancel_pending(&call, &challenge.nonce);
                    return success(&ErrorResult {
                        error: "The client could not show or collect the approval form.".into(),
                        code: "elicitation_failed",
                    });
                }
            };
            step = self
                .apply
                .answer(&call, &challenge.nonce, decision)
                .map_err(apply_error)?;
        }
    }
}

fn disk_tool() -> Tool {
    tool::<EmptyArgs>(
        DISK,
        "Read-only Claude Code disk inventory: logical and allocated bytes by category. No file contents or paths. Scans configuration and the server working directory's .claude/worktrees only. Partial scans are marked. Allocated bytes are not reclaimable space. Deletes nothing.",
        true,
    )
}

fn doctor_tool() -> Tool {
    tool::<EmptyArgs>(
        DOCTOR,
        "Read-only inventory of processes and their AgentDust classification.",
        true,
    )
}
fn plan_tool() -> Tool {
    tool::<EmptyArgs>(
        PLAN,
        "Create a fresh plan of eligible processes. This does not signal anything.",
        true,
    )
}
fn apply_tool() -> Tool {
    tool::<ApplyArgs>(
        APPLY,
        "Ask for typed approval, then send at most one SIGTERM to each approved, revalidated process.",
        false,
    )
}

fn tool<T: JsonSchema>(name: &'static str, description: &'static str, read_only: bool) -> Tool {
    let value = rmcp::schemars::schema_for!(T);
    let schema: JsonObject = serde_json::to_value(value)
        .expect("schema serializes")
        .as_object()
        .cloned()
        .expect("schema is object")
        .into_iter()
        .collect();
    let mut item = Tool::new(name, description, Arc::new(schema));
    item.annotations = Some(
        ToolAnnotations::new()
            .read_only(read_only)
            .destructive(!read_only),
    );
    item
}

fn arguments<T: for<'de> Deserialize<'de>>(request: &CallToolRequestParams) -> Result<T, ErrorData> {
    let value = request.arguments.clone().unwrap_or_default();
    serde_json::from_value(Value::Object(value))
        .map_err(|_| ErrorData::invalid_params("invalid tool arguments", None))
}

fn success<T: Serialize>(value: &T) -> Result<CallToolResponse, ErrorData> {
    let text = serde_json::to_string(value).map_err(internal)?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]).into())
}

fn input_required(challenge: &Challenge) -> Result<CallToolResponse, ErrorData> {
    let mut requests = InputRequests::new();
    requests.insert(
        INPUT_KEY.to_owned(),
        InputRequest::Elicitation(ElicitRequest::new(ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: challenge.message.clone(),
            requested_schema: approval_schema().map_err(internal)?,
        })),
    );
    Ok(InputRequiredResult::new(Some(requests), Some(challenge.nonce.clone())).into())
}

fn approval_schema() -> Result<ElicitationSchema, serde_json::Error> {
    let mut schema = ElicitationSchema::from_type::<ApprovalCode>()?;
    schema.title = None;
    schema.description = None;
    Ok(schema)
}

fn elicitation_response(result: ElicitResult) -> Response {
    match result.action {
        ElicitationAction::Accept => Response::Accept(
            result
                .content
                .as_ref()
                .and_then(|content| content.get("code"))
                .and_then(|value| value.as_str())
                .map(str::to_owned),
        ),
        ElicitationAction::Decline => Response::Decline,
        ElicitationAction::Cancel => Response::Cancel,
        _ => Response::Cancel,
    }
}

fn apply_error(error: ApplyError) -> ErrorData {
    ErrorData::invalid_params(error.to_string(), None)
}

fn internal(error: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(error.to_string(), None)
}
