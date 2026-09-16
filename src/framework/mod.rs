//! Lifecycle and service composition for trusted, cooperative in-process modules.
//!
//! A runtime has one startup/shutdown cycle. Scope is an exact string namespace;
//! there is no hierarchy, hot replacement, or process isolation in this crate.

mod cancellation;
pub use cancellation::{CallScope, Cancellation, DispatchError};

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::timeout;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct InstanceId(pub String);

impl InstanceId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl std::fmt::Display for InstanceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Exact service identity. Major versions and scopes never implicitly fall back.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ServiceKey {
    pub name: String,
    pub major: u32,
    pub scope: String,
}

impl ServiceKey {
    pub fn new(name: impl Into<String>, major: u32, scope: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            major,
            scope: scope.into(),
        }
    }
}

impl std::fmt::Display for ServiceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}[{}]", self.name, self.major, self.scope)
    }
}

#[derive(Clone, Debug)]
pub struct ModuleMetadata {
    pub instance: InstanceId,
    pub provides: Vec<ServiceKey>,
    pub requires: Vec<ServiceKey>,
}

impl ModuleMetadata {
    pub fn new(instance: InstanceId) -> Self {
        Self {
            instance,
            provides: Vec::new(),
            requires: Vec::new(),
        }
    }

    pub fn provides(mut self, service: ServiceKey) -> Self {
        self.provides.push(service);
        self
    }

    pub fn requires(mut self, service: ServiceKey) -> Self {
        self.requires.push(service);
        self
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("{message}")]
pub struct ModuleError {
    pub message: String,
}

impl ModuleError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

pub type LifecycleFuture<'a> = Pin<Box<dyn Future<Output = Result<(), ModuleError>> + Send + 'a>>;

/// Hooks must yield, be cancellation safe, and finish cleanup after partial startup.
/// Stop must tolerate a retry if its future was cancelled by the caller.
pub trait Module: Send {
    fn metadata(&self) -> ModuleMetadata;
    fn start<'a>(&'a mut self, context: &'a ModuleContext) -> LifecycleFuture<'a>;
    fn stop<'a>(&'a mut self, _context: &'a ModuleContext) -> LifecycleFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Clone, Debug)]
pub struct LifecycleOptions {
    pub start_timeout: Duration,
    pub stop_timeout: Duration,
    pub cleanup_timeout: Duration,
    pub max_modules: usize,
    pub max_resources_per_module: usize,
    pub max_subscriptions_per_module: usize,
    pub max_event_queue: usize,
    pub max_event_bytes: usize,
}

impl Default for LifecycleOptions {
    fn default() -> Self {
        Self {
            start_timeout: Duration::from_secs(5),
            stop_timeout: Duration::from_secs(5),
            cleanup_timeout: Duration::from_secs(2),
            max_modules: 64,
            max_resources_per_module: 128,
            max_subscriptions_per_module: 64,
            max_event_queue: 256,
            max_event_bytes: 64 * 1024,
        }
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum FrameworkError {
    #[error("invalid configuration: {0}")]
    InvalidConfiguration(String),
    #[error("runtime already started or closed")]
    RuntimeNotFresh,
    #[error("duplicate instance: {0}")]
    DuplicateInstance(InstanceId),
    #[error("service provider conflict: {0}")]
    ProviderConflict(ServiceKey),
    #[error("{instance} requires missing service {service}")]
    MissingDependency {
        instance: InstanceId,
        service: ServiceKey,
    },
    #[error("cyclic module dependencies: {0:?}")]
    DependencyCycle(Vec<InstanceId>),
    #[error("module {instance} did not publish declared service {service}")]
    UnpublishedService {
        instance: InstanceId,
        service: ServiceKey,
    },
    #[error("service {service} is not declared by {instance}")]
    UndeclaredService {
        instance: InstanceId,
        service: ServiceKey,
    },
    #[error("service is unavailable: {0}")]
    ServiceUnavailable(ServiceKey),
    #[error("service Rust type does not match: {0}")]
    ServiceTypeMismatch(ServiceKey),
    #[error("instance is no longer accepting work: {0}")]
    InstanceInactive(InstanceId),
    #[error("capacity exceeded: {0}")]
    CapacityExceeded(String),
    #[error("{instance} {phase} failed: {message}")]
    LifecycleFailure {
        instance: InstanceId,
        phase: &'static str,
        message: String,
    },
    #[error("{instance} {phase} timed out")]
    LifecycleTimeout {
        instance: InstanceId,
        phase: &'static str,
    },
    #[error("framework state lock was poisoned")]
    StatePoisoned,
}

impl From<FrameworkError> for ModuleError {
    fn from(error: FrameworkError) -> Self {
        Self::new(error.to_string())
    }
}

#[derive(Debug, Default)]
pub struct ShutdownReport {
    pub issues: Vec<FrameworkError>,
}

impl ShutdownReport {
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }
}

#[derive(Debug, Error)]
#[error("startup failed: {cause}; cleanup issues: {cleanup:?}")]
pub struct StartupError {
    pub cause: FrameworkError,
    pub cleanup: ShutdownReport,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Event {
    pub topic: String,
    pub data: String,
}

/// An event is ephemeral. Full queues drop this event and are reported to its sender.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DeliveryReport {
    pub delivered: usize,
    pub full: usize,
    pub closed: usize,
}

pub struct EventSubscription {
    receiver: mpsc::Receiver<Event>,
}

impl EventSubscription {
    pub async fn recv(&mut self) -> Option<Event> {
        self.receiver.recv().await
    }

