//! Stable multi-Harness contracts and provider selection.
//!
//! A configured address selects an instance of an adapter that was compiled
//! into and registered by the trusted host. Configuration never downloads or
//! loads SDK code. Provider-specific protocols stay in their adapter module.

mod provider_path;
mod remote;
pub use remote::{REMOTE_NODE_ADAPTER, RemoteHarnessFactory, RemoteWorkspace};

use crate::framework::CallScope;
pub use crate::framework::Cancellation as HarnessCancellation;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::future::Future;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

pub const CONFIG_SCHEMA_VERSION: u32 = 1;

const MAX_CONFIG_BYTES: u64 = 64 * 1024;
const MAX_HARNESSES: usize = 64;
const MAX_WORKSPACE_ROOTS: usize = 16;
const MAX_IDENTIFIER_BYTES: usize = 64;
const MAX_ADDRESS_BYTES: usize = 2 * 1024;
const MAX_MODEL_BYTES: usize = 128;
const MAX_PROMPT_BYTES: usize = 64 * 1024;
const MAX_PROJECT_ID_BYTES: usize = 512;
const MAX_PROJECT_NAME_BYTES: usize = 256;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 512;
const MAX_DISCOVERED_PROJECTS: usize = 512;
const MAX_TURN_DURATION: Duration = Duration::from_secs(30 * 60);
const DEFAULT_TURN_DURATION: Duration = Duration::from_secs(3 * 60);

pub type HarnessRunFuture<'a> =
    Pin<Box<dyn Future<Output = Result<HarnessRunResult, HarnessError>> + Send + 'a>>;
pub type HarnessProjectListFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<HarnessProject>, HarnessError>> + Send + 'a>>;
pub type HarnessProjectCreateFuture<'a> =
    Pin<Box<dyn Future<Output = Result<HarnessProject, HarnessError>> + Send + 'a>>;

/// Roles share an instance but never share an approval/execution conversation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HarnessRole {
    #[default]
    Execution,
    Approval,
}

#[derive(Clone, Debug)]
pub struct HarnessTool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct HarnessToolCall {
    pub harness_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub call_id: String,
    pub tool: String,
    pub arguments: serde_json::Value,
    pub cancellation: HarnessCancellation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessToolResult {
    pub content: String,
    pub success: bool,
}

pub type HarnessToolFuture<'a> =
    Pin<Box<dyn Future<Output = Result<HarnessToolResult, HarnessError>> + Send + 'a>>;

/// Trusted handlers must validate parameters and authority before effects,
/// enforce their own finite deadline, observe cancellation before dispatch,
/// and durably record effects before returning. Cancellation never rolls back
/// an effect. Providers await every dispatched handler, including on failure.
pub trait HarnessToolHandler: Send + Sync {
    fn call<'a>(&'a self, call: HarnessToolCall) -> HarnessToolFuture<'a>;
}

trait HarnessCancellationExt {
    fn ensure_active(&self, definition: &HarnessDefinition) -> Result<(), HarnessError>;
}

impl HarnessCancellationExt for HarnessCancellation {
    fn ensure_active(&self, definition: &HarnessDefinition) -> Result<(), HarnessError> {
        if self.is_cancelled() {
            return Err(HarnessError::Interrupted {
                harness: definition.id.clone(),
            });
        }
        Ok(())
    }
}

/// User-editable configuration for a set of named Harness instances.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessRegistryConfig {
    pub schema_version: u32,
    #[serde(default)]
    pub default_harness: Option<String>,
    pub harnesses: Vec<HarnessDefinition>,
}

impl HarnessRegistryConfig {
    /// Checks the registry schema and instance selection without constructing
    /// providers or contacting any configured Harness.
    pub fn validate(&self) -> Result<(), HarnessError> {
        validate_registry_config(self)
    }

    /// Parses a bounded JSON document. Relative workspace roots are resolved by
    /// the registry builder against the process working directory.
    pub fn from_json(bytes: &[u8]) -> Result<Self, HarnessError> {
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return Err(HarnessError::InvalidConfiguration(format!(
                "Harness configuration exceeds {MAX_CONFIG_BYTES} bytes"
            )));
        }
        serde_json::from_slice(bytes)
            .map_err(|error| HarnessError::InvalidConfiguration(error.to_string()))
    }

    /// Loads JSON and resolves relative workspace roots against the directory
    /// containing the configuration file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, HarnessError> {
        let path = fs::canonicalize(path.as_ref()).map_err(|error| {
            HarnessError::InvalidConfiguration(format!(
                "cannot resolve Harness configuration: {error}"
            ))
        })?;
        let metadata = fs::metadata(&path).map_err(|error| {
            HarnessError::InvalidConfiguration(format!(
                "cannot inspect Harness configuration: {error}"
            ))
        })?;
        if !metadata.is_file() {
            return Err(HarnessError::InvalidConfiguration(
                "Harness configuration must be a file".to_owned(),
            ));
        }
        if metadata.len() > MAX_CONFIG_BYTES {
            return Err(HarnessError::InvalidConfiguration(format!(
                "Harness configuration exceeds {MAX_CONFIG_BYTES} bytes"
            )));
        }
        let file = fs::File::open(&path).map_err(|error| {
            HarnessError::InvalidConfiguration(format!(
                "cannot read Harness configuration: {error}"
            ))
        })?;
        let mut bytes = Vec::new();
        file.take(MAX_CONFIG_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| {
                HarnessError::InvalidConfiguration(format!(
                    "cannot read Harness configuration: {error}"
                ))
            })?;
        let mut config = Self::from_json(&bytes)?;
        let base = path.parent().ok_or_else(|| {
            HarnessError::InvalidConfiguration(
                "Harness configuration has no parent directory".to_owned(),
            )
        })?;
        for harness in &mut config.harnesses {
            if harness.adapter == REMOTE_NODE_ADAPTER {
                continue;
            }
            for root in &mut harness.workspace_roots {
                if root.is_relative() {
                    *root = base.join(&*root);
                }
            }
        }
        Ok(config)
    }
}

