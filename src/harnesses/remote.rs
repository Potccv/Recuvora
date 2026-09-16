//! Vendor-neutral Harness mapping over the external extension protocol.
use super::*;
use crate::nodes::ExtensionRegistry;
use crate::protocol::{CallbackFuture, CallbackHandler, ExtensionError, ExtensionKind};
use serde_json::{Value, json};
use std::sync::Mutex;

pub const REMOTE_NODE_ADAPTER: &str = "remote-node";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RemoteWorkspace {
    pub node_id: String,
    pub workspace_id: String,
}
impl RemoteWorkspace {
    pub fn resource_path(&self) -> PathBuf {
        PathBuf::from(format!("node://{}/{}", self.node_id, self.workspace_id))
    }
    pub(crate) fn validate(&self, definition: &HarnessDefinition) -> Result<(), HarnessError> {
        if !crate::protocol::valid_id(&self.node_id)
            || !crate::protocol::valid_id(&self.workspace_id)
            || definition.address != format!("node://{}", self.node_id)
            || !definition
                .workspace_roots
                .iter()
                .any(|id| id.to_str() == Some(&self.workspace_id))
        {
            return Err(HarnessError::InvalidRequest(
                "node/workspace is not in the selected Harness scope".into(),
            ));
        }
        Ok(())
    }
}

pub(crate) fn validate_remote_definition(
    definition: &HarnessDefinition,
) -> Result<(), HarnessError> {
    let Some(id) = definition.address.strip_prefix("node://") else {
        return Err(HarnessError::UnsupportedAddress {
            adapter: definition.adapter.clone(),
            address: definition.address.clone(),
        });
    };
    if !crate::protocol::valid_id(id)
        || definition.workspace_roots.iter().any(|root| {
            root.to_str()
                .is_none_or(|id| !crate::protocol::valid_id(id))
        })
    {
        return Err(HarnessError::InvalidConfiguration(
            "remote address must be node://ID and workspace_roots must contain workspace IDs"
                .into(),
        ));
    }
    Ok(())
}
pub(crate) fn valid_remote_path(path: &Path) -> bool {
    let Some(text) = path.to_str() else {
        return false;
    };
    !text.is_empty()
        && text.len() <= 4096
        && !text.chars().any(char::is_control)
        && (text.starts_with('/')
            || text.starts_with("\\\\")
            || (text.len() > 2
                && text.as_bytes()[0].is_ascii_alphabetic()
                && text.as_bytes()[1] == b':'
                && matches!(text.as_bytes()[2], b'\\' | b'/')))
}