    pub fn try_recv(&mut self) -> Result<Event, mpsc::error::TryRecvError> {
        self.receiver.try_recv()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RuntimeSnapshot {
    pub active_instances: usize,
    pub pending_cleanup_instances: usize,
    pub services: usize,
    pub subscriptions: usize,
    pub disposers: usize,
    pub background_tasks: usize,
}

struct ServiceEntry {
    owner: InstanceId,
    value: Arc<dyn Any + Send + Sync>,
}

#[derive(Default)]
struct Resources {
    disposers: Vec<Box<dyn FnOnce() + Send>>,
    tasks: Vec<JoinHandle<()>>,
}

struct InstanceState {
    metadata: ModuleMetadata,
    accepting: bool,
    resources: Resources,
}

struct Listener {
    owner: InstanceId,
    topic: String,
    sender: mpsc::Sender<Event>,
}

#[derive(Default)]
struct SharedState {
    instances: BTreeMap<InstanceId, InstanceState>,
    services: BTreeMap<ServiceKey, ServiceEntry>,
    listeners: Vec<Listener>,
}

impl SharedState {
    fn active(&self, instance: &InstanceId) -> Result<&InstanceState, FrameworkError> {
        self.instances
            .get(instance)
            .filter(|state| state.accepting)
            .ok_or_else(|| FrameworkError::InstanceInactive(instance.clone()))
    }

    fn take_resources(&mut self, instance: &InstanceId) -> Resources {
        self.services
            .retain(|_, service| &service.owner != instance);
        self.listeners
            .retain(|listener| &listener.owner != instance);
        if let Some(state) = self.instances.get_mut(instance) {
            state.accepting = false;
            std::mem::take(&mut state.resources)
        } else {
            Resources::default()
        }
    }
}

/// Clones share one instance ownership boundary and become inactive at shutdown.
#[derive(Clone)]
pub struct ModuleContext {
    instance: InstanceId,
    state: Arc<Mutex<SharedState>>,
    options: LifecycleOptions,
}

impl ModuleContext {
    fn lock(&self) -> Result<MutexGuard<'_, SharedState>, FrameworkError> {
        self.state.lock().map_err(|_| FrameworkError::StatePoisoned)
    }

    pub fn instance(&self) -> &InstanceId {
        &self.instance
    }

    pub fn is_accepting(&self) -> bool {
        self.lock()
            .is_ok_and(|state| state.active(&self.instance).is_ok())
    }

    /// Publish a sized service value; a contract may wrap an Arc<dyn Trait>.
    pub fn publish<T: Any + Send + Sync>(
        &self,
        service: ServiceKey,
        value: Arc<T>,
    ) -> Result<(), FrameworkError> {
        let mut state = self.lock()?;
        if !state
            .active(&self.instance)?
            .metadata
            .provides
            .contains(&service)
        {
            return Err(FrameworkError::UndeclaredService {
                instance: self.instance.clone(),
                service,
            });
        }
        if state.services.contains_key(&service) {
            return Err(FrameworkError::ProviderConflict(service));
        }
        state.services.insert(
            service,
            ServiceEntry {
                owner: self.instance.clone(),
                value,
            },
        );
        Ok(())
    }

    /// Resolution is restricted to declared requirements and owned services.
    /// Already acquired Arcs require the service's own dispatch/drain discipline.
    pub fn service<T: Any + Send + Sync>(
        &self,
        service: &ServiceKey,
    ) -> Result<Arc<T>, FrameworkError> {
        let state = self.lock()?;
        let owner = state.active(&self.instance)?;
        if !owner.metadata.requires.contains(service) && !owner.metadata.provides.contains(service)
        {
            return Err(FrameworkError::UndeclaredService {
                instance: self.instance.clone(),
                service: service.clone(),
            });
        }
        let entry = state
            .services
            .get(service)
            .ok_or_else(|| FrameworkError::ServiceUnavailable(service.clone()))?;
        state.active(&entry.owner)?;
        Arc::downcast(entry.value.clone())
            .map_err(|_| FrameworkError::ServiceTypeMismatch(service.clone()))
    }

    /// Callbacks are quick synchronous disposers, invoked in reverse registration order.
    pub fn on_dispose(
        &self,
        dispose: impl FnOnce() + Send + 'static,
    ) -> Result<(), FrameworkError> {
        let mut state = self.lock()?;
        self.check_resource_capacity(&state)?;
        let owner = state
            .instances
            .get_mut(&self.instance)
            .ok_or_else(|| FrameworkError::InstanceInactive(self.instance.clone()))?;
        owner.resources.disposers.push(Box::new(dispose));
        Ok(())
    }

    /// Owned async tasks are aborted and joined at cleanup; futures must cooperate.
    pub fn spawn(
        &self,
        task: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), FrameworkError> {
        let mut state = self.lock()?;
        self.check_resource_capacity(&state)?;
        let owner = state
            .instances
            .get_mut(&self.instance)
            .ok_or_else(|| FrameworkError::InstanceInactive(self.instance.clone()))?;
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| {
            FrameworkError::InvalidConfiguration("background task requires a Tokio runtime".into())
        })?;
        owner.resources.tasks.push(runtime.spawn(task));
        Ok(())
    }