/// A configured Harness instance. `adapter` names trusted code registered by
/// the host; `address` is interpreted only by that adapter.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessDefinition {
    pub id: String,
    pub adapter: String,
    pub address: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    pub workspace_roots: Vec<PathBuf>,
}

impl HarnessDefinition {
    pub fn new(
        id: impl Into<String>,
        adapter: impl Into<String>,
        address: impl Into<String>,
        workspace_roots: Vec<PathBuf>,
    ) -> Self {
        Self {
            id: id.into(),
            adapter: adapter.into(),
            address: address.into(),
            enabled: true,
            workspace_roots,
        }
    }
}

fn enabled_by_default() -> bool {
    true
}

/// Whether a provider conversation is stored for a conversation client.
///
/// `Hidden` maps to an ephemeral provider thread. It describes client history,
/// not data privacy: a provider still receives the request. `Client` asks the
/// provider to persist the thread so a compatible client can display it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConversationVisibility {
    Hidden,
    Client,
}

/// What has been verified about a separate conversation client's project UI.
///
/// A provider-native project response and a desktop client's project grouping
/// can use different identifiers and persistence layers. Providers must return
/// `Unverified` unless they have observed the separate client state through a
/// supported integration. `Confirmed { client_project_id: None }` means that
/// the client was observed outside every project at that point in time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientProjectGrouping {
    NotApplicable,
    Unverified,
    Confirmed { client_project_id: Option<String> },
}

/// Optional provider-native project assignment for a conversation.
///
/// Project identifiers are opaque and scoped to one configured Harness
/// instance. A provider confirming this assignment does not by itself promise
/// that a separate desktop client has refreshed its own project membership.
/// The execution directory remains an independent, mandatory security boundary
/// even when no project is assigned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConversationPlacement {
    NoNativeProject,
    ExistingProject {
        harness_id: String,
        project_id: String,
    },
}

impl ConversationPlacement {
    fn project_id(&self) -> Option<&str> {
        match self {
            Self::NoNativeProject => None,
            Self::ExistingProject { project_id, .. } => Some(project_id),
        }
    }
}

/// One request to create a new provider conversation and run one turn.
#[derive(Clone)]
pub struct HarnessRunRequest {
    project_directory: PathBuf,
    remote_workspace: Option<RemoteWorkspace>,
    prompt: String,
    model: Option<String>,
    timeout: Duration,
    visibility: ConversationVisibility,
    placement: ConversationPlacement,
    cancellation: HarnessCancellation,
    role: HarnessRole,
    tools: Vec<HarnessTool>,
    tool_handler: Option<Arc<dyn HarnessToolHandler>>,
}

impl std::fmt::Debug for HarnessRunRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HarnessRunRequest")
            .field("project_directory", &self.project_directory)
            .field("role", &self.role)
            .field("tools", &self.tools)
            .field("timeout", &self.timeout)
            .field("visibility", &self.visibility)
            .finish_non_exhaustive()
    }
}

impl HarnessRunRequest {
    pub fn new(project_directory: impl Into<PathBuf>, prompt: impl Into<String>) -> Self {
        Self {
            project_directory: project_directory.into(),
            remote_workspace: None,
            prompt: prompt.into(),
            model: None,
            timeout: DEFAULT_TURN_DURATION,
            visibility: ConversationVisibility::Client,
            placement: ConversationPlacement::NoNativeProject,
            cancellation: HarnessCancellation::new(),
            role: HarnessRole::Execution,
            tools: Vec::new(),
            tool_handler: None,
        }
    }

    /// Selects a node-owned workspace identifier, never a host filesystem path.
    pub fn remote(
        node_id: impl Into<String>,
        workspace_id: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        let workspace = RemoteWorkspace {
            node_id: node_id.into(),
            workspace_id: workspace_id.into(),
        };
        let mut request = Self::new(workspace.resource_path(), prompt);
        request.remote_workspace = Some(workspace);
        request
    }

    pub fn remote_workspace(&self) -> Option<&RemoteWorkspace> {
        self.remote_workspace.as_ref()
    }

    pub fn with_role(mut self, role: HarnessRole) -> Self {
        self.role = role;
        if role == HarnessRole::Approval {
            self.visibility = ConversationVisibility::Hidden;
            self.placement = ConversationPlacement::NoNativeProject;
        }
        self
    }

    pub fn role(&self) -> HarnessRole {
        self.role
    }

    pub fn with_tools(
        mut self,
        tools: Vec<HarnessTool>,
        handler: Arc<dyn HarnessToolHandler>,
    ) -> Self {
        self.tools = tools;
        self.tool_handler = Some(handler);
        self
    }

    pub fn tools(&self) -> &[HarnessTool] {
        &self.tools
    }

