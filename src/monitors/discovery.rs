//! Configuration-authorized inventory polling and bounded durable target enrollment.
use super::*;
use std::sync::atomic::AtomicU64;

type RestoredMonitor = (MonitorState, Arc<dyn ObservationSource>);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorDiscovery {
    pub id: String,
    pub extension_id: String,
    pub contract: String,
    pub version: u32,
    pub method: String,
    pub params: Value,
    pub interval_ms: u64,
    pub timeout_ms: u64,
    pub max_targets: usize,
    pub parameter: String,
    pub template: MonitorDefinition,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryTarget {
    pub key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryBatch {
    pub schema_version: u32,
    pub complete: bool,
    pub targets: Vec<DiscoveryTarget>,
    pub error: Option<String>,
}

pub type DiscoveryFuture<'a> =
    Pin<Box<dyn Future<Output = Result<DiscoveryBatch, MonitorError>> + Send + 'a>>;

#[derive(Clone, Debug, Serialize)]
pub struct DiscoverySnapshot {
    pub id: String,
    pub extension_id: String,
    pub contract: String,
    pub version: u32,
    pub method: String,
    pub running: bool,
    pub last_received_at_ms: Option<u64>,
    pub complete: bool,
    pub known_targets: usize,
    pub present_targets: usize,
    pub last_error: Option<String>,
}

fn key_valid(key: &str) -> bool {
    (1..=64).contains(&key.len())
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn invalid(message: &str) -> MonitorError {
    MonitorError::Configuration(message.into())
}

pub(super) fn validate(config: &MonitorsConfig) -> Result<(), MonitorError> {
    if config.discoveries.len() > 8 {
        return Err(invalid("at most 8 discovery sources are supported"));
    }
    let mut reserved = config.monitors.len();
    let mut ids = BTreeSet::new();
    let mut prefixes = BTreeSet::new();
    if config
        .monitors
        .iter()
        .any(|monitor| monitor.id.starts_with("discovery."))
    {
        return Err(invalid(
            "discovery. monitor IDs are reserved for inventory checkpoints",
        ));
    }
    for item in &config.discoveries {
        if [
            &item.id,
            &item.extension_id,
            &item.contract,
            &item.method,
            &item.parameter,
        ]
        .iter()
        .any(|value| !crate::protocol::valid_id(value))
            || !crate::protocol::valid_id(&format!("discovery.{}", item.id))
            || !ids.insert(&item.id)
            || !prefixes.insert(&item.template.id)
            || item.template.id.starts_with("discovery.")
            || item.version == 0
            || item.contract == "recuvora"
            || item.contract.starts_with("recuvora.")
            || !(1..=64).contains(&item.max_targets)
            || !(10..=3_600_000).contains(&item.interval_ms)
            || !(1..=30_000).contains(&item.timeout_ms)
            || ["target_id", "source_id", "cursor", "generation"].contains(&item.parameter.as_str())
            || !item.params.is_object()
            || serde_json::to_vec(&item.params).map_or(true, |bytes| bytes.len() > 16 * 1024)
        {
            return Err(invalid(
                "invalid discovery identity, duration, limit or parameters",
            ));
        }
        reserved += item.max_targets;
        if reserved > 64 {
            return Err(invalid(
                "static monitors plus discovery reservations exceed 64",
            ));
        }
        MonitorsConfig {
            schema_version: 1,
            monitors: vec![item.template.clone()],
            discoveries: vec![],
        }
        .validate()?;
        if item.template.params.get(&item.parameter).is_some()
            || [
                &item.template.id,
                &item.template.target_id,
                &item.template.source_id,
            ]
            .iter()
            .any(|prefix| !crate::protocol::valid_id(&format!("{prefix}.{}", "k".repeat(64))))
            || config.monitors.iter().any(|monitor| {
                monitor
                    .id
                    .strip_prefix(&format!("{}.", item.template.id))
                    .is_some_and(key_valid)
            })
        {
            return Err(invalid(
                "discovery template prefix conflicts or parameter already exists",
            ));
        }
        // Account for the largest supported key before any remote inventory is accepted.
        let mut expanded = item.template.clone();
        expanded.params[&item.parameter] = json!("k".repeat(64));
        MonitorsConfig {
            schema_version: 1,
            monitors: vec![expanded],
            discoveries: vec![],
        }
        .validate()?;
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InventoryCheckpoint {
    schema_version: u32,
    binding: String,
    keys: Vec<String>,
}

pub(super) struct DiscoveryState {
    config: MonitorDiscovery,
    binding: String,
    sequence: u64,
    targets: BTreeMap<String, Arc<AtomicU64>>,
    view: DiscoverySnapshot,
}

fn inventory_binding(config: &MonitorDiscovery) -> Result<String, MonitorError> {
    serde_json::to_string(&(
        &config.extension_id,
        &config.contract,
        config.version,
        &config.method,
        &config.params,
        &config.parameter,
        &config.template.id,
        binding(&config.template)?,
    ))
    .map_err(|error| invalid(&error.to_string()))
}

impl DiscoveryState {
    pub(super) fn id(&self) -> &str {
        &self.config.id
    }

    fn checkpoint_id(&self) -> String {
        format!("discovery.{}", self.config.id)
    }

    pub(super) fn snapshot(&self) -> DiscoverySnapshot {
        self.view.clone()
    }

    pub(super) fn restore(
        config: MonitorDiscovery,
        store: &IncidentStore,
    ) -> Result<Self, MonitorError> {
        let binding = inventory_binding(&config)?;
        let previous = store.checkpoint(&format!("discovery.{}", config.id));
        let keys = if let Some(previous) = &previous {
            let checkpoint: InventoryCheckpoint = serde_json::from_value(previous.value.clone())
                .map_err(|error| invalid(&format!("invalid discovery checkpoint: {error}")))?;
            if checkpoint.schema_version != 1
                || checkpoint.binding != binding
                || checkpoint.keys.len() > config.max_targets
                || checkpoint.keys.iter().any(|key| !key_valid(key))
                || checkpoint.keys.iter().collect::<BTreeSet<_>>().len() != checkpoint.keys.len()
            {
                return Err(invalid(
                    "discovery checkpoint binding or lifetime target limit changed",
                ));
            }
            checkpoint.keys.into_iter().collect::<BTreeSet<_>>()
        } else {
            BTreeSet::new()
        };
        let view = DiscoverySnapshot {
            id: config.id.clone(),
            extension_id: config.extension_id.clone(),
            contract: config.contract.clone(),
            version: config.version,
            method: config.method.clone(),
            running: true,
            last_received_at_ms: None,
            complete: false,
            known_targets: keys.len(),
            present_targets: 0,
            last_error: None,
        };
        Ok(Self {
            config,
            binding,
            sequence: previous.map_or(0, |value| value.sequence),
            view,
            targets: keys
                .into_iter()
                .map(|key| (key, Arc::new(AtomicU64::new(0))))
                .collect(),
        })
    }

    fn definition(&self, key: &str) -> MonitorDefinition {
        let mut definition = self.config.template.clone();
        definition.id = format!("{}.{}", definition.id, key);
        definition.target_id = format!("{}.{}", definition.target_id, key);
        definition.source_id = format!("{}.{}", definition.source_id, key);
        definition.params[&self.config.parameter] = json!(key);
        definition
    }

    pub(super) fn restored_monitors(
        &self,
        store: &IncidentStore,
        source: Arc<dyn ObservationSource>,
    ) -> Result<Vec<RestoredMonitor>, MonitorError> {
        self.targets
            .iter()
            .map(|(key, present)| {
                let observer: Arc<dyn ObservationSource> = Arc::new(PresentSource {
                    source: source.clone(),
                    present: present.clone(),
                });
                Ok((restore_state(self.definition(key), store)?, observer))
            })
            .collect()
    }

    fn publish(&mut self, shared: &Shared) -> Result<(), MonitorError> {
        self.view.known_targets = self.targets.len();
        self.view.present_targets = self
            .targets
            .values()
            .filter(|present| present.load(Ordering::Acquire) % 2 == 1)
            .count();
        self.view.running = shared.accepting.load(Ordering::Acquire);
        lock(&shared.discoveries)?.insert(self.config.id.clone(), self.view.clone());
        Ok(())
    }

    fn reject(&mut self, message: &str, shared: &Shared) -> Result<(), MonitorError> {
        self.view.complete = false;
        self.view.last_error = Some(bounded_text(message));
        self.publish(shared)
    }

    fn accept(
        &mut self,
        batch: DiscoveryBatch,
        source: Arc<dyn ObservationSource>,
        shared: &Arc<Shared>,
    ) -> Result<(), MonitorError> {
        if batch.schema_version != 1
            || batch.targets.len() > 64
            || batch
                .error
                .as_ref()
                .is_some_and(|error| error.is_empty() || error.len() > 2048)
            || serde_json::to_vec(&batch).map_or(true, |bytes| bytes.len() > MAX_BATCH)
        {
            return Err(MonitorError::Observation(
                "invalid discovery response or limits".into(),
            ));
        }
        let mut present = BTreeSet::new();
        for target in batch.targets {
            if !key_valid(&target.key) || !present.insert(target.key) {
                return Err(MonitorError::Observation(
                    "invalid or duplicate discovered key".into(),
                ));
            }
        }
        let mut known: BTreeSet<String> = self.targets.keys().cloned().collect();
        known.extend(present.iter().cloned());
        if known.len() > self.config.max_targets {
            return Err(MonitorError::Observation(
                "discovery lifetime target capacity exhausted".into(),
            ));
        }
        let _registration = lock(&shared.registration)?;
        if !shared.accepting.load(Ordering::Acquire) {
            return Err(MonitorError::Stopped);
        }
        let additions: Vec<String> = known
            .iter()
            .filter(|key| !self.targets.contains_key(*key))
            .cloned()
            .collect();
        let mut prepared = Vec::new();
        {
            let mut store = lock(&shared.store)?;
            for key in &additions {
                let definition = self.definition(key);
                if lock(&shared.views)?.contains_key(&definition.id) {
                    return Err(invalid(
                        "discovered monitor identity conflicts with existing monitor",
                    ));
                }
                prepared.push((key.clone(), restore_state(definition, &store)?));
            }
            if !additions.is_empty() {
                let sequence = self
                    .sequence
                    .checked_add(1)
                    .ok_or_else(|| invalid("discovery sequence exhausted"))?;
                let checkpoint = InventoryCheckpoint {
                    schema_version: 1,
                    binding: self.binding.clone(),
                    keys: known.into_iter().collect(),
                };
                store.commit(MonitorCommit {
                    monitor_id: self.checkpoint_id(),
                    sequence,
                    checkpoint: serde_json::to_value(checkpoint)
                        .map_err(|error| invalid(&error.to_string()))?,
                    signals: Vec::new(),
                    now_ms: now_ms(),
                })?;
                self.sequence = sequence;
            }
        }
        // Persist all newly accepted identities before publishing or dispatching any worker.
        for (key, state) in prepared {
            let presence = Arc::new(AtomicU64::new(1));
            let observer = Arc::new(PresentSource {
                source: source.clone(),
                present: presence.clone(),
            });
            self.targets.insert(key, presence);
            lock(&shared.definitions)?.insert(state.config.id.clone(), state.config.clone());
            lock(&shared.views)?.insert(state.config.id.clone(), state.view.clone());
            shared.workers.send_modify(|count| *count += 1);
            launch_monitor(state, observer, shared.clone());
        }
        let complete = batch.complete && batch.error.is_none();
        for (key, flag) in &self.targets {
            let previous = flag.load(Ordering::Acquire);
            let expected = if present.contains(key) {
                true
            } else if complete {
                false
            } else {
                previous % 2 == 1
            };
            if previous == 0 && complete && !expected {
                // Zero is unverified after restart; a complete inventory can
                // explicitly establish absence without passing through present.
                flag.store(2, Ordering::Release);
            } else if (previous % 2 == 1) != expected {
                flag.store(
                    previous.checked_add(1).ok_or_else(|| {
                        MonitorError::Runtime("discovery presence epoch exhausted".into())
                    })?,
                    Ordering::Release,
                );
            }
        }
        self.view.last_received_at_ms = Some(now_ms());
        self.view.complete = complete;
        self.view.last_error = batch
            .error
            .or_else(|| (!complete).then(|| "discovery inventory is partial".into()));
        self.publish(shared)
    }
}

struct PresentSource {
    source: Arc<dyn ObservationSource>,
    present: Arc<AtomicU64>,
}
impl ObservationSource for PresentSource {
    fn observation_pending(&self) -> bool {
        self.present.load(Ordering::Acquire) == 0
    }
    fn observation_epoch(&self) -> Option<u64> {
        let epoch = self.present.load(Ordering::Acquire);
        (epoch % 2 == 1).then_some(epoch)
    }
    fn poll(
        &self,
        request: ObservationRequest,
        cancellation: Cancellation,
    ) -> ObservationFuture<'_> {
        Box::pin(async move {
            let missing = || MonitorError::Observation(DISCOVERY_INVALIDATED.into());
            let epoch = self.observation_epoch();
            if epoch.is_none() {
                return Err(missing());
            }
            let result = self.source.poll(request, cancellation).await;
            if self.observation_epoch() != epoch {
                return Err(missing());
            }
            result
        })
    }
}

async fn run(
    mut state: DiscoveryState,
    source: Arc<dyn ObservationSource>,
    shared: Arc<Shared>,
) -> Result<(), MonitorError> {
    loop {
        if !shared.accepting.load(Ordering::Acquire) {
            break;
        }
        let cancellation = Cancellation::new();
        let request = ObservationRequest {
            extension_id: state.config.extension_id.clone(),
            contract: state.config.contract.clone(),
            version: state.config.version,
            method: state.config.method.clone(),
            params: state.config.params.clone(),
            timeout: Duration::from_millis(state.config.timeout_ms),
        };
        let call = source.discover(request, cancellation.clone());
        tokio::pin!(call);
        let result = tokio::select! {
            biased;
            _ = shared.cancellation.cancelled() => { cancellation.cancel(); let _ = call.await; break; },
            result = &mut call => result,
            _ = tokio::time::sleep(Duration::from_millis(state.config.timeout_ms)) => {
                cancellation.cancel();
                state.reject("discovery deadline elapsed", &shared)?;
                let _ = call.await;
                Err(MonitorError::Observation("discovery deadline elapsed".into()))
            }
        };
        match result.and_then(|batch| state.accept(batch, source.clone(), &shared)) {
            Ok(()) => {}
            Err(MonitorError::Stopped) => break,
            Err(error @ MonitorError::Incident(_)) | Err(error @ MonitorError::Runtime(_)) => {
                return Err(error);
            }
            Err(error) => state.reject(&error.to_string(), &shared)?,
        }
        tokio::select! {
            biased;
            _ = shared.cancellation.cancelled() => break,
            _ = tokio::time::sleep(Duration::from_millis(state.config.interval_ms)) => {}
        }
    }
    Ok(())
}

pub(super) fn launch(
    state: DiscoveryState,
    source: Arc<dyn ObservationSource>,
    shared: Arc<Shared>,
) {
    let id = state.config.id.clone();
    let worker_shared = shared.clone();
    let worker = tokio::spawn(async move { run(state, source, worker_shared).await });
    tokio::spawn(async move {
        match worker.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => shared.fail(error.to_string()),
            Err(error) => shared.fail(format!("discovery {id} supervisor failed: {error}")),
        }
        if let Ok(mut views) = shared.discoveries.lock()
            && let Some(view) = views.get_mut(&id)
        {
            view.running = false;
        }
        worker_finished(&shared);
    });
}