    fn check_resource_capacity(&self, state: &SharedState) -> Result<(), FrameworkError> {
        let owner = state.active(&self.instance)?;
        if owner.resources.disposers.len() + owner.resources.tasks.len()
            >= self.options.max_resources_per_module
        {
            return Err(FrameworkError::CapacityExceeded(
                "instance resources".into(),
            ));
        }
        Ok(())
    }

    pub fn subscribe(
        &self,
        topic: impl Into<String>,
        capacity: usize,
    ) -> Result<EventSubscription, FrameworkError> {
        let topic = topic.into();
        if topic.is_empty()
            || topic.len() > self.options.max_event_bytes
            || capacity == 0
            || capacity > self.options.max_event_queue
            || capacity > tokio::sync::Semaphore::MAX_PERMITS
        {
            return Err(FrameworkError::InvalidConfiguration(
                "event topic or queue capacity".into(),
            ));
        }
        let mut state = self.lock()?;
        state.active(&self.instance)?;
        state
            .listeners
            .retain(|listener| !listener.sender.is_closed());
        if state
            .listeners
            .iter()
            .filter(|listener| listener.owner == self.instance)
            .count()
            >= self.options.max_subscriptions_per_module
        {
            return Err(FrameworkError::CapacityExceeded(
                "instance subscriptions".into(),
            ));
        }
        let (sender, receiver) = mpsc::channel(capacity);
        state.listeners.push(Listener {
            owner: self.instance.clone(),
            topic,
            sender,
        });
        Ok(EventSubscription { receiver })
    }