    pub fn tool_handler(&self) -> Option<&Arc<dyn HarnessToolHandler>> {
        self.tool_handler.as_ref()
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_cancellation(mut self, cancellation: HarnessCancellation) -> Self {
        self.cancellation = cancellation;
        self
    }

    pub fn cancellation(&self) -> &HarnessCancellation {
        &self.cancellation
    }

    pub fn with_visibility(mut self, visibility: ConversationVisibility) -> Self {
        self.visibility = visibility;
        self
    }

    pub fn with_placement(mut self, placement: ConversationPlacement) -> Self {
        self.placement = placement;
        self
    }

    /// Compatibility builder for callers that use the provider protocol term.
    /// Prefer [`Self::with_visibility`] in user-facing code.
    pub fn ephemeral(mut self, ephemeral: bool) -> Self {
        self.visibility = if ephemeral {
            ConversationVisibility::Hidden
        } else {
            ConversationVisibility::Client
        };
        self
    }

    pub fn project_directory(&self) -> &Path {
        &self.project_directory
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub fn visibility(&self) -> ConversationVisibility {
        self.visibility
    }

    pub fn placement(&self) -> &ConversationPlacement {
        &self.placement
    }

    pub fn is_ephemeral(&self) -> bool {
        self.visibility == ConversationVisibility::Hidden
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessRunResult {
    pub harness_id: String,
    pub adapter: String,
    pub address: String,
    pub thread_id: String,
    pub session_id: String,
    pub project_directory: PathBuf,
    pub visibility: ConversationVisibility,
    /// The provider-native project id confirmed for the new thread, if any.
    /// This is not necessarily an id used by a separate desktop client.
    pub native_project_id: Option<String>,
    pub client_project_grouping: ClientProjectGrouping,
    pub final_response: String,
}

/// A provider-native project discovered or created by one Harness.
///
/// `id` is meaningful only together with `harness_id`. Roots are provider
/// metadata and must be revalidated before they influence an execution scope.
/// A separate desktop client may maintain another project id and membership
/// layer that this value does not update.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessProject {
    pub harness_id: String,
    pub id: String,
    pub name: String,
    pub roots: Vec<PathBuf>,
}

/// Bounded request to discover the projects visible to one Harness instance.
#[derive(Clone, Debug)]
pub struct HarnessProjectListRequest {
    context_directory: PathBuf,
    remote_workspace: Option<RemoteWorkspace>,
    timeout: Duration,
    cancellation: HarnessCancellation,
}

impl HarnessProjectListRequest {
    pub fn new(context_directory: impl Into<PathBuf>) -> Self {
        Self {
            context_directory: context_directory.into(),
            remote_workspace: None,
            timeout: DEFAULT_TURN_DURATION,
            cancellation: HarnessCancellation::new(),
        }
    }

    pub fn remote(node_id: impl Into<String>, workspace_id: impl Into<String>) -> Self {
        let workspace = RemoteWorkspace {
            node_id: node_id.into(),
            workspace_id: workspace_id.into(),
        };
        let mut request = Self::new(workspace.resource_path());
        request.remote_workspace = Some(workspace);
        request
    }
    pub fn remote_workspace(&self) -> Option<&RemoteWorkspace> {
        self.remote_workspace.as_ref()
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn context_directory(&self) -> &Path {
        &self.context_directory
    }

    pub fn with_cancellation(mut self, cancellation: HarnessCancellation) -> Self {
        self.cancellation = cancellation;
        self
    }

    pub fn cancellation(&self) -> &HarnessCancellation {
        &self.cancellation
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

/// Request to register one existing, allowed directory as a provider project.
///
/// This contract never creates the root directory. The idempotency key lets a
/// supporting provider make an explicit create request safe to retry.
#[derive(Clone, Debug)]
pub struct HarnessProjectCreateRequest {
    name: String,
    root_directory: PathBuf,
    remote_workspace: Option<RemoteWorkspace>,
    idempotency_key: String,
    timeout: Duration,
    cancellation: HarnessCancellation,
}

impl HarnessProjectCreateRequest {
    pub fn new(
        name: impl Into<String>,
        root_directory: impl Into<PathBuf>,
        idempotency_key: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            root_directory: root_directory.into(),
            remote_workspace: None,
            idempotency_key: idempotency_key.into(),
            timeout: DEFAULT_TURN_DURATION,
            cancellation: HarnessCancellation::new(),
        }
    }

    pub fn remote(
        node_id: impl Into<String>,
        workspace_id: impl Into<String>,
        name: impl Into<String>,
        idempotency_key: impl Into<String>,
    ) -> Self {
        let workspace = RemoteWorkspace {
            node_id: node_id.into(),
            workspace_id: workspace_id.into(),
        };
        let mut request = Self::new(name, workspace.resource_path(), idempotency_key);
        request.remote_workspace = Some(workspace);
        request
    }
    pub fn remote_workspace(&self) -> Option<&RemoteWorkspace> {
        self.remote_workspace.as_ref()
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn with_cancellation(mut self, cancellation: HarnessCancellation) -> Self {
        self.cancellation = cancellation;
        self
    }

    pub fn cancellation(&self) -> &HarnessCancellation {
        &self.cancellation
    }

    pub fn root_directory(&self) -> &Path {
        &self.root_directory
    }

    pub fn idempotency_key(&self) -> &str {
        &self.idempotency_key
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

/// A configured provider hides its vendor protocol behind the common result.
/// Providers observe request cancellation, finish resource cleanup, and retain
/// any unknown persistent outcome in the returned error. Callers must continue
/// awaiting the request after cancelling its token to receive that outcome.
pub trait HarnessProvider: Send + Sync {
    fn definition(&self) -> &HarnessDefinition;
    fn run<'a>(&'a self, request: HarnessRunRequest) -> HarnessRunFuture<'a>;

    fn list_projects<'a>(
        &'a self,
        _request: HarnessProjectListRequest,
    ) -> HarnessProjectListFuture<'a> {
        let harness = self.definition().id.clone();
        Box::pin(async move { Err(HarnessError::ProjectDiscoveryUnsupported { harness }) })
    }

    fn create_project<'a>(
        &'a self,
        _request: HarnessProjectCreateRequest,
    ) -> HarnessProjectCreateFuture<'a> {
        let harness = self.definition().id.clone();
        Box::pin(async move { Err(HarnessError::ProjectCreationUnsupported { harness }) })
    }
}

/// Factories are registered by trusted boot code. A configuration string can
/// select a factory but cannot cause a new SDK or executable to be installed.
pub trait HarnessAdapterFactory: Send + Sync {
    fn adapter_id(&self) -> &str;
    fn build(
        &self,
        definition: HarnessDefinition,
    ) -> Result<Arc<dyn HarnessProvider>, HarnessError>;
}

pub struct HarnessRegistryBuilder {
    factories: BTreeMap<String, Arc<dyn HarnessAdapterFactory>>,
}

impl HarnessRegistryBuilder {
    pub fn new() -> Self {
        Self {
            factories: BTreeMap::new(),
        }
    }

    pub fn register(
        &mut self,
        factory: Arc<dyn HarnessAdapterFactory>,
    ) -> Result<(), HarnessError> {
        let id = factory.adapter_id().to_owned();
        validate_identifier("adapter", &id)?;
        if self.factories.contains_key(&id) {
            return Err(HarnessError::DuplicateAdapter(id));
        }
        self.factories.insert(id, factory);
        Ok(())
    }

    pub fn build(self, mut config: HarnessRegistryConfig) -> Result<HarnessRegistry, HarnessError> {
        validate_registry_config(&config)?;
        let mut entries = BTreeMap::new();
        for definition in &mut config.harnesses {
            let factory = self
                .factories
                .get(&definition.adapter)
                .ok_or_else(|| HarnessError::UnsupportedAdapter(definition.adapter.clone()))?;
            canonicalize_workspace_roots(definition)?;
            let provider = factory.build(definition.clone())?;
            entries.insert(
                definition.id.clone(),
                RegistryEntry {
                    definition: definition.clone(),
                    provider,
                },
            );
        }
        Ok(HarnessRegistry {
            default_harness: config.default_harness,
            entries: Arc::new(entries),
            calls: CallScope::default(),
        })
    }
}

impl Default for HarnessRegistryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

struct RegistryEntry {
    definition: HarnessDefinition,
    provider: Arc<dyn HarnessProvider>,
}

/// A bounded set of explicitly named Harness instances. Routing never falls
/// back to a different instance after a requested provider fails.
#[derive(Clone)]
pub struct HarnessRegistry {
    default_harness: Option<String>,
    entries: Arc<BTreeMap<String, RegistryEntry>>,
    calls: CallScope,
}

impl HarnessRegistry {
    pub fn definitions(&self) -> Vec<HarnessDefinition> {
        self.entries
            .values()
            .map(|entry| entry.definition.clone())
            .collect()
    }

    pub fn default_harness(&self) -> Option<&str> {
        self.default_harness.as_deref()
    }

    pub async fn list_projects(
        &self,
        harness_id: Option<&str>,
        request: HarnessProjectListRequest,
    ) -> Result<Vec<HarnessProject>, HarnessError> {
        let registry = self.clone();
        let harness_id = harness_id.map(str::to_owned);
        self.calls
            .run(request.cancellation.clone(), async move {
                registry
                    .list_projects_inner(harness_id.as_deref(), request)
                    .await
            })
            .await
            .map_err(dispatch_error)?
    }

    async fn list_projects_inner(
        &self,
        harness_id: Option<&str>,
        request: HarnessProjectListRequest,
    ) -> Result<Vec<HarnessProject>, HarnessError> {
        let entry = self.entry(harness_id)?;
        let request = validate_project_list_request(&entry.definition, request)?;
        let mut projects = entry.provider.list_projects(request).await?;
        normalize_project_list_result(&entry.definition, &mut projects)?;
        Ok(projects)
    }

    pub async fn create_project(
        &self,
        harness_id: Option<&str>,
        request: HarnessProjectCreateRequest,
    ) -> Result<HarnessProject, HarnessError> {
        let registry = self.clone();
        let uncertainty = ProjectCreationUncertainty {
            harness: harness_id
                .or(self.default_harness())
                .unwrap_or("selected")
                .into(),
            name: request.name.clone(),
            root_directory: request.root_directory.clone(),
            idempotency_key: request.idempotency_key.clone(),
            project_id: None,
            message:
                "service supervisor terminated after dispatch; inspect the original project request"
                    .into(),
        };
        let harness_id = harness_id.map(str::to_owned);
        self.calls
            .run(request.cancellation.clone(), async move {
                registry
                    .create_project_inner(harness_id.as_deref(), request)
                    .await
            })
            .await
            .map_err(|error| match error {
                crate::framework::DispatchError::Supervisor(_) => {
                    HarnessError::ProjectCreationOutcomeUnknown(Box::new(uncertainty))
                }
                error => dispatch_error(error),
            })?
    }

    async fn create_project_inner(
        &self,
        harness_id: Option<&str>,
        request: HarnessProjectCreateRequest,
    ) -> Result<HarnessProject, HarnessError> {
        let entry = self.entry(harness_id)?;
        let request = validate_project_create_request(&entry.definition, request)?;
        let remote = request.remote_workspace.is_some();
        let expected_name = request.name.clone();
        let expected_root = request.root_directory.clone();
        let idempotency_key = request.idempotency_key.clone();
        let mut project = entry.provider.create_project(request).await?;
        let observed_project_id = valid_project_id(&project.id).then(|| project.id.clone());
        if let Err(error) = normalize_project(&entry.definition, &mut project) {
            return Err(HarnessError::ProjectCreationOutcomeUnknown(Box::new(
                ProjectCreationUncertainty {
                    harness: entry.definition.id.clone(),
                    name: expected_name,
                    root_directory: expected_root,
                    idempotency_key,
                    project_id: observed_project_id,
                    message: error.to_string(),
                },
            )));
        }
        let returned_root = match project.roots.as_slice() {
            [root] => root.clone(),
            _ => {
                return Err(HarnessError::ProjectCreationOutcomeUnknown(Box::new(
                    ProjectCreationUncertainty {
                        harness: entry.definition.id.clone(),
                        name: expected_name,
                        root_directory: expected_root,
                        idempotency_key,
                        project_id: Some(project.id.clone()),
                        message: "created project did not return exactly one root".to_owned(),
                    },
                )));
            }
        };
        if project.name != expected_name || (!remote && returned_root != expected_root) {
            return Err(HarnessError::ProjectCreationOutcomeUnknown(Box::new(
                ProjectCreationUncertainty {
                    harness: entry.definition.id.clone(),
                    name: expected_name,
                    root_directory: expected_root,
                    idempotency_key,
                    project_id: Some(project.id.clone()),
                    message: "created project does not match the requested name and root"
                        .to_owned(),
                },
            )));
        }
        Ok(project)
    }

    pub async fn run(
        &self,
        harness_id: Option<&str>,
        request: HarnessRunRequest,
    ) -> Result<HarnessRunResult, HarnessError> {
        let registry = self.clone();
        let uncertainty = ConversationUncertainty {
            harness: harness_id
                .or(self.default_harness())
                .unwrap_or("selected")
                .into(),
            thread_id: None,
            project_directory: request.project_directory.clone(),
            visibility: request.visibility,
            native_project_id: request.placement.project_id().map(str::to_owned),
            message:
                "service supervisor terminated after dispatch; inspect the original conversation"
                    .into(),
        };
        let harness_id = harness_id.map(str::to_owned);
        self.calls
            .run(request.cancellation.clone(), async move {
                registry.run_inner(harness_id.as_deref(), request).await
            })
            .await
            .map_err(|error| match error {
                crate::framework::DispatchError::Supervisor(_) => {
                    HarnessError::ConversationOutcomeUnknown(Box::new(uncertainty))
                }
                error => dispatch_error(error),
            })?
    }

    async fn run_inner(
        &self,
        harness_id: Option<&str>,
        request: HarnessRunRequest,
    ) -> Result<HarnessRunResult, HarnessError> {
        let entry = self.entry(harness_id)?;
        let validated = validate_run_request(&entry.definition, request)?;
        let expected_directory = validated.project_directory.clone();
        let expected_visibility = validated.visibility;
        let expected_project_id = validated.placement.project_id().map(str::to_owned);
        let result = entry.provider.run(validated.into_request()).await?;
        if result.harness_id != entry.definition.id
            || result.adapter != entry.definition.adapter
            || result.address != entry.definition.address
            || result.project_directory != expected_directory
            || result.visibility != expected_visibility
            || result.native_project_id != expected_project_id
        {
            return Err(HarnessError::ConversationOutcomeUnknown(Box::new(
                ConversationUncertainty {
                    harness: entry.definition.id.clone(),
                    thread_id: valid_project_id(&result.thread_id)
                        .then(|| result.thread_id.clone()),
                    project_directory: expected_directory,
                    visibility: expected_visibility,
                    native_project_id: expected_project_id,
                    message: "provider result does not match the selected Harness instance"
                        .to_owned(),
                },
            )));
        }
        if let Err(error) = validate_client_project_grouping(
            &entry.definition,
            expected_visibility,
            &result.client_project_grouping,
        ) {
            return Err(HarnessError::ConversationOutcomeUnknown(Box::new(
                ConversationUncertainty {
                    harness: entry.definition.id.clone(),
                    thread_id: valid_project_id(&result.thread_id)
                        .then(|| result.thread_id.clone()),
                    project_directory: expected_directory,
                    visibility: expected_visibility,
                    native_project_id: expected_project_id,
                    message: error.to_string(),
                },
            )));
        }
        Ok(result)
    }

    pub async fn shutdown(&self) -> Result<(), HarnessError> {
        self.calls.shutdown().await.map_err(dispatch_error)
    }

    pub fn begin_shutdown(&self) -> Result<(), HarnessError> {
        self.calls.close().map_err(dispatch_error)
    }

    fn entry(&self, harness_id: Option<&str>) -> Result<&RegistryEntry, HarnessError> {
        let id = match harness_id {
            Some(id) => id,
            None => self
                .default_harness
                .as_deref()
                .ok_or(HarnessError::NoDefaultHarness)?,
        };
        let entry = self
            .entries
            .get(id)
            .ok_or_else(|| HarnessError::UnknownHarness(id.to_owned()))?;
        if !entry.definition.enabled {
            return Err(HarnessError::HarnessDisabled(id.to_owned()));
        }
        Ok(entry)
    }
}

fn dispatch_error(error: crate::framework::DispatchError) -> HarnessError {
    HarnessError::Unavailable {
        harness: "registry".into(),
        message: error.to_string(),
    }
}

#[derive(Debug, Error)]
#[error(
    "Harness {harness} project creation may have persisted (name={name}, root={root_directory:?}, idempotency_key={idempotency_key}, project_id={project_id:?}): {message}"
)]
pub struct ProjectCreationUncertainty {
    pub harness: String,
    pub name: String,
    pub root_directory: PathBuf,
    pub idempotency_key: String,
    pub project_id: Option<String>,
    pub message: String,
}

#[derive(Debug, Error)]
#[error(
    "Harness {harness} conversation may persist after an incomplete run (thread_id={thread_id:?}, cwd={project_directory:?}, visibility={visibility:?}, native_project_id={native_project_id:?}): {message}"
)]
pub struct ConversationUncertainty {
    pub harness: String,
    pub thread_id: Option<String>,
    pub project_directory: PathBuf,
    pub visibility: ConversationVisibility,
    pub native_project_id: Option<String>,
    pub message: String,
}