pub struct RemoteHarnessFactory {
    registry: Arc<ExtensionRegistry>,
}
impl RemoteHarnessFactory {
    pub fn new(registry: Arc<ExtensionRegistry>) -> Self {
        Self { registry }
    }
}
impl HarnessAdapterFactory for RemoteHarnessFactory {
    fn adapter_id(&self) -> &str {
        REMOTE_NODE_ADAPTER
    }
    fn build(
        &self,
        definition: HarnessDefinition,
    ) -> Result<Arc<dyn HarnessProvider>, HarnessError> {
        validate_remote_definition(&definition)?;
        let id = definition.address.trim_start_matches("node://");
        let metadata = self
            .registry
            .metadata(id)
            .ok_or_else(|| HarnessError::Unavailable {
                harness: definition.id.clone(),
                message: "node not registered".into(),
            })?;
        if metadata.kind != ExtensionKind::Node
            || !metadata
                .contracts
                .iter()
                .any(|c| c.id == "recuvora.harness" && c.version == 1)
            || definition.workspace_roots.iter().any(|root| {
                !metadata
                    .workspaces
                    .iter()
                    .any(|id| root.to_str() == Some(id))
            })
        {
            return Err(HarnessError::Unavailable {
                harness: definition.id.clone(),
                message: "node does not provide the Harness contract and configured workspaces"
                    .into(),
            });
        }
        Ok(Arc::new(RemoteProvider {
            node_id: id.to_owned(),
            definition,
            registry: self.registry.clone(),
        }))
    }
}
struct RemoteProvider {
    definition: HarnessDefinition,
    node_id: String,
    registry: Arc<ExtensionRegistry>,
}
impl RemoteProvider {
    fn require(&self, capability: &str) -> Result<(), HarnessError> {
        if self
            .registry
            .metadata(&self.node_id)
            .is_some_and(|m| m.capabilities.iter().any(|c| c == capability))
        {
            Ok(())
        } else {
            Err(HarnessError::Unavailable {
                harness: self.definition.id.clone(),
                message: format!("node does not declare {capability}"),
            })
        }
    }
    fn protocol(&self, message: impl Into<String>) -> HarnessError {
        HarnessError::ProtocolViolation {
            harness: self.definition.id.clone(),
            message: message.into(),
        }
    }
    fn project(&self, value: Value) -> Result<HarnessProject, HarnessError> {
        let project: WireProject =
            serde_json::from_value(value).map_err(|e| self.protocol(e.to_string()))?;
        let mut project = HarnessProject {
            harness_id: self.definition.id.clone(),
            id: project.id,
            name: project.name,
            roots: project.roots.into_iter().map(PathBuf::from).collect(),
        };
        normalize_project(&self.definition, &mut project)?;
        Ok(project)
    }
}
impl HarnessProvider for RemoteProvider {
    fn definition(&self) -> &HarnessDefinition {
        &self.definition
    }
    fn run<'a>(&'a self, request: HarnessRunRequest) -> HarnessRunFuture<'a> {
        Box::pin(async move {
            let validated = validate_run_request(&self.definition, request)?;
            let request = validated.into_request();
            self.require("text")?;
            if request.visibility == ConversationVisibility::Client {
                self.require("client_visibility")?;
            }
            if request.role == HarnessRole::Approval {
                self.require("approval")?;
            }
            if !request.tools.is_empty() {
                self.require("tools")?;
            }
            if request.placement.project_id().is_some() {
                self.require("projects")?;
            }
            let placement = match &request.placement {
                ConversationPlacement::NoNativeProject => json!({"type":"none"}),
                ConversationPlacement::ExistingProject { project_id, .. } => {
                    json!({"type":"existing","project_id":project_id})
                }
            };
            let tools=request.tools.iter().map(|t|json!({"name":t.name,"description":t.description,"input_schema":t.input_schema})).collect::<Vec<_>>();
            let params = json!({"harness_id":self.definition.id,"workspace":request.remote_workspace,"prompt":request.prompt,"model":request.model,"visibility":visibility(request.visibility),"placement":placement,"role":if request.role==HarnessRole::Approval {"approval"}else{"execution"},"tools":tools});
            let scope = Arc::new(Mutex::new(ToolScope::default()));
            let handler = request.tool_handler.clone().map(|handler| {
                Arc::new(ToolRouter {
                    harness_id: self.definition.id.clone(),
                    tools: request.tools.clone(),
                    handler,
                    scope: scope.clone(),
                }) as Arc<dyn CallbackHandler>
            });
            let value = self
                .registry
                .call_harness(
                    &self.node_id,
                    "run",
                    params,
                    request.timeout,
                    request.cancellation.clone(),
                    handler,
                    request.role == HarnessRole::Approval,
                )
                .await;
            let uncertainty = |message: String| {
                HarnessError::ConversationOutcomeUnknown(Box::new(ConversationUncertainty {
                    harness: self.definition.id.clone(),
                    thread_id: scope.lock().ok().and_then(|s| s.thread.clone()),
                    project_directory: request.project_directory.clone(),
                    visibility: request.visibility,
                    native_project_id: request.placement.project_id().map(str::to_owned),
                    message,
                }))
            };
            let value = value.map_err(|e| match e {
                ExtensionError::Unknown { .. } => uncertainty(e.to_string()),
                other => map_error(&self.definition, other),
            })?;
            let result: WireRunResult =
                serde_json::from_value(value).map_err(|e| uncertainty(e.to_string()))?;
            if !valid_project_id(&result.thread_id)
                || !valid_project_id(&result.session_id)
                || !valid_remote_path(Path::new(&result.project_directory))
                || result.final_response.len() > 256 * 1024
                || result.visibility != visibility(request.visibility)
                || result.native_project_id.as_deref() != request.placement.project_id()
            {
                return Err(uncertainty(
                    "invalid or mismatched remote run result".into(),
                ));
            }
            if scope
                .lock()
                .map_err(|_| uncertainty("tool scope unavailable".into()))?
                .thread
                .as_ref()
                .is_some_and(|id| id != &result.thread_id)
            {
                return Err(uncertainty(
                    "tool thread differs from returned conversation".into(),
                ));
            }
            let grouping = match result.client_project_grouping {
                WireGrouping::NotApplicable => ClientProjectGrouping::NotApplicable,
                WireGrouping::Unverified => ClientProjectGrouping::Unverified,
                WireGrouping::Confirmed { client_project_id } => {
                    ClientProjectGrouping::Confirmed { client_project_id }
                }
            };
            validate_client_project_grouping(&self.definition, request.visibility, &grouping)
                .map_err(|e| uncertainty(e.to_string()))?;
            Ok(HarnessRunResult {
                harness_id: self.definition.id.clone(),
                adapter: self.definition.adapter.clone(),
                address: self.definition.address.clone(),
                thread_id: result.thread_id,
                session_id: result.session_id,
                // An explicit node resource reference; never a local path.
                project_directory: request.project_directory,
                visibility: request.visibility,
                native_project_id: result.native_project_id,
                client_project_grouping: grouping,
                final_response: result.final_response,
            })
        })
    }
    fn list_projects<'a>(
        &'a self,
        request: HarnessProjectListRequest,
    ) -> HarnessProjectListFuture<'a> {
        Box::pin(async move {
            self.require("projects")?;
            let request = validate_project_list_request(&self.definition, request)?;
            let value = self
                .registry
                .call_harness(
                    &self.node_id,
                    "projects",
                    json!({"workspace":request.remote_workspace}),
                    request.timeout,
                    request.cancellation,
                    None,
                    false,
                )
                .await
                .map_err(|e| map_error(&self.definition, e))?;
            let projects = value
                .as_array()
                .ok_or_else(|| self.protocol("projects result must be array"))?;
            if projects.len() > 512 {
                return Err(self.protocol("too many projects"));
            }
            let mut projects = projects
                .iter()
                .cloned()
                .map(|p| self.project(p))
                .collect::<Result<Vec<_>, _>>()?;
            normalize_project_list_result(&self.definition, &mut projects)?;
            Ok(projects)
        })
    }
    fn create_project<'a>(
        &'a self,
        request: HarnessProjectCreateRequest,
    ) -> HarnessProjectCreateFuture<'a> {
        Box::pin(async move {
            self.require("projects")?;
            let request = validate_project_create_request(&self.definition, request)?;
            let uncertainty = |message: String| {
                HarnessError::ProjectCreationOutcomeUnknown(Box::new(ProjectCreationUncertainty {
                    harness: self.definition.id.clone(),
                    name: request.name.clone(),
                    root_directory: request.root_directory.clone(),
                    idempotency_key: request.idempotency_key.clone(),
                    project_id: None,
                    message,
                }))
            };
            let value=self.registry.call_harness(&self.node_id,"create_project",json!({"workspace":request.remote_workspace,"name":request.name,"idempotency_key":request.idempotency_key}),request.timeout,request.cancellation.clone(),None,false).await.map_err(|e|match e {ExtensionError::Unknown {..}=>uncertainty(e.to_string()),other=>map_error(&self.definition,other)})?;
            self.project(value).map_err(|e| uncertainty(e.to_string()))
        })
    }
}
fn visibility(value: ConversationVisibility) -> &'static str {
    match value {
        ConversationVisibility::Hidden => "hidden",
        ConversationVisibility::Client => "client",
    }
}
fn map_error(definition: &HarnessDefinition, error: ExtensionError) -> HarnessError {
    match error {
        ExtensionError::Cancelled => HarnessError::Interrupted {
            harness: definition.id.clone(),
        },
        ExtensionError::Configuration(message) => HarnessError::InvalidConfiguration(message),
        ExtensionError::Rejected(message) => HarnessError::TurnFailed {
            harness: definition.id.clone(),
            message,
        },
        ExtensionError::Protocol(message) => HarnessError::ProtocolViolation {
            harness: definition.id.clone(),
            message,
        },
        other => HarnessError::Unavailable {
            harness: definition.id.clone(),
            message: other.to_string(),
        },
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireProject {
    id: String,
    name: String,
    roots: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireRunResult {
    thread_id: String,
    session_id: String,
    project_directory: String,
    visibility: String,
    native_project_id: Option<String>,
    client_project_grouping: WireGrouping,
    final_response: String,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum WireGrouping {
    NotApplicable,
    Unverified,
    Confirmed { client_project_id: Option<String> },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireTool {
    harness_id: String,
    thread_id: String,
    turn_id: String,
    call_id: String,
    tool: String,
    arguments: Value,
}
#[derive(Default)]
struct ToolScope {
    thread: Option<String>,
    turn: Option<String>,
    calls: BTreeSet<String>,
}
struct ToolRouter {
    harness_id: String,
    tools: Vec<HarnessTool>,
    handler: Arc<dyn HarnessToolHandler>,
    scope: Arc<Mutex<ToolScope>>,
}
impl CallbackHandler for ToolRouter {
    fn call<'a>(
        &'a self,
        method: String,
        params: Value,
        cancellation: HarnessCancellation,
    ) -> CallbackFuture<'a> {
        Box::pin(async move {
            if method != "tool" {
                return Err(ExtensionError::Rejected(
                    "only declared host tools are supported".into(),
                ));
            }
            let tool: WireTool = serde_json::from_value(params)
                .map_err(|e| ExtensionError::Rejected(e.to_string()))?;
            if tool.harness_id != self.harness_id
                || !valid_project_id(&tool.thread_id)
                || !valid_project_id(&tool.turn_id)
                || !valid_project_id(&tool.call_id)
                || tool.arguments.to_string().len() > 64 * 1024
                || !tool.arguments.is_object()
                || !self.tools.iter().any(|t| t.name == tool.tool)
            {
                return Err(ExtensionError::Rejected(
                    "invalid tool scope, arguments or tool name".into(),
                ));
            }
            {
                let mut scope = self
                    .scope
                    .lock()
                    .map_err(|_| ExtensionError::Rejected("tool scope unavailable".into()))?;
                if scope
                    .thread
                    .as_ref()
                    .is_some_and(|id| id != &tool.thread_id)
                    || scope.turn.as_ref().is_some_and(|id| id != &tool.turn_id)
                    || scope.calls.len() >= 64
                    || !scope.calls.insert(tool.call_id.clone())
                {
                    return Err(ExtensionError::Rejected(
                        "duplicate tool or changed conversation scope".into(),
                    ));
                }
                scope.thread = Some(tool.thread_id.clone());
                scope.turn = Some(tool.turn_id.clone());
            }
            if cancellation.is_cancelled() {
                return Err(ExtensionError::Cancelled);
            }
            let result = self
                .handler
                .call(HarnessToolCall {
                    harness_id: tool.harness_id,
                    thread_id: tool.thread_id,
                    turn_id: tool.turn_id,
                    call_id: tool.call_id,
                    tool: tool.tool,
                    arguments: tool.arguments,
                    cancellation,
                })
                .await
                .map_err(|e| ExtensionError::Rejected(e.to_string()))?;
            if result.content.len() > 256 * 1024 {
                return Err(ExtensionError::Rejected(
                    "tool output exceeds 256 KiB".into(),
                ));
            }
            Ok(json!({"content":result.content,"success":result.success}))
        })
    }
}
