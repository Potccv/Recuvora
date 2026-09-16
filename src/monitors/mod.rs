//! Read-only observation polling and evidence-based monitoring. No repair dispatch.

mod discovery;
pub use discovery::{
    DiscoveryBatch, DiscoveryFuture, DiscoverySnapshot, DiscoveryTarget, MonitorDiscovery,
};

use crate::framework::Cancellation;
use crate::nodes::ExtensionRegistry;
use crate::recovery::incidents::{
    IncidentError, IncidentKind, IncidentRecord, IncidentSignal, IncidentStore,
    IncidentStoreConfig, MonitorCommit, SignalCondition,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::Future;
use std::io::Read;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::sync::watch;
use tokio::time::Instant;

const MAX_CONFIG: usize = 256 * 1024;
const MAX_BATCH: usize = 256 * 1024;
const MAX_SAMPLES: usize = 32;
const MAX_SAFE_NUMBER: f64 = 9_007_199_254_740_992.0;
const DISCOVERY_INVALIDATED: &str =
    "discovered target absent or presence changed; fresh observation required";

#[derive(Debug, Error)]
pub enum MonitorError {
    #[error("invalid monitoring configuration: {0}")]
    Configuration(String),
    #[error("invalid observation: {0}")]
    Observation(String),
    #[error("monitoring runtime: {0}")]
    Runtime(String),
    #[error("monitoring is stopped")]
    Stopped,
    #[error(transparent)]
    Incident(#[from] IncidentError),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorsConfig {
    pub schema_version: u32,
    pub monitors: Vec<MonitorDefinition>,
    #[serde(default)]
    pub discoveries: Vec<MonitorDiscovery>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorDefinition {
    pub id: String,
    pub target_id: String,
    pub source_id: String,
    #[serde(default)]
    pub view_role: Option<String>,
    pub extension_id: String,
    pub contract: String,
    pub version: u32,
    pub method: String,
    pub params: Value,
    pub interval_ms: u64,
    pub timeout_ms: u64,
    pub stale_after_ms: u64,
    pub startup_grace_ms: u64,
    pub rule: MonitorRule,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorRule {
    /// JSON pointer into each sample's value; comparison true means healthy.
    pub pointer: String,
    pub operator: RuleOperator,
    pub value: Value,
    pub failure_samples: u32,
    pub success_samples: u32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleOperator {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

impl MonitorsConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, MonitorError> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .and_then(|file| file.take(MAX_CONFIG as u64 + 1).read_to_end(&mut bytes))
            .map_err(|e| MonitorError::Configuration(e.to_string()))?;
        if bytes.len() > MAX_CONFIG {
            return Err(MonitorError::Configuration(
                "configuration exceeds 256 KiB".into(),
            ));
        }
        let value: Self = serde_json::from_slice(&bytes)
            .map_err(|e| MonitorError::Configuration(e.to_string()))?;
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), MonitorError> {
        let invalid = |message: &str| MonitorError::Configuration(message.into());
        if self.schema_version != 1 || self.monitors.len() > 64 {
            return Err(invalid("expected schema_version 1 and at most 64 monitors"));
        }
        let mut ids = BTreeSet::new();
        for item in &self.monitors {
            if !ids.insert(&item.id)
                || [
                    &item.id,
                    &item.target_id,
                    &item.source_id,
                    &item.extension_id,
                    &item.contract,
                    &item.method,
                ]
                .iter()
                .any(|id| !crate::protocol::valid_id(id))
                || item
                    .view_role
                    .as_deref()
                    .is_some_and(|role| !crate::protocol::valid_id(role))
                || item.version == 0
                || item.contract == "recuvora"
                || item.contract.starts_with("recuvora.")
            {
                return Err(invalid(
                    "invalid identity, duplicate monitor or reserved contract",
                ));
            }
            let Some(params) = item.params.as_object() else {
                return Err(invalid("params must be an object"));
            };
            if ["target_id", "source_id", "cursor", "generation"]
                .iter()
                .any(|key| params.contains_key(*key))
                || serde_json::to_vec(params).map_or(true, |v| v.len() > 16 * 1024)
            {
                return Err(invalid(
                    "params exceeds limit or contains reserved observation fields",
                ));
            }
            if !(10..=3_600_000).contains(&item.interval_ms)
                || !(1..=30_000).contains(&item.timeout_ms)
                || !(10..=86_400_000).contains(&item.stale_after_ms)
                || !(10..=86_400_000).contains(&item.startup_grace_ms)
                || !(1..=1000).contains(&item.rule.failure_samples)
                || !(1..=1000).contains(&item.rule.success_samples)
            {
                return Err(invalid(
                    "monitor durations or sample thresholds outside supported limits",
                ));
            }
            item.rule.validate()?;
        }
        if serde_json::to_vec(self).map_or(true, |v| v.len() > MAX_CONFIG) {
            return Err(invalid("configuration exceeds 256 KiB"));
        }
        discovery::validate(self)?;
        Ok(())
    }
}

impl MonitorRule {
    fn validate(&self) -> Result<(), MonitorError> {
        let invalid = || {
            MonitorError::Configuration(
                "rule requires a valid JSON pointer and bounded bool/number/string comparison"
                    .into(),
            )
        };
        if self.pointer.len() > 1024 || (!self.pointer.is_empty() && !self.pointer.starts_with('/'))
        {
            return Err(invalid());
        }
        let mut chars = self.pointer.chars();
        while let Some(ch) = chars.next() {
            if ch == '~' && !matches!(chars.next(), Some('0' | '1')) {
                return Err(invalid());
            }
        }
        match &self.value {
            Value::Bool(_) => {}
            Value::String(value) if value.len() <= 4096 => {}
            Value::Number(value) if bounded_number(value).is_some() => {}
            _ => return Err(invalid()),
        }
        if !matches!(self.operator, RuleOperator::Eq | RuleOperator::Ne) && !self.value.is_number()
        {
            return Err(invalid());
        }
        if serde_json::to_vec(self).map_or(true, |value| value.len() > 4096) {
            return Err(invalid());
        }
        Ok(())
    }

    fn evaluate(&self, value: &Value) -> Result<bool, MonitorError> {
        let observed = value
            .pointer(&self.pointer)
            .ok_or_else(|| MonitorError::Observation("rule field is missing".into()))?;
        let ordering = match (&self.value, observed) {
            (Value::Bool(expected), Value::Bool(actual)) => actual.partial_cmp(expected),
            (Value::String(expected), Value::String(actual)) if actual.len() <= 4096 => {
                actual.partial_cmp(expected)
            }
            (Value::Number(expected), Value::Number(actual)) => {
                let expected = bounded_number(expected);
                let actual = bounded_number(actual);
                actual.zip(expected).and_then(|(a, b)| a.partial_cmp(&b))
            }
            _ => None,
        }
        .ok_or_else(|| {
            MonitorError::Observation("rule field has an invalid type or number range".into())
        })?;
        Ok(match self.operator {
            RuleOperator::Eq => ordering.is_eq(),
            RuleOperator::Ne => !ordering.is_eq(),
            RuleOperator::Gt => ordering.is_gt(),
            RuleOperator::Ge => !ordering.is_lt(),
            RuleOperator::Lt => ordering.is_lt(),
            RuleOperator::Le => !ordering.is_gt(),
        })
    }
}

fn safe_number(number: f64) -> bool {
    number.is_finite() && number.abs() <= MAX_SAFE_NUMBER
}
fn bounded_number(number: &serde_json::Number) -> Option<f64> {
    if number
        .as_u64()
        .is_some_and(|value| value > 9_007_199_254_740_992)
        || number
            .as_i64()
            .is_some_and(|value| value.unsigned_abs() > 9_007_199_254_740_992)
    {
        return None;
    }
    number.as_f64().filter(|number| safe_number(*number))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationBatch {
    pub schema_version: u32,
    pub target_id: String,
    pub source_id: String,
    pub generation: String,
    /// Echo of the requested cursor. None on the first request.
    pub cursor: Option<String>,
    pub next_cursor: String,
    pub coverage: BatchCoverage,
    pub has_more: bool,
    pub error: Option<String>,
    pub samples: Vec<ObservationSample>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BatchCoverage {
    Complete,
    Partial,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationSample {
    pub id: String,
    pub sequence: u64,
    /// Age measured by the source when it assembles the reply, not a wall clock timestamp.
    pub age_ms: u64,
    pub value: Value,
    pub evidence: Value,
}

#[derive(Clone, Debug)]
pub struct ObservationRequest {
    pub extension_id: String,
    pub contract: String,
    pub version: u32,
    pub method: String,
    pub params: Value,
    pub timeout: Duration,
}

pub type ObservationFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ObservationBatch, MonitorError>> + Send + 'a>>;

pub trait ObservationSource: Send + Sync {
    /// Discovery-backed sources change this epoch whenever presence changes.
    /// None revokes current observation validity without discarding its cursor.
    fn observation_epoch(&self) -> Option<u64> {
        Some(0)
    }
    /// Restored discovery members await a first inventory without bypassing
    /// startup grace. Pending sources must not dispatch a real observation.
    fn observation_pending(&self) -> bool {
        false
    }
    /// Must observe cancellation and remain bounded; the engine drains this future before stopping.
    fn poll(
        &self,
        request: ObservationRequest,
        cancellation: Cancellation,
    ) -> ObservationFuture<'_>;

    fn discover(
        &self,
        _request: ObservationRequest,
        _cancellation: Cancellation,
    ) -> DiscoveryFuture<'_> {
        Box::pin(async {
            Err(MonitorError::Observation(
                "source does not support discovery".into(),
            ))
        })
    }
}

struct RegistrySource(Arc<ExtensionRegistry>);
impl ObservationSource for RegistrySource {
    fn discover(
        &self,
        request: ObservationRequest,
        cancellation: Cancellation,
    ) -> DiscoveryFuture<'_> {
        Box::pin(async move {
            let value = self
                .0
                .call_read_only(
                    &request.extension_id,
                    &request.contract,
                    request.version,
                    &request.method,
                    request.params,
                    request.timeout,
                    cancellation,
                )
                .await
                .map_err(|error| MonitorError::Observation(error.to_string()))?;
            serde_json::from_value(value)
                .map_err(|error| MonitorError::Observation(error.to_string()))
        })
    }
    fn poll(
        &self,
        request: ObservationRequest,
        cancellation: Cancellation,
    ) -> ObservationFuture<'_> {
        Box::pin(async move {
            let value = self
                .0
                .call_read_only(
                    &request.extension_id,
                    &request.contract,
                    request.version,
                    &request.method,
                    request.params,
                    request.timeout,
                    cancellation,
                )
                .await
                .map_err(|e| MonitorError::Observation(e.to_string()))?;
            serde_json::from_value(value).map_err(|e| MonitorError::Observation(e.to_string()))
        })
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TargetHealth {
    Unknown,
    Healthy,
    Unhealthy,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    Missing,
    Fresh,
    Stale,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    Unknown,
    Complete,
    Partial,
    Unavailable,
}

#[derive(Clone, Debug, Serialize)]
pub struct MonitorSnapshot {
    pub id: String,
    pub target_id: String,
    pub source_id: String,
    pub view_role: Option<String>,
    pub extension_id: String,
    pub contract: String,
    pub version: u32,
    pub method: String,
    pub health: TargetHealth,
    pub freshness: Freshness,
    pub coverage: Coverage,
    pub running: bool,
    pub last_received_at_ms: Option<u64>,
    pub last_sample_id: Option<String>,
    pub last_value: Option<Value>,
    pub sample_age_ms: Option<u64>,
    pub consecutive_failures: u32,
    pub consecutive_successes: u32,
    pub generation: Option<String>,
    pub cursor: Option<String>,
    pub last_error: Option<String>,
    pub interval_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct MonitoringSnapshot {
    pub monitors: Vec<MonitorSnapshot>,
    pub discoveries: Vec<DiscoverySnapshot>,
    pub runtime_error: Option<String>,
    pub running: bool,
}

struct Shared {
    store: Mutex<IncidentStore>,
    definitions: Mutex<BTreeMap<String, MonitorDefinition>>,
    views: Mutex<BTreeMap<String, MonitorSnapshot>>,
    discoveries: Mutex<BTreeMap<String, DiscoverySnapshot>>,
    registration: Mutex<()>,
    error: Mutex<Option<String>>,
    cancellation: Cancellation,
    accepting: AtomicBool,
    workers: watch::Sender<usize>,
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, MonitorError> {
    mutex
        .lock()
        .map_err(|_| MonitorError::Runtime("monitor state lock poisoned".into()))
}

impl Shared {
    fn fail(&self, message: String) {
        if let Ok(mut error) = self.error.lock() {
            *error = Some(message.clone());
        }
        if let Ok(mut views) = self.views.lock() {
            for view in views.values_mut() {
                view.running = false;
                view.health = TargetHealth::Unknown;
                view.last_error = Some(message.clone());
            }
        }
        self.accepting.store(false, Ordering::Release);
        self.cancellation.cancel();
        if let Ok(mut discoveries) = self.discoveries.lock() {
            for view in discoveries.values_mut() {
                view.running = false;
            }
        }
    }
}

#[derive(Clone)]
pub struct MonitorHandle {
    shared: Arc<Shared>,
}

impl MonitorHandle {
    pub fn snapshot(&self) -> Result<MonitoringSnapshot, MonitorError> {
        Ok(MonitoringSnapshot {
            monitors: lock(&self.shared.views)?.values().cloned().collect(),
            discoveries: lock(&self.shared.discoveries)?.values().cloned().collect(),
            runtime_error: lock(&self.shared.error)?.clone(),
            running: self.shared.accepting.load(Ordering::Acquire),
        })
    }
    pub fn monitor(&self, id: &str) -> Result<Option<MonitorSnapshot>, MonitorError> {
        Ok(lock(&self.shared.views)?.get(id).cloned())
    }
    /// The trusted enrolled definition, including fixed target parameters.
    /// This is an in-process interface; HTTP summaries do not expose configuration.
    pub fn definition(&self, id: &str) -> Result<Option<MonitorDefinition>, MonitorError> {
        Ok(lock(&self.shared.definitions)?.get(id).cloned())
    }
    pub fn incidents(&self) -> Result<Vec<IncidentRecord>, MonitorError> {
        Ok(lock(&self.shared.store)?.list())
    }
    pub fn map_incidents<T>(
        &self,
        projection: impl FnMut(&IncidentRecord) -> T,
    ) -> Result<Vec<T>, MonitorError> {
        Ok(lock(&self.shared.store)?.map_records(projection))
    }
    pub fn incident(&self, id: &str) -> Result<Option<IncidentRecord>, MonitorError> {
        Ok(lock(&self.shared.store)?.get(id))
    }
    pub fn acknowledge(
        &self,
        id: &str,
        expected_revision: u64,
        actor: &str,
        note: &str,
    ) -> Result<IncidentRecord, MonitorError> {
        let mut store = lock(&self.shared.store)?;
        if !self.shared.accepting.load(Ordering::Acquire) {
            return Err(MonitorError::Stopped);
        }
        match store.acknowledge(id, expected_revision, actor, note, now_ms()) {
            Ok(record) => Ok(record),
            Err(error) => {
                if matches!(
                    error,
                    IncidentError::Io(_)
                        | IncidentError::Unavailable(_)
                        | IncidentError::Corrupt(_)
                        | IncidentError::Capacity(_)
                ) {
                    self.shared.fail(error.to_string());
                }
                Err(error.into())
            }
        }
    }
    pub fn begin_shutdown(&self) -> Result<(), MonitorError> {
        let _registration = lock(&self.shared.registration)?;
        self.shared.accepting.store(false, Ordering::Release);
        self.shared.cancellation.cancel();
        for view in lock(&self.shared.views)?.values_mut() {
            view.running = false;
            view.health = TargetHealth::Unknown;
        }
        for view in lock(&self.shared.discoveries)?.values_mut() {
            view.running = false;
        }
        if *self.shared.workers.borrow() == 0 {
            lock(&self.shared.store)?.close()?;
        }
        Ok(())
    }
    pub async fn drain(&self) -> Result<(), MonitorError> {
        let mut workers = self.shared.workers.subscribe();
        loop {
            if *workers.borrow_and_update() == 0 {
                break;
            }
            workers
                .changed()
                .await
                .map_err(|_| MonitorError::Runtime("monitor supervisor disappeared".into()))?;
        }
        lock(&self.shared.store)?.close()?;
        if let Some(error) = lock(&self.shared.error)?.clone() {
            return Err(MonitorError::Runtime(error));
        }
        Ok(())
    }
}

pub struct MonitorEngine {
    handle: MonitorHandle,
}

impl MonitorEngine {
    pub fn start(
        config: MonitorsConfig,
        registry: Arc<ExtensionRegistry>,
        data_dir: impl AsRef<Path>,
    ) -> Result<Self, MonitorError> {
        Self::start_with_source(config, Arc::new(RegistrySource(registry)), data_dir)
    }

    pub fn start_with_source(
        config: MonitorsConfig,
        source: Arc<dyn ObservationSource>,
        data_dir: impl AsRef<Path>,
    ) -> Result<Self, MonitorError> {
        config.validate()?;
        tokio::runtime::Handle::try_current().map_err(|e| MonitorError::Runtime(e.to_string()))?;
        let store = IncidentStore::open(
            data_dir.as_ref().join("incidents.jsonl"),
            IncidentStoreConfig::default(),
        )?;
        let mut states = Vec::new();
        let mut views = BTreeMap::new();
        let mut definitions = BTreeMap::new();
        for definition in config.monitors {
            let state = restore_state(definition, &store)?;
            definitions.insert(state.config.id.clone(), state.config.clone());
            views.insert(state.config.id.clone(), state.view.clone());
            states.push((state, source.clone()));
        }
        let mut discoveries = Vec::new();
        let mut discovery_views = BTreeMap::new();
        for definition in config.discoveries {
            let discovery = discovery::DiscoveryState::restore(definition, &store)?;
            for (state, observer) in discovery.restored_monitors(&store, source.clone())? {
                definitions.insert(state.config.id.clone(), state.config.clone());
                views.insert(state.config.id.clone(), state.view.clone());
                states.push((state, observer));
            }
            discovery_views.insert(discovery.id().to_owned(), discovery.snapshot());
            discoveries.push(discovery);
        }
        let (workers, _) = watch::channel(states.len() + discoveries.len());
        let shared = Arc::new(Shared {
            store: Mutex::new(store),
            definitions: Mutex::new(definitions),
            views: Mutex::new(views),
            discoveries: Mutex::new(discovery_views),
            registration: Mutex::new(()),
            error: Mutex::new(None),
            cancellation: Cancellation::new(),
            accepting: AtomicBool::new(true),
            workers,
        });
        for (state, observer) in states {
            launch_monitor(state, observer, shared.clone());
        }
        for discovery in discoveries {
            discovery::launch(discovery, source.clone(), shared.clone());
        }
        Ok(Self {
            handle: MonitorHandle { shared },
        })
    }

    pub fn handle(&self) -> MonitorHandle {
        self.handle.clone()
    }
    pub async fn shutdown(&mut self) -> Result<(), MonitorError> {
        self.handle.begin_shutdown()?;
        self.handle.drain().await
    }
}

impl Drop for MonitorEngine {
    fn drop(&mut self) {
        let _ = self.handle.begin_shutdown();
    }
}

fn restore_state(
    definition: MonitorDefinition,
    store: &IncidentStore,
) -> Result<MonitorState, MonitorError> {
    let previous = store.checkpoint(&definition.id);
    let checkpoint = match &previous {
        Some(previous) => serde_json::from_value::<SourceCheckpoint>(previous.value.clone())
            .map_err(|error| {
                MonitorError::Configuration(format!("invalid monitor checkpoint: {error}"))
            })?,
        None => SourceCheckpoint::new(&definition)?,
    };
    if checkpoint.binding != binding(&definition)? {
        return Err(MonitorError::Configuration(format!(
            "monitor {} changed source identity; use a new monitor ID",
            definition.id
        )));
    }
    Ok(MonitorState::new(
        definition,
        checkpoint,
        previous.map_or(0, |value| value.sequence),
    ))
}

// Registration reserves ownership before spawning; shutdown cannot observe zero
// while a discovery supervisor is still able to add an observation worker.
fn launch_monitor(state: MonitorState, source: Arc<dyn ObservationSource>, shared: Arc<Shared>) {
    let id = state.config.id.clone();
    let worker_shared = shared.clone();
    let worker = tokio::spawn(async move { run_monitor(state, source, worker_shared).await });
    tokio::spawn(async move {
        match worker.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => shared.fail(error.to_string()),
            Err(error) => shared.fail(format!("monitor {id} supervisor failed: {error}")),
        }
        if let Ok(mut views) = shared.views.lock()
            && let Some(view) = views.get_mut(&id)
        {
            view.running = false;
            view.health = TargetHealth::Unknown;
        }
        worker_finished(&shared);
    });
}

fn worker_finished(shared: &Shared) {
    let result = (|| -> Result<(), MonitorError> {
        let _registration = lock(&shared.registration)?;
        shared
            .workers
            .send_modify(|count| *count = count.saturating_sub(1));
        if *shared.workers.borrow() == 0 && !shared.accepting.load(Ordering::Acquire) {
            lock(&shared.store)?.close()?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        shared.fail(error.to_string());
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceCheckpoint {
    binding: String,
    generation: Option<String>,
    cursor: Option<String>,
    last_sequence: Option<u64>,
    recent_ids: VecDeque<String>,
    retired_generations: VecDeque<String>,
}

fn binding(config: &MonitorDefinition) -> Result<String, MonitorError> {
    serde_json::to_string(&(
        &config.target_id,
        &config.source_id,
        &config.extension_id,
        &config.contract,
        config.version,
        &config.method,
        &config.params,
    ))
    .map_err(|e| MonitorError::Configuration(e.to_string()))
}
impl SourceCheckpoint {
    fn new(config: &MonitorDefinition) -> Result<Self, MonitorError> {
        Ok(Self {
            binding: binding(config)?,
            generation: None,
            cursor: None,
            last_sequence: None,
            recent_ids: VecDeque::new(),
            retired_generations: VecDeque::new(),
        })
    }
}

struct MonitorState {
    config: MonitorDefinition,
    checkpoint: SourceCheckpoint,
    committed_checkpoint: SourceCheckpoint,
    commit_sequence: u64,
    view: MonitorSnapshot,
    started: Instant,
    last_sample: Option<(Instant, u64)>,
    coverage_problem: Option<String>,
}

impl MonitorState {
    fn new(config: MonitorDefinition, checkpoint: SourceCheckpoint, commit_sequence: u64) -> Self {
        let view = MonitorSnapshot {
            id: config.id.clone(),
            target_id: config.target_id.clone(),
            source_id: config.source_id.clone(),
            view_role: config.view_role.clone(),
            extension_id: config.extension_id.clone(),
            contract: config.contract.clone(),
            version: config.version,
            method: config.method.clone(),
            health: TargetHealth::Unknown,
            freshness: Freshness::Missing,
            coverage: Coverage::Unknown,
            running: true,
            last_received_at_ms: None,
            last_sample_id: None,
            last_value: None,
            sample_age_ms: None,
            consecutive_failures: 0,
            consecutive_successes: 0,
            generation: checkpoint.generation.clone(),
            cursor: checkpoint.cursor.clone(),
            last_error: None,
            interval_ms: config.interval_ms,
        };
        Self {
            config,
            committed_checkpoint: checkpoint.clone(),
            checkpoint,
            commit_sequence,
            view,
            started: Instant::now(),
            last_sample: None,
            coverage_problem: None,
        }
    }

    fn request(&self) -> ObservationRequest {
        let mut params = self.config.params.clone();
        params["target_id"] = json!(self.config.target_id);
        params["source_id"] = json!(self.config.source_id);
        params["cursor"] = json!(self.checkpoint.cursor);
        params["generation"] = json!(self.checkpoint.generation);
        ObservationRequest {
            extension_id: self.config.extension_id.clone(),
            contract: self.config.contract.clone(),
            version: self.config.version,
            method: self.config.method.clone(),
            params,
            timeout: Duration::from_millis(self.config.timeout_ms),
        }
    }

    fn signal(
        &self,
        kind: IncidentKind,
        condition: SignalCondition,
        summary: &str,
        mut evidence: Value,
    ) -> IncidentSignal {
        evidence["rule"] = json!(self.config.rule);
        IncidentSignal {
            monitor_id: self.config.id.clone(),
            target_id: self.config.target_id.clone(),
            rule_id: self.config.id.clone(),
            kind,
            condition,
            summary: summary.into(),
            evidence,
        }
    }

    fn evidence(&self, reason: &str) -> Value {
        json!({"reason":reason,"source_id":self.config.source_id,"generation":self.checkpoint.generation,
            "cursor":self.checkpoint.cursor,"sample_id":self.view.last_sample_id,"value":self.view.last_value})
    }

    fn unknown(&mut self) {
        self.view.health = TargetHealth::Unknown;
        self.view.consecutive_failures = 0;
        self.view.consecutive_successes = 0;
    }

    fn coverage_failure(&mut self, reason: String, coverage: Coverage) -> Vec<IncidentSignal> {
        let reason = bounded_text(&reason);
        let target_changed = self.view.health != TargetHealth::Unknown;
        self.unknown();
        self.view.coverage = coverage;
        self.view.last_error = Some(reason.clone());
        let changed = self.coverage_problem.as_ref() != Some(&reason);
        self.coverage_problem = Some(reason.clone());
        let mut signals = Vec::new();
        if changed {
            signals.push(self.signal(
                IncidentKind::Coverage,
                SignalCondition::Active,
                &reason,
                self.evidence(&reason),
            ));
        }
        if changed || target_changed {
            signals.push(self.signal(
                IncidentKind::Target,
                SignalCondition::Unknown,
                "target lacks current complete evidence",
                self.evidence(&reason),
            ));
        }
        signals
    }

    fn tick(&mut self) -> Vec<IncidentSignal> {
        if let Some((received, age)) = self.last_sample {
            let age = age.saturating_add(duration_ms(received.elapsed()));
            self.view.sample_age_ms = Some(age);
            if age >= self.config.stale_after_ms {
                self.view.freshness = Freshness::Stale;
                if self.view.health != TargetHealth::Unknown
                    || self.coverage_problem.as_deref() != Some("sample is stale")
                {
                    return self.coverage_failure("sample is stale".into(), self.view.coverage);
                }
            }
        } else if duration_ms(self.started.elapsed()) >= self.config.startup_grace_ms
            && self.coverage_problem.as_deref() != Some("no fresh sample after startup grace")
        {
            return self.coverage_failure(
                "no fresh sample after startup grace".into(),
                self.view.coverage,
            );
        }
        Vec::new()
    }

    fn accept(
        &mut self,
        batch: ObservationBatch,
        elapsed: Duration,
    ) -> Result<Vec<IncidentSignal>, MonitorError> {
        self.validate_batch(&batch)?;
        self.view.last_received_at_ms = Some(now_ms());
        let changed_generation = self
            .checkpoint
            .generation
            .as_ref()
            .is_some_and(|generation| generation != &batch.generation);
        if changed_generation {
            if self
                .checkpoint
                .retired_generations
                .contains(&batch.generation)
            {
                return Err(MonitorError::Observation(
                    "retired source generation replayed".into(),
                ));
            }
            if let Some(old) = self.checkpoint.generation.take() {
                self.checkpoint.retired_generations.push_back(old);
                if self.checkpoint.retired_generations.len() > 16 {
                    self.checkpoint.retired_generations.pop_front();
                }
            }
            self.checkpoint.last_sequence = None;
            self.checkpoint.recent_ids.clear();
            self.last_sample = None;
            self.view.freshness = Freshness::Missing;
            self.unknown();
        }
        self.checkpoint.generation = Some(batch.generation.clone());
        let complete = batch.coverage == BatchCoverage::Complete
            && !batch.has_more
            && batch.error.is_none()
            && !changed_generation;
        self.view.coverage = if complete {
            Coverage::Complete
        } else {
            Coverage::Partial
        };
        let reason = if changed_generation {
            "source generation changed; continuity lost".to_string()
        } else {
            batch
                .error
                .clone()
                .unwrap_or_else(|| "observation coverage is partial".into())
        };
        let mut signals = if complete {
            Vec::new()
        } else {
            self.coverage_failure(reason, Coverage::Partial)
        };
        let mut fresh_accepted = false;
        let mut sample_problem = None;
        let empty = batch.samples.is_empty();
        for sample in batch.samples {
            if self
                .checkpoint
                .last_sequence
                .is_some_and(|last| sample.sequence <= last)
                || self.checkpoint.recent_ids.contains(&sample.id)
            {
                continue;
            }
            self.checkpoint.last_sequence = Some(sample.sequence);
            self.checkpoint.recent_ids.push_back(sample.id.clone());
            if self.checkpoint.recent_ids.len() > MAX_SAMPLES {
                self.checkpoint.recent_ids.pop_front();
            }
            let age = sample.age_ms.saturating_add(duration_ms(elapsed));
            if age >= self.config.stale_after_ms {
                self.unknown();
                revoke_pending_clears(&mut signals);
                sample_problem = Some("batch contains stale samples".into());
                // Historical data can be retained in evidence, never refresh a current sample.
                if self.last_sample.is_none() {
                    self.last_sample = Some((Instant::now(), age));
                    self.view.freshness = Freshness::Stale;
                    self.view.sample_age_ms = Some(age);
                    self.view.last_sample_id = Some(sample.id);
                    self.view.last_value = Some(sample.value.clone());
                }
                continue;
            }
            let healthy = match self.config.rule.evaluate(&sample.value) {
                Ok(healthy) => healthy,
                Err(error) => {
                    sample_problem = Some(error.to_string());
                    self.unknown();
                    revoke_pending_clears(&mut signals);
                    continue;
                }
            };
            fresh_accepted = true;
            self.last_sample = Some((Instant::now(), age));
            self.view.freshness = Freshness::Fresh;
            self.view.sample_age_ms = Some(age);
            self.view.last_sample_id = Some(sample.id.clone());
            self.view.last_value = Some(sample.value.clone());
            let evidence = json!({"source_id":self.config.source_id,"generation":batch.generation,
                "sample_id":sample.id,"sequence":sample.sequence,"age_ms":age,"value":sample.value,"source_evidence":sample.evidence});
            if healthy && complete {
                self.view.consecutive_failures = 0;
                self.view.consecutive_successes = self
                    .view
                    .consecutive_successes
                    .saturating_add(1)
                    .min(self.config.rule.success_samples);
                if self.view.consecutive_successes >= self.config.rule.success_samples {
                    self.view.health = TargetHealth::Healthy;
                    signals.push(self.signal(
                        IncidentKind::Target,
                        SignalCondition::Clear,
                        "fresh samples satisfy healthy rule",
                        evidence,
                    ));
                }
            } else if !healthy {
                self.view.consecutive_successes = 0;
                self.view.consecutive_failures = self
                    .view
                    .consecutive_failures
                    .saturating_add(1)
                    .min(self.config.rule.failure_samples);
                if self.view.consecutive_failures >= self.config.rule.failure_samples {
                    self.view.health = TargetHealth::Unhealthy;
                    signals.push(self.signal(
                        IncidentKind::Target,
                        SignalCondition::Active,
                        "fresh samples violate healthy rule",
                        evidence,
                    ));
                } else {
                    revoke_pending_clears(&mut signals);
                    if self.view.health == TargetHealth::Healthy {
                        self.view.health = TargetHealth::Unknown;
                    }
                    signals.push(self.signal(
                        IncidentKind::Target,
                        SignalCondition::Unknown,
                        "unhealthy samples below fault threshold",
                        evidence,
                    ));
                }
            } else {
                self.view.consecutive_successes = 0;
            }
        }
        if empty {
            self.view.consecutive_failures = 0;
            self.view.consecutive_successes = 0;
        }
        self.checkpoint.cursor = Some(batch.next_cursor);
        self.view.cursor = self.checkpoint.cursor.clone();
        self.view.generation = self.checkpoint.generation.clone();
        if let Some(problem) = sample_problem {
            revoke_pending_clears(&mut signals);
            signals.extend(self.coverage_failure(problem, Coverage::Partial));
        } else if complete && fresh_accepted {
            self.coverage_problem = None;
            self.view.last_error = None;
            signals.retain(|signal| !matches!(signal.kind, IncidentKind::Coverage));
            signals.push(self.signal(
                IncidentKind::Coverage,
                SignalCondition::Clear,
                "fresh complete observation restored coverage",
                self.evidence("fresh complete observation"),
            ));
        }
        Ok(signals)
    }

    fn validate_batch(&self, batch: &ObservationBatch) -> Result<(), MonitorError> {
        let invalid = |reason: &str| MonitorError::Observation(reason.into());
        if batch
            .samples
            .iter()
            .any(|sample| !bounded_depth(&sample.value, 24) || !bounded_depth(&sample.evidence, 24))
        {
            return Err(invalid("sample value/evidence nesting exceeds 24 levels"));
        }
        if serde_json::to_vec(batch).map_or(true, |bytes| bytes.len() > MAX_BATCH)
            || batch.samples.len() > MAX_SAMPLES
            || batch.schema_version != 1
            || batch.target_id != self.config.target_id
            || batch.source_id != self.config.source_id
            || batch.cursor != self.checkpoint.cursor
            || !crate::protocol::valid_id(&batch.generation)
            || batch.next_cursor.is_empty()
            || batch.next_cursor.len() > 4096
            || batch
                .error
                .as_ref()
                .is_some_and(|error| error.is_empty() || error.len() > 2048)
        {
            return Err(invalid(
                "batch identity, cursor, generation or limits invalid",
            ));
        }
        let mut identities = BTreeMap::<&str, &ObservationSample>::new();
        for sample in &batch.samples {
            if let Some(previous) = identities.insert(&sample.id, sample)
                && (previous.sequence != sample.sequence
                    || previous.value != sample.value
                    || previous.evidence != sample.evidence)
            {
                return Err(invalid(
                    "sample identity is reused with contradictory content",
                ));
            }
            if self.checkpoint.generation.as_ref() == Some(&batch.generation)
                && self
                    .checkpoint
                    .last_sequence
                    .is_some_and(|last| sample.sequence > last)
                && self.checkpoint.recent_ids.contains(&sample.id)
            {
                return Err(invalid("sample identity is reused for a newer sequence"));
            }
            if !crate::protocol::valid_id(&sample.id)
                || sample.sequence == 0
                || !sample.evidence.is_object()
                || sample.sequence > 9_007_199_254_740_992
                || sample.age_ms > 9_007_199_254_740_992
                || serde_json::to_vec(&sample.evidence).map_or(true, |v| v.len() > 4096)
                || serde_json::to_vec(&sample.value).map_or(true, |v| v.len() > 4096)
            {
                return Err(invalid(
                    "sample identity, sequence or evidence/value limit invalid",
                ));
            }
        }
        for pair in batch.samples.windows(2) {
            if pair[1].sequence < pair[0].sequence
                || (pair[1].sequence == pair[0].sequence
                    && (pair[1].id != pair[0].id
                        || pair[1].value != pair[0].value
                        || pair[1].evidence != pair[0].evidence))
            {
                return Err(invalid(
                    "batch samples are out of order or contradict a sequence identity",
                ));
            }
        }
        let evidence_bytes: usize = batch
            .samples
            .iter()
            .map(|sample| {
                serde_json::to_vec(&sample.value).map_or(MAX_BATCH, |bytes| bytes.len())
                    + serde_json::to_vec(&sample.evidence).map_or(MAX_BATCH, |bytes| bytes.len())
            })
            .sum();
        if evidence_bytes > 128 * 1024 {
            return Err(invalid("batch evidence exceeds 128 KiB"));
        }
        Ok(())
    }

    fn publish(&self, shared: &Shared) -> Result<(), MonitorError> {
        let mut view = self.view.clone();
        if !shared.accepting.load(Ordering::Acquire) {
            view.running = false;
            view.health = TargetHealth::Unknown;
            if let Some(error) = lock(&shared.error)?.clone() {
                view.last_error = Some(error);
            }
        }
        lock(&shared.views)?.insert(self.config.id.clone(), view);
        Ok(())
    }

    fn commit(
        &mut self,
        shared: &Shared,
        signals: Vec<IncidentSignal>,
    ) -> Result<(), MonitorError> {
        if signals.is_empty() && self.checkpoint == self.committed_checkpoint {
            return self.publish(shared);
        }
        let sequence = self
            .commit_sequence
            .checked_add(1)
            .ok_or_else(|| MonitorError::Runtime("monitor sequence exhausted".into()))?;
        let checkpoint = serde_json::to_value(&self.checkpoint)
            .map_err(|e| MonitorError::Runtime(e.to_string()))?;
        lock(&shared.store)?.commit(MonitorCommit {
            monitor_id: self.config.id.clone(),
            sequence,
            checkpoint,
            signals,
            now_ms: now_ms(),
        })?;
        self.commit_sequence = sequence;
        self.committed_checkpoint = self.checkpoint.clone();
        self.publish(shared)
    }
}

async fn run_monitor(
    mut state: MonitorState,
    source: Arc<dyn ObservationSource>,
    shared: Arc<Shared>,
) -> Result<(), MonitorError> {
    let mut next_poll = Instant::now();
    let mut observed_epoch = source.observation_epoch();
    let tick_ms = state
        .config
        .interval_ms
        .min(state.config.stale_after_ms)
        .min(state.config.startup_grace_ms)
        .min(250);
    let mut ticks = tokio::time::interval(Duration::from_millis(tick_ms));
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = shared.cancellation.cancelled() => break,
            _ = ticks.tick() => {
                let _registration = lock(&shared.registration)?;
                let current_epoch = source.observation_epoch();
                let signals = if (current_epoch.is_none() && !source.observation_pending())
                    || (observed_epoch.is_some() && current_epoch != observed_epoch) {
                    state.coverage_failure(DISCOVERY_INVALIDATED.into(), Coverage::Unavailable)
                } else { state.tick() };
                observed_epoch = current_epoch;
                if !signals.is_empty() { state.commit(&shared, signals)?; } else { state.publish(&shared)?; }
            },
            _ = tokio::time::sleep_until(next_poll) => {
                if !shared.accepting.load(Ordering::Acquire) { break; }
                if source.observation_pending() {
                    // Keep the independent freshness/grace clock alive while
                    // waiting for inventory; no observation future is created.
                    next_poll = Instant::now() + Duration::from_millis(tick_ms);
                    continue;
                }
                let start = Instant::now();
                let epoch = source.observation_epoch();
                let request = state.request();
                let cancellation = Cancellation::new();
                let poll = source.poll(request, cancellation.clone());
                tokio::pin!(poll);
                let deadline = tokio::time::sleep(Duration::from_millis(state.config.timeout_ms));
                tokio::pin!(deadline);
                let mut timed_out = false;
                let result = loop {
                    tokio::select! {
                        biased;
                        _ = shared.cancellation.cancelled() => { cancellation.cancel(); let _ = poll.await; break None; },
                        result = &mut poll => break Some(result),
                        _ = &mut deadline, if !timed_out => {
                            timed_out = true;
                            cancellation.cancel();
                            let signals = state.coverage_failure("observation deadline elapsed".into(), Coverage::Unavailable);
                            if let Err(error) = state.commit(&shared, signals) {
                                shared.fail(error.to_string());
                                let _ = poll.await;
                                return Err(error);
                            }
                        },
                        _ = ticks.tick() => {
                            let result = {
                                let _registration = lock(&shared.registration)?;
                                let signals = if source.observation_epoch() != epoch || epoch.is_none() {
                                    cancellation.cancel();
                                    state.coverage_failure(DISCOVERY_INVALIDATED.into(), Coverage::Unavailable)
                                } else { state.tick() };
                                if !signals.is_empty() { state.commit(&shared, signals) } else { state.publish(&shared) }
                            };
                            if let Err(error) = result {
                                shared.fail(error.to_string());
                                cancellation.cancel();
                                let _ = poll.await;
                                return Err(error);
                            }
                        }
                    }
                };
                let Some(result) = result else { break; };
                let _registration = lock(&shared.registration)?;
                let current_epoch = source.observation_epoch();
                let mut signals = if observed_epoch.is_some() && current_epoch != observed_epoch {
                    state.coverage_failure(DISCOVERY_INVALIDATED.into(), Coverage::Unavailable)
                } else { Vec::new() };
                observed_epoch = current_epoch;
                let result = if current_epoch != epoch || epoch.is_none() {
                    Err(MonitorError::Observation(DISCOVERY_INVALIDATED.into()))
                } else if timed_out { Err(MonitorError::Observation("observation deadline elapsed".into())) } else { result };
                signals.extend(match result.and_then(|batch| state.accept(batch, start.elapsed())) {
                    Ok(signals) => signals,
                    Err(error) => state.coverage_failure(error.to_string(), Coverage::Unavailable),
                });
                state.commit(&shared, signals)?;
                next_poll = Instant::now() + Duration::from_millis(state.config.interval_ms);
            }
        }
    }
    Ok(())
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
fn now_ms() -> u64 {
    duration_ms(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default(),
    )
}
fn bounded_text(value: &str) -> String {
    value.chars().take(500).collect()
}
fn bounded_depth(value: &Value, maximum: usize) -> bool {
    let mut pending = vec![(value, 0usize)];
    while let Some((value, depth)) = pending.pop() {
        if depth > maximum {
            return false;
        }
        match value {
            Value::Object(object) => {
                pending.extend(object.values().map(|child| (child, depth + 1)))
            }
            Value::Array(array) => pending.extend(array.iter().map(|child| (child, depth + 1))),
            _ => {}
        }
    }
    true
}
fn revoke_pending_clears(signals: &mut Vec<IncidentSignal>) {
    while let Some(index) = signals
        .iter()
        .rposition(|signal| matches!(signal.kind, IncidentKind::Target))
    {
        if !matches!(signals[index].condition, SignalCondition::Clear) {
            break;
        }
        signals.remove(index);
    }
}