#[derive(Debug, Error)]
pub enum HarnessError {
    #[error("invalid Harness configuration: {0}")]
    InvalidConfiguration(String),
    #[error("duplicate Harness id: {0}")]
    DuplicateHarness(String),
    #[error("duplicate Harness adapter registration: {0}")]
    DuplicateAdapter(String),
    #[error("Harness adapter is not installed: {0}")]
    UnsupportedAdapter(String),
    #[error("Harness address is not supported by adapter {adapter}: {address}")]
    UnsupportedAddress { adapter: String, address: String },
    #[error("unknown Harness: {0}")]
    UnknownHarness(String),
    #[error("Harness is disabled: {0}")]
    HarnessDisabled(String),
    #[error("no default Harness is configured")]
    NoDefaultHarness,
    #[error("project directory is outside the configured workspace roots: {0}")]
    WorkspaceDenied(PathBuf),
    #[error("invalid Harness request: {0}")]
    InvalidRequest(String),
    #[error("Harness {harness} is unavailable: {message}")]
    Unavailable { harness: String, message: String },
    #[error("Harness {harness} requires authentication")]
    AuthenticationRequired { harness: String },
    #[error("Harness {harness} does not support provider-native project discovery")]
    ProjectDiscoveryUnsupported { harness: String },
    #[error("Harness {harness} does not support provider-native project creation")]
    ProjectCreationUnsupported { harness: String },
    #[error("Harness {harness} does not support provider-native project placement")]
    ProjectPlacementUnsupported { harness: String },
    #[error("Harness {harness} has no provider-native project {project_id}")]
    UnknownProject { harness: String, project_id: String },
    #[error("Harness {harness} protocol violation: {message}")]
    ProtocolViolation { harness: String, message: String },
    #[error("Harness {harness} rejected a server-initiated operation: {method}")]
    ToolRequestRejected { harness: String, method: String },
    #[error("Harness {harness} turn exceeded its {seconds}s deadline")]
    DeadlineExceeded { harness: String, seconds: u64 },
    #[error("Harness {harness} output exceeded the configured limit")]
    OutputLimitExceeded { harness: String },
    #[error("Harness {harness} turn was interrupted")]
    Interrupted { harness: String },
    #[error("Harness {harness} turn failed: {message}")]
    TurnFailed { harness: String, message: String },
    #[error(transparent)]
    ProjectCreationOutcomeUnknown(Box<ProjectCreationUncertainty>),
    #[error(transparent)]
    ConversationOutcomeUnknown(Box<ConversationUncertainty>),
}