    pub fn emit(
        &self,
        topic: impl Into<String>,
        data: impl Into<String>,
    ) -> Result<DeliveryReport, FrameworkError> {
        let event = Event {
            topic: topic.into(),
            data: data.into(),
        };
        if event.topic.is_empty()
            || event.topic.len().saturating_add(event.data.len()) > self.options.max_event_bytes
        {
            return Err(FrameworkError::CapacityExceeded("event bytes".into()));
        }
        let mut state = self.lock()?;
        state.active(&self.instance)?;
        let mut report = DeliveryReport::default();
        state.listeners.retain(|listener| {
            if listener.topic != event.topic {
                return !listener.sender.is_closed();
            }
            match listener.sender.try_send(event.clone()) {
                Ok(()) => {
                    report.delivered += 1;
                    true
                }
                Err(mpsc::error::TrySendError::Full(_)) => {
                    report.full += 1;
                    true
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    report.closed += 1;
                    false
                }
            }
        });
        Ok(report)
    }
}

struct Entry {
    module: Box<dyn Module>,
    context: ModuleContext,
    metadata: ModuleMetadata,
    stop_finished: bool,
    cleanup: Option<Resources>,
}

pub struct Runtime {
    options: LifecycleOptions,
    state: Arc<Mutex<SharedState>>,
    entries: Vec<Entry>,
    active: Vec<usize>,
    fresh: bool,
    shutdown_issues: Vec<FrameworkError>,
}

impl Runtime {
    pub fn new(options: LifecycleOptions) -> Self {
        Self {
            options,
            state: Arc::new(Mutex::new(SharedState::default())),
            entries: Vec::new(),
            active: Vec::new(),
            fresh: true,
            shutdown_issues: Vec::new(),
        }
    }

    pub fn add(&mut self, module: Box<dyn Module>) -> Result<(), FrameworkError> {
        if !self.fresh {
            return Err(FrameworkError::RuntimeNotFresh);
        }
        if self.entries.len() >= self.options.max_modules {
            return Err(FrameworkError::CapacityExceeded("modules".into()));
        }
        let metadata = module.metadata();
        if metadata.instance.0.is_empty()
            || metadata
                .provides
                .iter()
                .chain(&metadata.requires)
                .any(|key| key.name.is_empty() || key.scope.is_empty() || key.major == 0)
        {
            return Err(FrameworkError::InvalidConfiguration(
                "empty identity or zero service major".into(),
            ));
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| FrameworkError::StatePoisoned)?;
        if state.instances.contains_key(&metadata.instance) {
            return Err(FrameworkError::DuplicateInstance(metadata.instance));
        }
        state.instances.insert(
            metadata.instance.clone(),
            InstanceState {
                metadata: metadata.clone(),
                accepting: false,
                resources: Resources::default(),
            },
        );
        let context = ModuleContext {
            instance: metadata.instance.clone(),
            state: self.state.clone(),
            options: self.options.clone(),
        };
        self.entries.push(Entry {
            module,
            context,
            metadata,
            stop_finished: false,
            cleanup: None,
        });
        Ok(())
    }

    /// Validate the entire graph before executing any module hook.
    pub fn validate(&self) -> Result<Vec<InstanceId>, FrameworkError> {
        self.order().map(|order| {
            order
                .iter()
                .map(|index| self.entries[*index].metadata.instance.clone())
                .collect()
        })
    }

