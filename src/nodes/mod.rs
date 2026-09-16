//! Host-side registration and routing; external programs are independently shipped.
use crate::framework::{CallScope, Cancellation};
use crate::protocol::{
    self, CallbackFuture, CallbackHandler, CommandSpec, ContractDeclaration, ExtensionCall,
    ExtensionClient, ExtensionError, ExtensionKind, ExtensionMetadata,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

pub const MONITORING_VIEW_CAPABILITY: &str = "recuvora.monitoring_view.v1";
pub const MONITORING_VIEW_METHOD: &str = "describe_monitoring_view";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AllowedMethod {
    pub contract: String,
    pub version: u32,
    pub method: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionDefinition {
    pub id: String,
    pub kind: ExtensionKind,
    #[serde(default = "enabled")]
    pub enabled: bool,
    pub command: CommandSpec,
    #[serde(default)]
    pub namespaces: Vec<String>,
    #[serde(default)]
    pub allow_calls: Vec<AllowedMethod>,
    #[serde(default)]
    pub allow_nodes: Vec<String>,
}
fn enabled() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionsConfig {
    pub schema_version: u32,
    pub extensions: Vec<ExtensionDefinition>,
}
impl ExtensionsConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ExtensionError> {
        let path = std::fs::canonicalize(path)
            .map_err(|e| ExtensionError::Configuration(e.to_string()))?;
        let base = path
            .parent()
            .ok_or_else(|| ExtensionError::Configuration("configuration has no parent".into()))?;
        let mut data = Vec::new();
        std::fs::File::open(&path)
            .map_err(|e| ExtensionError::Configuration(e.to_string()))?
            .take(256 * 1024 + 1)
            .read_to_end(&mut data)
            .map_err(|e| ExtensionError::Configuration(e.to_string()))?;
        if data.len() > 256 * 1024 {
            return Err(ExtensionError::Configuration(
                "configuration exceeds 256 KiB".into(),
            ));
        }
        let mut config: Self = serde_json::from_slice(&data)
            .map_err(|e| ExtensionError::Configuration(e.to_string()))?;
        for definition in &mut config.extensions {
            if definition.command.program.is_relative() {
                definition.command.program = base.join(&definition.command.program);
            }
            if definition.command.cwd.is_relative() {
                definition.command.cwd = base.join(&definition.command.cwd);
            }
        }
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<(), ExtensionError> {
        if self.schema_version != 1 || self.extensions.len() > 64 {
            return Err(ExtensionError::Configuration(
                "expected schema_version 1 and at most 64 extensions".into(),
            ));
        }
        let mut ids = BTreeSet::new();
        let mut namespaces = Vec::<String>::new();
        for definition in &self.extensions {
            if !protocol::valid_id(&definition.id)
                || !ids.insert(&definition.id)
                || definition.namespaces.len() > 16
                || definition.allow_calls.len() > 128
                || definition.allow_nodes.len() > 64
            {
                return Err(ExtensionError::Configuration(
                    "invalid or duplicate extension identity/limits".into(),
                ));
            }
            definition.command.validate()?;
            for namespace in &definition.namespaces {
                if definition.kind != ExtensionKind::Plugin
                    || !protocol::valid_id(namespace)
                    || namespace == "recuvora"
                    || namespace.starts_with("recuvora.")
                    || namespaces.iter().any(|n| {
                        n == namespace
                            || n.starts_with(&format!("{namespace}."))
                            || namespace.starts_with(&format!("{n}."))
                    })
                {
                    return Err(ExtensionError::Configuration(
                        "invalid, reserved or conflicting namespace".into(),
                    ));
                }
                namespaces.push(namespace.clone());
            }
            for allowed in &definition.allow_calls {
                if !protocol::valid_id(&allowed.contract)
                    || !protocol::valid_id(&allowed.method)
                    || allowed.version == 0
                {
                    return Err(ExtensionError::Configuration(
                        "invalid call allowlist".into(),
                    ));
                }
            }
            if definition
                .allow_nodes
                .iter()
                .any(|id| !protocol::valid_id(id))
            {
                return Err(ExtensionError::Configuration("invalid allowed node".into()));
            }
        }
        Ok(())
    }
}

struct Entry {
    definition: ExtensionDefinition,
    client: ExtensionClient,
    metadata: ExtensionMetadata,
    ordinary: Arc<Semaphore>,
    monitoring_view: Arc<Semaphore>,
    approval: Arc<Semaphore>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ExtensionStatus {
    pub id: String,
    pub kind: ExtensionKind,
    pub available: bool,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MonitoringViewRegistration {
    pub extension_id: String,
    pub contract: String,
    pub version: u32,
    pub method: String,
}
#[derive(Clone)]
pub struct ExtensionRegistry {
    entries: BTreeMap<String, Arc<Entry>>,
    definitions: Vec<ExtensionDefinition>,
    statuses: Vec<ExtensionStatus>,
    contract_owners: BTreeMap<(String, u32), String>,
    calls: CallScope,
}

fn dispatch_error(error: crate::framework::DispatchError) -> ExtensionError {
    match error {
        crate::framework::DispatchError::Supervisor(message) => ExtensionError::Unknown {
            // No fabricated wire identifier: the supervisor may have exited
            // before obtaining the remote call ID. Original caller context is
            // retained by the domain request and must be reconciled there.
            call_id: String::new(),
            message: format!(
                "supervisor exited after dispatch; remote call ID unavailable: {message}"
            ),
        },
        error => ExtensionError::Unavailable(error.to_string()),
    }
}

impl ExtensionRegistry {
    pub async fn connect(config: ExtensionsConfig) -> Result<Self, ExtensionError> {
        config.validate()?;
        let original_definitions = config.extensions.clone();
        let mut statuses = Vec::new();
        let mut entries = BTreeMap::new();
        let mut contracts = BTreeMap::<(String, u32), ContractDeclaration>::new();
        let mut contract_owners = BTreeMap::<(String, u32), String>::new();
        // Consumers own new contracts. Nodes can only implement a contract whose
        // trusted plugin has already registered an identical declaration.
        let mut definitions = config
            .extensions
            .into_iter()
            .filter(|d| d.enabled)
            .collect::<Vec<_>>();
        definitions.sort_by_key(|d| {
            if d.kind == ExtensionKind::Plugin {
                0
            } else {
                1
            }
        });
        for definition in definitions {
            let client = ExtensionClient {
                id: definition.id.clone(),
                kind: definition.kind,
                command: definition.command.clone(),
            };
            let registration: Result<ExtensionMetadata, ExtensionError> =
                async {
                    let metadata = client.probe().await?;
                    validate_metadata(&definition, &metadata)?;
                    let mut staged = contracts.clone();
                    let mut staged_owners = contract_owners.clone();
                    for contract in &metadata.contracts {
                        let key = (contract.id.clone(), contract.version);
                        if definition.kind == ExtensionKind::Plugin {
                            if !definition.namespaces.iter().any(|n| {
                                contract.id == *n || contract.id.starts_with(&format!("{n}."))
                            }) {
                                return Err(ExtensionError::Rejected(
                                    "contract outside configured plugin namespace".into(),
                                ));
                            }
                            if staged.insert(key, contract.clone()).is_some() {
                                return Err(ExtensionError::Rejected(
                                    "duplicate contract registration".into(),
                                ));
                            }
                            staged_owners.insert(
                                (contract.id.clone(), contract.version),
                                definition.id.clone(),
                            );
                        } else if contract.id != "recuvora.harness"
                            && contracts.get(&key) != Some(contract)
                        {
                            return Err(ExtensionError::Rejected(format!(
                                "node {} has missing or incompatible consumer for {}",
                                definition.id, contract.id
                            )));
                        } else if contract.id == "recuvora.harness" && contract.version != 1 {
                            return Err(ExtensionError::Rejected(
                                "unsupported Harness contract version".into(),
                            ));
                        }
                    }
                    contracts = staged;
                    contract_owners = staged_owners;
                    Ok(metadata)
                }
                .await;
            let metadata = match registration {
                Ok(metadata) => {
                    statuses.push(ExtensionStatus {
                        id: definition.id.clone(),
                        kind: definition.kind,
                        available: true,
                        error: None,
                    });
                    metadata
                }
                Err(error) => {
                    statuses.push(ExtensionStatus {
                        id: definition.id.clone(),
                        kind: definition.kind,
                        available: false,
                        error: Some(error.to_string()),
                    });
                    continue;
                }
            };
            entries.insert(
                definition.id.clone(),
                Arc::new(Entry {
                    definition,
                    client,
                    metadata,
                    ordinary: Arc::new(Semaphore::new(4)),
                    monitoring_view: Arc::new(Semaphore::new(1)),
                    approval: Arc::new(Semaphore::new(2)),
                }),
            );
        }
        for definition in &original_definitions {
            if !definition.enabled {
                statuses.push(ExtensionStatus {
                    id: definition.id.clone(),
                    kind: definition.kind,
                    available: false,
                    error: Some("disabled by configuration".into()),
                });
            }
        }
        Ok(Self {
            entries,
            definitions: original_definitions,
            statuses,
            contract_owners,
            calls: CallScope::default(),
        })
    }
    pub fn definitions(&self) -> Vec<ExtensionDefinition> {
        self.definitions.clone()
    }
    pub fn statuses(&self) -> &[ExtensionStatus] {
        &self.statuses
    }
    pub fn metadata(&self, id: &str) -> Option<&ExtensionMetadata> {
        self.entries.get(id).map(|e| &e.metadata)
    }

    pub fn contract_owner(&self, contract: &str, version: u32) -> Option<&str> {
        self.contract_owners
            .get(&(contract.to_owned(), version))
            .map(String::as_str)
    }

    pub fn monitoring_view_registration(
        &self,
        id: &str,
    ) -> Result<Option<MonitoringViewRegistration>, ExtensionError> {
        let entry = self.entry(id)?;
        if !entry
            .metadata
            .capabilities
            .iter()
            .any(|capability| capability == MONITORING_VIEW_CAPABILITY)
        {
            return Ok(None);
        }
        if entry.definition.kind != ExtensionKind::Plugin {
            return Err(ExtensionError::Rejected(
                "monitoring view registration requires a plugin".into(),
            ));
        }
        let candidates = entry
            .metadata
            .contracts
            .iter()
            .flat_map(|contract| {
                contract
                    .methods
                    .iter()
                    .filter(|method| method.name == MONITORING_VIEW_METHOD)
                    .map(move |method| (contract, method))
            })
            .collect::<Vec<_>>();
        if candidates.len() != 1 {
            return Err(ExtensionError::Rejected(
                "monitoring view capability requires exactly one descriptor method".into(),
            ));
        }
        let (contract, method) = candidates[0];
        let owned = contract.id != "recuvora"
            && !contract.id.starts_with("recuvora.")
            && self.contract_owner(&contract.id, contract.version)
                == Some(entry.definition.id.as_str());
        if !owned || !method.read_only {
            return Err(ExtensionError::Rejected(
                "monitoring view descriptor must be a read-only method in the plugin namespace"
                    .into(),
            ));
        }
        protocol::validate_value(&method.input_schema, &json!({"schema_version":1})).map_err(
            |_| {
                ExtensionError::Rejected(
                    "monitoring view descriptor must accept the fixed schema_version 1 input"
                        .into(),
                )
            },
        )?;
        if !entry.definition.allow_calls.iter().any(|allowed| {
            allowed.contract == contract.id
                && allowed.version == contract.version
                && allowed.method == method.name
        }) {
            return Err(ExtensionError::Rejected(
                "monitoring view descriptor is not in the trusted call allowlist".into(),
            ));
        }
        Ok(Some(MonitoringViewRegistration {
            extension_id: entry.definition.id.clone(),
            contract: contract.id.clone(),
            version: contract.version,
            method: method.name.clone(),
        }))
    }

    /// Calls an optional plugin monitoring-view descriptor without granting it
    /// the node-read callback authority available to ordinary plugin calls.
    pub async fn call_monitoring_view(
        &self,
        registration: &MonitoringViewRegistration,
        params: Value,
        timeout: Duration,
        cancellation: Cancellation,
    ) -> Result<Value, ExtensionError> {
        let registry = self.clone();
        let registration = registration.clone();
        self.calls
            .run(cancellation.clone(), async move {
                registry
                    .call_monitoring_view_inner(&registration, params, timeout, cancellation)
                    .await
            })
            .await
            .map_err(dispatch_error)?
    }

    async fn call_monitoring_view_inner(
        &self,
        registration: &MonitoringViewRegistration,
        params: Value,
        timeout: Duration,
        cancellation: Cancellation,
    ) -> Result<Value, ExtensionError> {
        let current = self
            .monitoring_view_registration(&registration.extension_id)?
            .ok_or_else(|| {
                ExtensionError::Rejected("extension did not register a monitoring view".into())
            })?;
        if &current != registration {
            return Err(ExtensionError::Rejected(
                "monitoring view registration does not match the current declaration".into(),
            ));
        }
        let entry = self.entry(&registration.extension_id)?;
        let declaration = method_for(
            entry,
            &registration.contract,
            registration.version,
            &registration.method,
        )?;
        if !declaration.read_only
            || registration.contract == "recuvora"
            || registration.contract.starts_with("recuvora.")
        {
            return Err(ExtensionError::Rejected(
                "monitoring view descriptor must be a non-reserved read-only method".into(),
            ));
        }
        protocol::validate_value(&declaration.input_schema, &params)?;
        let permit = entry
            .monitoring_view
            .clone()
            .try_acquire_owned()
            .map_err(|_| ExtensionError::Rejected("monitoring view capacity exhausted".into()))?;
        let result = entry
            .client
            .call(
                ExtensionCall {
                    contract: registration.contract.clone(),
                    version: registration.version,
                    method: registration.method.clone(),
                    params,
                    timeout,
                },
                entry.metadata.clone(),
                cancellation,
                // View declarations are data-only. In particular, they cannot
                // use the normal plugin callback router to read from nodes.
                None,
                Some(permit),
            )
            .await?;
        protocol::validate_value(&declaration.output_schema, &result).map_err(|error| {
            ExtensionError::Protocol(format!(
                "monitoring view descriptor output violates its schema: {error}"
            ))
        })?;
        Ok(result)
    }

    pub async fn shutdown(&self) -> Result<(), ExtensionError> {
        self.calls
            .shutdown()
            .await
            .map_err(|error| ExtensionError::Unavailable(error.to_string()))
    }

    pub fn begin_shutdown(&self) -> Result<(), ExtensionError> {
        self.calls
            .close()
            .map_err(|error| ExtensionError::Unavailable(error.to_string()))
    }

    // The explicit contract/version/method triplet is the wire routing identity;
    // keeping it separate from cancellation and payload makes authority visible.
    #[allow(clippy::too_many_arguments)]
    pub async fn call_read_only(
        &self,
        id: &str,
        contract: &str,
        version: u32,
        method: &str,
        params: Value,
        timeout: Duration,
        cancellation: Cancellation,
    ) -> Result<Value, ExtensionError> {
        let registry = self.clone();
        let (id, contract, method) = (id.to_owned(), contract.to_owned(), method.to_owned());
        self.calls
            .run(cancellation.clone(), async move {
                registry
                    .call_read_only_inner(
                        &id,
                        &contract,
                        version,
                        &method,
                        params,
                        timeout,
                        cancellation,
                    )
                    .await
            })
            .await
            .map_err(dispatch_error)?
    }

    #[allow(clippy::too_many_arguments)]
    async fn call_read_only_inner(
        &self,
        id: &str,
        contract: &str,
        version: u32,
        method: &str,
        params: Value,
        timeout: Duration,
        cancellation: Cancellation,
    ) -> Result<Value, ExtensionError> {
        let entry = self.entry(id)?;
        let declaration = method_for(entry, contract, version, method)?;
        if !declaration.read_only || contract.starts_with("recuvora.") {
            return Err(ExtensionError::Rejected(
                "generic extension calls permit only non-reserved read-only contracts".into(),
            ));
        }
        protocol::validate_value(&declaration.input_schema, &params)?;
        let handler = if entry.definition.kind == ExtensionKind::Plugin {
            Some(Arc::new(ReadRouter {
                registry: self.clone(),
                allowed: entry.definition.allow_nodes.clone(),
            }) as Arc<dyn CallbackHandler>)
        } else {
            None
        };
        let permit = entry
            .ordinary
            .clone()
            .try_acquire_owned()
            .map_err(|_| ExtensionError::Rejected("extension capacity exhausted".into()))?;
        let result = entry
            .client
            .call(
                ExtensionCall {
                    contract: contract.into(),
                    version,
                    method: method.into(),
                    params,
                    timeout,
                },
                entry.metadata.clone(),
                cancellation,
                handler,
                Some(permit),
            )
            .await;
        let result = result?;
        protocol::validate_value(&declaration.output_schema, &result)?;
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn call_harness(
        &self,
        id: &str,
        method: &str,
        params: Value,
        timeout: Duration,
        cancellation: Cancellation,
        handler: Option<Arc<dyn CallbackHandler>>,
        approval: bool,
    ) -> Result<Value, ExtensionError> {
        let registry = self.clone();
        let (id, method) = (id.to_owned(), method.to_owned());
        self.calls
            .run(cancellation.clone(), async move {
                registry
                    .call_harness_inner(
                        &id,
                        &method,
                        params,
                        timeout,
                        cancellation,
                        handler,
                        approval,
                    )
                    .await
            })
            .await
            .map_err(dispatch_error)?
    }

    #[allow(clippy::too_many_arguments)]
    async fn call_harness_inner(
        &self,
        id: &str,
        method: &str,
        params: Value,
        timeout: Duration,
        cancellation: Cancellation,
        handler: Option<Arc<dyn CallbackHandler>>,
        approval: bool,
    ) -> Result<Value, ExtensionError> {
        let entry = self.entry(id)?;
        if entry.definition.kind != ExtensionKind::Node {
            return Err(ExtensionError::Rejected(
                "Harness provider must be a node".into(),
            ));
        }
        let _ = method_for(entry, "recuvora.harness", 1, method)?;
        let permit = if approval {
            entry.approval.clone()
        } else {
            entry.ordinary.clone()
        }
        .try_acquire_owned()
        .map_err(|_| ExtensionError::Rejected("Harness role capacity exhausted".into()))?;
        entry
            .client
            .call(
                ExtensionCall {
                    contract: "recuvora.harness".into(),
                    version: 1,
                    method: method.into(),
                    params,
                    timeout,
                },
                entry.metadata.clone(),
                cancellation,
                handler,
                Some(permit),
            )
            .await
    }
    fn entry(&self, id: &str) -> Result<&Entry, ExtensionError> {
        self.entries
            .get(id)
            .map(AsRef::as_ref)
            .ok_or_else(|| ExtensionError::Unavailable(format!("extension {id} not registered")))
    }
}

fn method_for<'a>(
    entry: &'a Entry,
    contract: &str,
    version: u32,
    method: &str,
) -> Result<&'a protocol::MethodDeclaration, ExtensionError> {
    if !entry
        .definition
        .allow_calls
        .iter()
        .any(|a| a.contract == contract && a.version == version && a.method == method)
    {
        return Err(ExtensionError::Rejected(
            "method is not in the trusted configuration allowlist".into(),
        ));
    }
    entry
        .metadata
        .contracts
        .iter()
        .find(|c| c.id == contract && c.version == version)
        .and_then(|c| c.methods.iter().find(|m| m.name == method))
        .ok_or_else(|| ExtensionError::Rejected("method was not registered".into()))
}

fn validate_metadata(
    definition: &ExtensionDefinition,
    metadata: &ExtensionMetadata,
) -> Result<(), ExtensionError> {
    if metadata.contracts.len() > 32
        || metadata.capabilities.len() > 64
        || metadata.workspaces.len() > 64
    {
        return Err(ExtensionError::Protocol("metadata limit exceeded".into()));
    }
    let mut contracts = BTreeSet::new();
    for contract in &metadata.contracts {
        if !protocol::valid_id(&contract.id)
            || contract.version == 0
            || contract.methods.is_empty()
            || contract.methods.len() > 32
            || !contracts.insert((&contract.id, contract.version))
        {
            return Err(ExtensionError::Protocol(
                "invalid or duplicate contract".into(),
            ));
        }
        let mut methods = BTreeSet::new();
        for method in &contract.methods {
            if !protocol::valid_id(&method.name) || !methods.insert(&method.name) {
                return Err(ExtensionError::Protocol(
                    "invalid or duplicate method".into(),
                ));
            }
            protocol::validate_schema(&method.input_schema)?;
            protocol::validate_schema(&method.output_schema)?;
            // A malformed optional view must not disable the plugin itself.
            // It remains uncallable unless monitoring_view_registration later
            // accepts the unique read-only declaration and trusted allowlist.
            if definition.kind == ExtensionKind::Plugin
                && !method.read_only
                && !(metadata
                    .capabilities
                    .iter()
                    .any(|capability| capability == MONITORING_VIEW_CAPABILITY)
                    && method.name == MONITORING_VIEW_METHOD)
            {
                return Err(ExtensionError::Rejected(
                    "new plugin methods currently must be read-only".into(),
                ));
            }
        }
    }
    for list in [&metadata.capabilities, &metadata.workspaces] {
        let mut seen = BTreeSet::new();
        for value in list {
            if !protocol::valid_id(value) || !seen.insert(value) {
                return Err(ExtensionError::Protocol(
                    "invalid or duplicate capability/workspace".into(),
                ));
            }
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadRequest {
    node_id: String,
    contract: String,
    version: u32,
    method: String,
    params: Value,
}
struct ReadRouter {
    registry: ExtensionRegistry,
    allowed: Vec<String>,
}
impl CallbackHandler for ReadRouter {
    fn call<'a>(
        &'a self,
        method: String,
        params: Value,
        cancellation: Cancellation,
    ) -> CallbackFuture<'a> {
        Box::pin(async move {
            if method != "service.call" {
                return Err(ExtensionError::Rejected(
                    "unsupported plugin callback".into(),
                ));
            }
            let request: ReadRequest = serde_json::from_value(params)
                .map_err(|e| ExtensionError::Rejected(e.to_string()))?;
            if !self.allowed.contains(&request.node_id)
                || self
                    .registry
                    .metadata(&request.node_id)
                    .is_none_or(|m| m.kind != ExtensionKind::Node)
            {
                return Err(ExtensionError::Rejected(
                    "plugin cannot access this node".into(),
                ));
            }
            self.registry
                .call_read_only(
                    &request.node_id,
                    &request.contract,
                    request.version,
                    &request.method,
                    request.params,
                    Duration::from_secs(30),
                    cancellation,
                )
                .await
        })
    }
}