pub(crate) struct ValidatedRunRequest {
    pub project_directory: PathBuf,
    pub remote_workspace: Option<RemoteWorkspace>,
    pub prompt: String,
    pub model: Option<String>,
    pub timeout: Duration,
    pub visibility: ConversationVisibility,
    pub placement: ConversationPlacement,
    pub cancellation: HarnessCancellation,
    pub role: HarnessRole,
    pub tools: Vec<HarnessTool>,
    pub tool_handler: Option<Arc<dyn HarnessToolHandler>>,
}

impl ValidatedRunRequest {
    fn into_request(self) -> HarnessRunRequest {
        HarnessRunRequest {
            project_directory: self.project_directory,
            remote_workspace: self.remote_workspace,
            prompt: self.prompt,
            model: self.model,
            timeout: self.timeout,
            visibility: self.visibility,
            placement: self.placement,
            cancellation: self.cancellation,
            role: self.role,
            tools: self.tools,
            tool_handler: self.tool_handler,
        }
    }
}

pub(crate) fn validate_run_request(
    definition: &HarnessDefinition,
    request: HarnessRunRequest,
) -> Result<ValidatedRunRequest, HarnessError> {
    request.cancellation.ensure_active(definition)?;
    validate_run_tools(&request)?;
    if request.prompt.trim().is_empty() {
        return Err(HarnessError::InvalidRequest(
            "prompt must not be empty".to_owned(),
        ));
    }
    if request.prompt.len() > MAX_PROMPT_BYTES {
        return Err(HarnessError::InvalidRequest(format!(
            "prompt exceeds {MAX_PROMPT_BYTES} bytes"
        )));
    }
    validate_timeout(request.timeout)?;
    if let Some(model) = request.model.as_deref()
        && (model.is_empty()
            || model.len() > MAX_MODEL_BYTES
            || model.chars().any(char::is_control))
    {
        return Err(HarnessError::InvalidRequest(
            "model identifier is invalid".to_owned(),
        ));
    }
    validate_placement(definition, request.visibility, &request.placement)?;
    let project_directory = validate_directory(
        definition,
        &request.project_directory,
        request.remote_workspace.as_ref(),
        "project directory",
    )?;
    Ok(ValidatedRunRequest {
        project_directory,
        remote_workspace: request.remote_workspace,
        prompt: request.prompt,
        model: request.model,
        timeout: request.timeout,
        visibility: request.visibility,
        placement: request.placement,
        cancellation: request.cancellation,
        role: request.role,
        tools: request.tools,
        tool_handler: request.tool_handler,
    })
}