    fn order(&self) -> Result<Vec<usize>, FrameworkError> {
        if self.options.start_timeout.is_zero()
            || self.options.stop_timeout.is_zero()
            || self.options.cleanup_timeout.is_zero()
        {
            return Err(FrameworkError::InvalidConfiguration(
                "lifecycle deadlines must be positive".into(),
            ));
        }
        if [
            self.options.start_timeout,
            self.options.stop_timeout,
            self.options.cleanup_timeout,
        ]
        .iter()
        .any(|duration| std::time::Instant::now().checked_add(*duration).is_none())
        {
            return Err(FrameworkError::InvalidConfiguration(
                "lifecycle deadline is out of range".into(),
            ));
        }
        let mut providers = BTreeMap::new();
        for (index, entry) in self.entries.iter().enumerate() {
            for service in &entry.metadata.provides {
                if providers.insert(service.clone(), index).is_some() {
                    return Err(FrameworkError::ProviderConflict(service.clone()));
                }
            }
        }
        let mut dependencies = Vec::new();
        for entry in &self.entries {
            let mut required = BTreeSet::new();
            for service in &entry.metadata.requires {
                let provider =
                    providers
                        .get(service)
                        .ok_or_else(|| FrameworkError::MissingDependency {
                            instance: entry.metadata.instance.clone(),
                            service: service.clone(),
                        })?;
                required.insert(*provider);
            }
            dependencies.push(required);
        }
        let mut done = BTreeSet::new();
        let mut order = Vec::new();
        while order.len() < self.entries.len() {
            let next = (0..self.entries.len())
                .find(|index| !done.contains(index) && dependencies[*index].is_subset(&done));
            match next {
                Some(index) => {
                    done.insert(index);
                    order.push(index);
                }
                None => {
                    return Err(FrameworkError::DependencyCycle(
                        (0..self.entries.len())
                            .filter(|index| !done.contains(index))
                            .map(|index| self.entries[index].metadata.instance.clone())
                            .collect(),
                    ));
                }
            }
        }
        Ok(order)
    }

    pub async fn start(&mut self) -> Result<(), StartupError> {
        if !self.fresh {
            return Err(StartupError {
                cause: FrameworkError::RuntimeNotFresh,
                cleanup: ShutdownReport::default(),
            });
        }
        let order = self.order().map_err(|cause| StartupError {
            cause,
            cleanup: ShutdownReport::default(),
        })?;
        self.fresh = false;
        for index in order {
            let activate = self
                .state
                .lock()
                .map_err(|_| FrameworkError::StatePoisoned)
                .and_then(|mut state| {
                    let instance = &self.entries[index].metadata.instance;
                    let owner = state
                        .instances
                        .get_mut(instance)
                        .ok_or_else(|| FrameworkError::InstanceInactive(instance.clone()))?;
                    owner.accepting = true;
                    Ok(())
                });
            if let Err(cause) = activate {
                return Err(StartupError {
                    cause,
                    cleanup: self.shutdown().await,
                });
            }
            // Track before awaiting, so an interrupted start can still be shut down.
            self.active.push(index);
            let entry = &mut self.entries[index];
            let result = timeout(
                self.options.start_timeout,
                entry.module.start(&entry.context),
            )
            .await;
            let cause = match result {
                Ok(Ok(())) => self.check_publications(index).err(),
                Ok(Err(error)) => Some(FrameworkError::LifecycleFailure {
                    instance: entry.metadata.instance.clone(),
                    phase: "start",
                    message: error.message,
                }),
                Err(_) => Some(FrameworkError::LifecycleTimeout {
                    instance: entry.metadata.instance.clone(),
                    phase: "start",
                }),
            };
            if let Some(cause) = cause {
                return Err(StartupError {
                    cause,
                    cleanup: self.shutdown().await,
                });
            }
        }
        Ok(())
    }

    fn check_publications(&self, index: usize) -> Result<(), FrameworkError> {
        let state = self
            .state
            .lock()
            .map_err(|_| FrameworkError::StatePoisoned)?;
        let metadata = &self.entries[index].metadata;
        for service in &metadata.provides {
            if !state.services.contains_key(service) {
                return Err(FrameworkError::UnpublishedService {
                    instance: metadata.instance.clone(),
                    service: service.clone(),
                });
            }
        }
        Ok(())
    }