fn validate_run_tools(request: &HarnessRunRequest) -> Result<(), HarnessError> {
    if request.role == HarnessRole::Approval
        && (!request.tools.is_empty()
            || request.tool_handler.is_some()
            || request.visibility != ConversationVisibility::Hidden
            || request.placement != ConversationPlacement::NoNativeProject)
    {
        return Err(HarnessError::InvalidRequest(
            "approval runs require a fresh hidden conversation without tools or native project"
                .to_owned(),
        ));
    }
    if request.tools.len() > 32 || request.tools.is_empty() != request.tool_handler.is_none() {
        return Err(HarnessError::InvalidRequest(
            "tools require a handler and between 1 and 32 definitions".to_owned(),
        ));
    }
    let mut names = BTreeSet::new();
    let mut schema_bytes = 0_usize;
    for tool in &request.tools {
        if tool.name.is_empty()
            || tool.name.len() > 64
            || !tool
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            || !names.insert(&tool.name)
            || tool.description.trim().is_empty()
            || tool.description.len() > 4096
            || !tool.input_schema.is_object()
            || tool
                .input_schema
                .get("type")
                .and_then(serde_json::Value::as_str)
                != Some("object")
        {
            return Err(HarnessError::InvalidRequest(
                "invalid or duplicate host tool definition".to_owned(),
            ));
        }
        schema_bytes = schema_bytes.saturating_add(tool.input_schema.to_string().len());
    }
    if schema_bytes > 64 * 1024 {
        return Err(HarnessError::InvalidRequest(
            "host tool schemas exceed 64 KiB".to_owned(),
        ));
    }
    Ok(())
}

fn validate_project_list_request(
    definition: &HarnessDefinition,
    mut request: HarnessProjectListRequest,
) -> Result<HarnessProjectListRequest, HarnessError> {
    request.cancellation.ensure_active(definition)?;
    validate_timeout(request.timeout)?;
    request.context_directory = validate_directory(
        definition,
        &request.context_directory,
        request.remote_workspace.as_ref(),
        "project discovery context directory",
    )?;
    Ok(request)
}

fn validate_project_create_request(
    definition: &HarnessDefinition,
    mut request: HarnessProjectCreateRequest,
) -> Result<HarnessProjectCreateRequest, HarnessError> {
    request.cancellation.ensure_active(definition)?;
    validate_timeout(request.timeout)?;
    request.name = request.name.trim().to_owned();
    if !valid_project_name(&request.name) {
        return Err(HarnessError::InvalidRequest(format!(
            "project name must contain 1..={MAX_PROJECT_NAME_BYTES} bytes and no control characters"
        )));
    }
    if request.idempotency_key.trim().is_empty()
        || request.idempotency_key.len() > MAX_IDEMPOTENCY_KEY_BYTES
        || request.idempotency_key.chars().any(char::is_control)
    {
        return Err(HarnessError::InvalidRequest(format!(
            "project idempotency key must contain 1..={MAX_IDEMPOTENCY_KEY_BYTES} bytes and no control characters"
        )));
    }
    request.root_directory = validate_directory(
        definition,
        &request.root_directory,
        request.remote_workspace.as_ref(),
        "project root directory",
    )?;
    Ok(request)
}

fn validate_placement(
    definition: &HarnessDefinition,
    visibility: ConversationVisibility,
    placement: &ConversationPlacement,
) -> Result<(), HarnessError> {
    let ConversationPlacement::ExistingProject {
        harness_id,
        project_id,
    } = placement
    else {
        return Ok(());
    };
    if visibility == ConversationVisibility::Hidden {
        return Err(HarnessError::InvalidRequest(
            "a hidden conversation cannot request a provider-native project".to_owned(),
        ));
    }
    if harness_id != &definition.id {
        return Err(HarnessError::InvalidRequest(format!(
            "provider-native project belongs to Harness {harness_id}, not {}",
            definition.id
        )));
    }
    if !valid_project_id(project_id) {
        return Err(HarnessError::InvalidRequest(format!(
            "project id must contain 1..={MAX_PROJECT_ID_BYTES} bytes and no control characters"
        )));
    }
    Ok(())
}

fn validate_timeout(timeout: Duration) -> Result<(), HarnessError> {
    if timeout.is_zero() || timeout > MAX_TURN_DURATION {
        return Err(HarnessError::InvalidRequest(format!(
            "timeout must be between 1ns and {}s",
            MAX_TURN_DURATION.as_secs()
        )));
    }
    Ok(())
}

fn validate_directory(
    definition: &HarnessDefinition,
    directory: &Path,
    remote: Option<&RemoteWorkspace>,
    kind: &str,
) -> Result<PathBuf, HarnessError> {
    if definition.adapter == REMOTE_NODE_ADAPTER {
        let workspace = remote.ok_or_else(|| {
            HarnessError::InvalidRequest(
                "remote Harness requires an explicit node-owned workspace".into(),
            )
        })?;
        workspace.validate(definition)?;
        Ok(workspace.resource_path())
    } else if remote.is_some() {
        Err(HarnessError::InvalidRequest(
            "local Harness does not accept remote workspaces".into(),
        ))
    } else {
        canonicalize_allowed_directory(definition, directory, kind)
    }
}

fn canonicalize_allowed_directory(
    definition: &HarnessDefinition,
    directory: &Path,
    kind: &str,
) -> Result<PathBuf, HarnessError> {
    let canonical = fs::canonicalize(directory)
        .map_err(|error| HarnessError::InvalidRequest(format!("cannot resolve {kind}: {error}")))?;
    if !canonical.is_dir() {
        return Err(HarnessError::InvalidRequest(format!(
            "{kind} must be an existing directory"
        )));
    }
    if !definition
        .workspace_roots
        .iter()
        .any(|root| canonical.starts_with(root))
    {
        return Err(HarnessError::WorkspaceDenied(canonical));
    }
    Ok(canonical)
}

fn normalize_project_list_result(
    definition: &HarnessDefinition,
    projects: &mut [HarnessProject],
) -> Result<(), HarnessError> {
    if projects.len() > MAX_DISCOVERED_PROJECTS {
        return Err(HarnessError::ProtocolViolation {
            harness: definition.id.clone(),
            message: format!(
                "project discovery returned more than {MAX_DISCOVERED_PROJECTS} projects"
            ),
        });
    }
    let mut ids = BTreeSet::new();
    for project in projects {
        normalize_project(definition, project)?;
        if !ids.insert(project.id.as_str()) {
            return Err(HarnessError::ProtocolViolation {
                harness: definition.id.clone(),
                message: "project discovery returned a duplicate project id".to_owned(),
            });
        }
    }
    Ok(())
}

fn normalize_project(
    definition: &HarnessDefinition,
    project: &mut HarnessProject,
) -> Result<(), HarnessError> {
    if project.harness_id != definition.id {
        return Err(HarnessError::ProtocolViolation {
            harness: definition.id.clone(),
            message: "project result belongs to a different Harness instance".to_owned(),
        });
    }
    if !valid_project_id(&project.id) {
        return Err(HarnessError::ProtocolViolation {
            harness: definition.id.clone(),
            message: "project result contains an invalid project id".to_owned(),
        });
    }
    if !valid_project_name(&project.name) {
        return Err(HarnessError::ProtocolViolation {
            harness: definition.id.clone(),
            message: "project result contains an invalid project name".to_owned(),
        });
    }
    if project.roots.is_empty() || project.roots.len() > MAX_WORKSPACE_ROOTS {
        return Err(HarnessError::ProtocolViolation {
            harness: definition.id.clone(),
            message: "project result must contain bounded absolute roots".to_owned(),
        });
    }
    if definition.adapter == REMOTE_NODE_ADAPTER {
        if project
            .roots
            .iter()
            .any(|root| !remote::valid_remote_path(root))
        {
            return Err(HarnessError::ProtocolViolation {
                harness: definition.id.clone(),
                message: "invalid remote root metadata".into(),
            });
        }
        project.roots.sort();
        project.roots.dedup();
        return Ok(());
    }
    let mut canonical_roots = Vec::with_capacity(project.roots.len());
    for root in &project.roots {
        let canonical =
            provider_path::canonicalize_provider_directory(root, &definition.workspace_roots)
                .map_err(|error| HarnessError::ProtocolViolation {
                    harness: definition.id.clone(),
                    message: format!(
                        "project result contains an invalid or disallowed root: {error}"
                    ),
                })?;
        canonical_roots.push(canonical);
    }
    canonical_roots.sort();
    canonical_roots.dedup();
    project.roots = canonical_roots;
    Ok(())
}