    /// Idempotent shutdown. One module's failure does not skip remaining cleanup.
    pub async fn shutdown(&mut self) -> ShutdownReport {
        self.fresh = false;
        while let Some(&index) = self.active.last() {
            let entry = &mut self.entries[index];
            if !entry.stop_finished {
                match self.state.lock() {
                    Ok(mut state) => {
                        if let Some(owner) = state.instances.get_mut(&entry.metadata.instance) {
                            owner.accepting = false;
                        }
                        state
                            .listeners
                            .retain(|listener| listener.owner != entry.metadata.instance);
                    }
                    Err(_) => self.shutdown_issues.push(FrameworkError::StatePoisoned),
                }
                match timeout(self.options.stop_timeout, entry.module.stop(&entry.context)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => self.shutdown_issues.push(FrameworkError::LifecycleFailure {
                        instance: entry.metadata.instance.clone(),
                        phase: "stop",
                        message: error.message,
                    }),
                    Err(_) => self.shutdown_issues.push(FrameworkError::LifecycleTimeout {
                        instance: entry.metadata.instance.clone(),
                        phase: "stop",
                    }),
                }
                entry.stop_finished = true;
            }
            if entry.cleanup.is_none() {
                match self.state.lock() {
                    Ok(mut state) => {
                        entry.cleanup = Some(state.take_resources(&entry.metadata.instance))
                    }
                    Err(_) => {
                        self.shutdown_issues.push(FrameworkError::StatePoisoned);
                        break;
                    }
                }
            }
            // Keep handles in the runtime across every await. Cancelling shutdown
            // never detaches these tasks or loses their synchronous disposers.
            let Some(resources) = entry.cleanup.as_mut() else {
                break;
            };
            for task in &resources.tasks {
                task.abort();
            }
            let deadline = tokio::time::Instant::now() + self.options.cleanup_timeout;
            let mut timed_out = false;
            while let Some(task) = resources.tasks.last_mut() {
                match tokio::time::timeout_at(deadline, task).await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) if error.is_cancelled() => {}
                    Ok(Err(error)) => self.shutdown_issues.push(FrameworkError::LifecycleFailure {
                        instance: entry.metadata.instance.clone(),
                        phase: "background task",
                        message: error.to_string(),
                    }),
                    Err(_) => {
                        self.shutdown_issues.push(FrameworkError::LifecycleTimeout {
                            instance: entry.metadata.instance.clone(),
                            phase: "cleanup",
                        });
                        timed_out = true;
                        break;
                    }
                }
                resources.tasks.pop();
            }
            if timed_out {
                // Retain this instance and its dependencies until the old task has
                // stopped. A subsequent shutdown can finish the pending cleanup.
                break;
            }
            while let Some(dispose) = resources.disposers.pop() {
                dispose();
            }
            entry.cleanup = None;
            self.active.pop();
        }
        ShutdownReport {
            issues: std::mem::take(&mut self.shutdown_issues),
        }
    }

    pub fn snapshot(&self) -> Result<RuntimeSnapshot, FrameworkError> {
        let state = self
            .state
            .lock()
            .map_err(|_| FrameworkError::StatePoisoned)?;
        Ok(RuntimeSnapshot {
            active_instances: state
                .instances
                .values()
                .filter(|owner| owner.accepting)
                .count(),
            pending_cleanup_instances: self
                .entries
                .iter()
                .filter(|entry| entry.cleanup.is_some())
                .count(),
            services: state.services.len(),
            subscriptions: state.listeners.len(),
            disposers: state
                .instances
                .values()
                .map(|owner| owner.resources.disposers.len())
                .sum::<usize>()
                + self
                    .entries
                    .iter()
                    .filter_map(|entry| entry.cleanup.as_ref())
                    .map(|resources| resources.disposers.len())
                    .sum::<usize>(),
            background_tasks: state
                .instances
                .values()
                .map(|owner| owner.resources.tasks.len())
                .sum::<usize>()
                + self
                    .entries
                    .iter()
                    .filter_map(|entry| entry.cleanup.as_ref())
                    .map(|resources| resources.tasks.len())
                    .sum::<usize>(),
        })
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new(LifecycleOptions::default())
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        // Async stop hooks require explicit shutdown. Drop only revokes work and
        // schedules best-effort reclamation on an existing Tokio runtime.
        let mut resources = if let Ok(mut state) = self.state.lock() {
            self.entries
                .iter()
                .map(|entry| state.take_resources(&entry.metadata.instance))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        resources.extend(
            self.entries
                .iter_mut()
                .filter_map(|entry| entry.cleanup.take()),
        );
        for resource in &resources {
            for task in &resource.tasks {
                task.abort();
            }
        }
        let reclaim = async move {
            for resources in resources.into_iter().rev() {
                for task in resources.tasks {
                    let _ = task.await;
                }
                for dispose in resources.disposers.into_iter().rev() {
                    dispose();
                }
            }
        };
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(reclaim);
        }
    }
}