fn validate_client_project_grouping(
    definition: &HarnessDefinition,
    visibility: ConversationVisibility,
    grouping: &ClientProjectGrouping,
) -> Result<(), HarnessError> {
    let valid = match (visibility, grouping) {
        (ConversationVisibility::Hidden, ClientProjectGrouping::NotApplicable) => true,
        (
            ConversationVisibility::Client,
            ClientProjectGrouping::Unverified
            | ClientProjectGrouping::Confirmed {
                client_project_id: None,
            },
        ) => true,
        (
            ConversationVisibility::Client,
            ClientProjectGrouping::Confirmed {
                client_project_id: Some(id),
            },
        ) => valid_project_id(id),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(HarnessError::ProtocolViolation {
            harness: definition.id.clone(),
            message: "provider returned an invalid client project grouping state".to_owned(),
        })
    }
}

fn valid_project_id(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAX_PROJECT_ID_BYTES
        && !value.chars().any(char::is_control)
}

fn valid_project_name(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAX_PROJECT_NAME_BYTES
        && !value.chars().any(char::is_control)
}

fn validate_registry_config(config: &HarnessRegistryConfig) -> Result<(), HarnessError> {
    if config.schema_version != CONFIG_SCHEMA_VERSION {
        return Err(HarnessError::InvalidConfiguration(format!(
            "unsupported schema_version {}; expected {CONFIG_SCHEMA_VERSION}",
            config.schema_version
        )));
    }
    if config.harnesses.is_empty() || config.harnesses.len() > MAX_HARNESSES {
        return Err(HarnessError::InvalidConfiguration(format!(
            "configuration must contain between 1 and {MAX_HARNESSES} Harnesses"
        )));
    }
    let mut ids = BTreeSet::new();
    for harness in &config.harnesses {
        validate_identifier("Harness", &harness.id)?;
        validate_identifier("adapter", &harness.adapter)?;
        validate_address(&harness.address)?;
        if harness.workspace_roots.is_empty() || harness.workspace_roots.len() > MAX_WORKSPACE_ROOTS
        {
            return Err(HarnessError::InvalidConfiguration(format!(
                "Harness {} must contain between 1 and {MAX_WORKSPACE_ROOTS} workspace roots",
                harness.id
            )));
        }
        if !ids.insert(harness.id.clone()) {
            return Err(HarnessError::DuplicateHarness(harness.id.clone()));
        }
    }
    if let Some(default) = config.default_harness.as_deref() {
        validate_identifier("default Harness", default)?;
        let harness = config
            .harnesses
            .iter()
            .find(|harness| harness.id == default)
            .ok_or_else(|| {
                HarnessError::InvalidConfiguration(format!(
                    "default Harness {default} is not configured"
                ))
            })?;
        if !harness.enabled {
            return Err(HarnessError::InvalidConfiguration(format!(
                "default Harness {default} is disabled"
            )));
        }
    }
    Ok(())
}

fn canonicalize_workspace_roots(definition: &mut HarnessDefinition) -> Result<(), HarnessError> {
    if definition.adapter == REMOTE_NODE_ADAPTER {
        remote::validate_remote_definition(definition)?;
        return Ok(());
    }
    for root in &mut definition.workspace_roots {
        let absolute = if root.is_absolute() {
            root.clone()
        } else {
            std::env::current_dir()
                .map_err(|error| HarnessError::InvalidConfiguration(error.to_string()))?
                .join(&*root)
        };
        let canonical = fs::canonicalize(&absolute).map_err(|error| {
            HarnessError::InvalidConfiguration(format!(
                "cannot resolve workspace root for Harness {}: {error}",
                definition.id
            ))
        })?;
        if !canonical.is_dir() {
            return Err(HarnessError::InvalidConfiguration(format!(
                "workspace root for Harness {} is not a directory",
                definition.id
            )));
        }
        *root = canonical;
    }
    definition.workspace_roots.sort();
    definition.workspace_roots.dedup();
    Ok(())
}

fn validate_identifier(kind: &str, value: &str) -> Result<(), HarnessError> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(HarnessError::InvalidConfiguration(format!(
            "{kind} identifier must use 1..={MAX_IDENTIFIER_BYTES} ASCII letters, digits, '.', '_' or '-'"
        )));
    }
    Ok(())
}

fn validate_address(value: &str) -> Result<(), HarnessError> {
    if value.is_empty() || value.len() > MAX_ADDRESS_BYTES || value.chars().any(char::is_control) {
        return Err(HarnessError::InvalidConfiguration(
            "Harness address is empty, too long, or contains control characters".to_owned(),
        ));
    }
    let Some((scheme, _rest)) = value.split_once("://") else {
        return Err(HarnessError::InvalidConfiguration(
            "Harness address must include a URI scheme".to_owned(),
        ));
    };
    let mut bytes = scheme.bytes();
    if !bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
    {
        return Err(HarnessError::InvalidConfiguration(
            "Harness address has an invalid URI scheme".to_owned(),
        ));
    }
    Ok(())
}
