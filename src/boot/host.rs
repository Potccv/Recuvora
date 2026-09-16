//! Shared runtime composition for real CLI and HTTP capabilities.
pub use super::configuration::{
    extension_protected_paths, load_harness_config, load_repair_config,
};
use crate::framework::{
    InstanceId, LifecycleFuture, LifecycleOptions, Module, ModuleContext, ModuleError,
    ModuleMetadata, Runtime, RuntimeSnapshot, ServiceKey,
};
use crate::harnesses::{
    HarnessAdapterFactory, HarnessError, HarnessRegistry, HarnessRegistryBuilder,
    HarnessRegistryConfig,
};
use crate::monitors::{MonitorEngine, MonitorHandle, MonitorsConfig};
use crate::nodes::{ExtensionRegistry, ExtensionsConfig};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use thiserror::Error;

#[derive(Default)]
pub struct HostConfig {
    pub harnesses: Option<HarnessRegistryConfig>,
    pub extensions: Option<ExtensionsConfig>,
    pub monitoring: Option<MonitoringHostConfig>,
}

pub struct MonitoringHostConfig {
    pub config: MonitorsConfig,
    pub data_dir: PathBuf,
}

#[derive(Debug, Error)]
pub enum HostError {
    #[error(transparent)]
    Harness(#[from] HarnessError),
    #[error("host lifecycle: {0}")]
    Lifecycle(String),
}

#[derive(Clone, Default)]
struct Services {
    harnesses: Option<Arc<HarnessRegistry>>,
    extensions: Option<Arc<ExtensionRegistry>>,
    monitoring: Option<Arc<MonitorHandle>>,
}

/// Owns one real host startup/shutdown cycle. Callers must drain their business
/// operations before shutdown and keep their durable result handling active.
pub struct HostRuntime {
    runtime: Option<Runtime>,
    services: Services,
}

impl HostRuntime {
    pub async fn start(config: HostConfig) -> Result<Self, HostError> {
        let has_monitoring = config.monitoring.is_some();
        let extensions_config = config.extensions.or_else(|| {
            has_monitoring.then(|| ExtensionsConfig {
                schema_version: 1,
                extensions: Vec::new(),
            })
        });
        let has_extensions = extensions_config.is_some();
        let has_harnesses = config.harnesses.is_some();
        let mut runtime = Runtime::new(LifecycleOptions {
            // Each configured extension has a bounded 10s handshake and drain.
            start_timeout: Duration::from_secs(30 * 64),
            stop_timeout: Duration::from_secs(30),
            ..LifecycleOptions::default()
        });
        let output = Arc::new(OnceLock::new());
        let harness_failure = Arc::new(Mutex::new(None));
        // Add consumers first so only declared dependencies control the order.
        runtime
            .add(Box::new(BindingsModule {
                output: output.clone(),
                has_harnesses,
                has_extensions,
                has_monitoring,
            }))
            .map_err(lifecycle)?;
        if let Some(harnesses) = config.harnesses {
            runtime
                .add(Box::new(HarnessModule {
                    config: Some(harnesses),
                    has_extensions,
                    registry: None,
                    failure: harness_failure.clone(),
                }))
                .map_err(lifecycle)?;
        }
        if let Some(monitoring) = config.monitoring {
            runtime
                .add(Box::new(MonitoringModule {
                    config: Some(monitoring),
                    engine: None,
                }))
                .map_err(lifecycle)?;
        }
        if let Some(extensions) = extensions_config {
            runtime
                .add(Box::new(ExtensionsModule {
                    config: Some(extensions),
                    registry: None,
                }))
                .map_err(lifecycle)?;
        }
        if let Err(error) = runtime.start().await {
            // Preserve stable adapter/configuration error categories when
            // startup cleanup succeeded. Cleanup errors take precedence.
            if error.cleanup.is_clean()
                && let Some(harness_error) = harness_failure
                    .lock()
                    .map_err(|_| lifecycle("startup error lock poisoned"))?
                    .take()
            {
                return Err(HostError::Harness(harness_error));
            }
            return Err(lifecycle(error));
        }
        let services = output
            .get()
            .cloned()
            .ok_or_else(|| lifecycle("host services were not bound"))?;
        Ok(Self {
            runtime: Some(runtime),
            services,
        })
    }

    pub fn harnesses(&self) -> Option<Arc<HarnessRegistry>> {
        self.services.harnesses.clone()
    }

    pub fn extensions(&self) -> Option<Arc<ExtensionRegistry>> {
        self.services.extensions.clone()
    }

    pub fn monitoring(&self) -> Option<Arc<MonitorHandle>> {
        self.services.monitoring.clone()
    }

    pub fn snapshot(&self) -> Result<RuntimeSnapshot, HostError> {
        self.runtime
            .as_ref()
            .map_or(Ok(RuntimeSnapshot::default()), |runtime| {
                runtime.snapshot().map_err(lifecycle)
            })
    }

    pub async fn shutdown(&mut self) -> Result<(), HostError> {
        close(&self.services)?;
        // Keep the runtime and dependencies owned if a drain deadline expires.
        // A later shutdown resumes waiting; a timeout is never a clean result.
        tokio::time::timeout(Duration::from_secs(30), drain(&self.services))
            .await
            .map_err(|_| lifecycle("service drain timed out; cleanup remains pending"))??;
        if let Some(runtime) = &mut self.runtime {
            let report = runtime.shutdown().await;
            if !report.is_clean() {
                return Err(lifecycle(format!("shutdown reported: {:?}", report.issues)));
            }
            if runtime.snapshot().map_err(lifecycle)? != RuntimeSnapshot::default() {
                return Err(lifecycle("shutdown left runtime resources"));
            }
        }
        self.runtime = None;
        Ok(())
    }
}

impl Drop for HostRuntime {
    fn drop(&mut self) {
        let Some(mut runtime) = self.runtime.take() else {
            return;
        };
        let services = self.services.clone();
        let _ = close(&services);
        // A copied Arc must not remain dispatchable merely because the owner
        // was dropped. Explicit shutdown is still required to inspect errors.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = drain(&services).await;
                let _ = runtime.shutdown().await;
            });
        }
    }
}

fn close(services: &Services) -> Result<(), HostError> {
    // Revoke all service entry points before waiting on any provider.
    let monitoring = services
        .monitoring
        .as_ref()
        .map(|handle| handle.begin_shutdown())
        .transpose();
    let harnesses = services
        .harnesses
        .as_ref()
        .map(|registry| registry.begin_shutdown())
        .transpose();
    let extensions = services
        .extensions
        .as_ref()
        .map(|registry| registry.begin_shutdown())
        .transpose();
    monitoring.map_err(lifecycle)?;
    harnesses?;
    extensions.map_err(lifecycle)?;
    Ok(())
}

async fn drain(services: &Services) -> Result<(), HostError> {
    if let Some(monitoring) = &services.monitoring {
        monitoring.drain().await.map_err(lifecycle)?;
    }
    if let Some(registry) = &services.harnesses {
        registry.shutdown().await?;
    }
    if let Some(registry) = &services.extensions {
        registry.shutdown().await.map_err(lifecycle)?;
    }
    Ok(())
}

fn harness_service() -> ServiceKey {
    ServiceKey::new("harness.registry", 1, "host")
}
fn extensions_service() -> ServiceKey {
    ServiceKey::new("extensions.registry", 1, "host")
}

fn monitoring_service() -> ServiceKey {
    ServiceKey::new("monitoring.service", 1, "host")
}

struct MonitoringModule {
    config: Option<MonitoringHostConfig>,
    engine: Option<MonitorEngine>,
}

impl Module for MonitoringModule {
    fn metadata(&self) -> ModuleMetadata {
        ModuleMetadata::new(InstanceId::new("host-monitoring"))
            .requires(extensions_service())
            .provides(monitoring_service())
    }

    fn start<'a>(&'a mut self, context: &'a ModuleContext) -> LifecycleFuture<'a> {
        Box::pin(async move {
            let config = self
                .config
                .take()
                .ok_or_else(|| ModuleError::new("monitoring already initialized"))?;
            let engine = MonitorEngine::start(
                config.config,
                context.service(&extensions_service())?,
                config.data_dir,
            )
            .map_err(module_error)?;
            let handle = Arc::new(engine.handle());
            self.engine = Some(engine);
            context.publish(monitoring_service(), handle)?;
            Ok(())
        })
    }

    fn stop<'a>(&'a mut self, _context: &'a ModuleContext) -> LifecycleFuture<'a> {
        Box::pin(async move {
            if let Some(engine) = &mut self.engine {
                engine.shutdown().await.map_err(module_error)?;
            }
            Ok(())
        })
    }
}

struct ExtensionsModule {
    config: Option<ExtensionsConfig>,
    registry: Option<Arc<ExtensionRegistry>>,
}

impl Module for ExtensionsModule {
    fn metadata(&self) -> ModuleMetadata {
        ModuleMetadata::new(InstanceId::new("host-extensions")).provides(extensions_service())
    }
    fn start<'a>(&'a mut self, context: &'a ModuleContext) -> LifecycleFuture<'a> {
        Box::pin(async move {
            let config = self
                .config
                .take()
                .ok_or_else(|| ModuleError::new("extensions already initialized"))?;
            let registry = Arc::new(
                ExtensionRegistry::connect(config)
                    .await
                    .map_err(module_error)?,
            );
            self.registry = Some(registry.clone());
            context.publish(extensions_service(), registry)?;
            Ok(())
        })
    }
    fn stop<'a>(&'a mut self, _context: &'a ModuleContext) -> LifecycleFuture<'a> {
        Box::pin(async move {
            if let Some(registry) = &self.registry {
                registry.shutdown().await.map_err(module_error)?;
            }
            Ok(())
        })
    }
}

struct HarnessModule {
    config: Option<HarnessRegistryConfig>,
    has_extensions: bool,
    registry: Option<Arc<HarnessRegistry>>,
    failure: Arc<Mutex<Option<HarnessError>>>,
}

impl Module for HarnessModule {
    fn metadata(&self) -> ModuleMetadata {
        let metadata =
            ModuleMetadata::new(InstanceId::new("host-harnesses")).provides(harness_service());
        if self.has_extensions {
            metadata.requires(extensions_service())
        } else {
            metadata
        }
    }
    fn start<'a>(&'a mut self, context: &'a ModuleContext) -> LifecycleFuture<'a> {
        Box::pin(async move {
            let extensions = if self.has_extensions {
                Some(context.service(&extensions_service())?)
            } else {
                None
            };
            let config = self
                .config
                .take()
                .ok_or_else(|| ModuleError::new("Harnesses already initialized"))?;
            let registry = match build_harnesses(config, extensions) {
                Ok(registry) => Arc::new(registry),
                Err(error) => {
                    let message = error.to_string();
                    *self
                        .failure
                        .lock()
                        .map_err(|_| ModuleError::new("startup error lock poisoned"))? =
                        Some(error);
                    return Err(ModuleError::new(message));
                }
            };
            self.registry = Some(registry.clone());
            context.publish(harness_service(), registry)?;
            Ok(())
        })
    }
    fn stop<'a>(&'a mut self, _context: &'a ModuleContext) -> LifecycleFuture<'a> {
        Box::pin(async move {
            if let Some(registry) = &self.registry {
                registry.shutdown().await.map_err(module_error)?;
            }
            Ok(())
        })
    }
}

struct BindingsModule {
    output: Arc<OnceLock<Services>>,
    has_harnesses: bool,
    has_extensions: bool,
    has_monitoring: bool,
}

impl Module for BindingsModule {
    fn metadata(&self) -> ModuleMetadata {
        let mut metadata = ModuleMetadata::new(InstanceId::new("host-bindings"));
        if self.has_harnesses {
            metadata = metadata.requires(harness_service());
        }
        if self.has_extensions {
            metadata = metadata.requires(extensions_service());
        }
        if self.has_monitoring {
            metadata = metadata.requires(monitoring_service());
        }
        metadata
    }
    fn start<'a>(&'a mut self, context: &'a ModuleContext) -> LifecycleFuture<'a> {
        Box::pin(async move {
            self.output
                .set(Services {
                    harnesses: if self.has_harnesses {
                        Some(context.service(&harness_service())?)
                    } else {
                        None
                    },
                    extensions: if self.has_extensions {
                        Some(context.service(&extensions_service())?)
                    } else {
                        None
                    },
                    monitoring: if self.has_monitoring {
                        Some(context.service(&monitoring_service())?)
                    } else {
                        None
                    },
                })
                .map_err(|_| ModuleError::new("host bindings already initialized"))?;
            Ok(())
        })
    }
}

pub(super) fn build_harnesses(
    config: HarnessRegistryConfig,
    extensions: Option<Arc<ExtensionRegistry>>,
) -> Result<HarnessRegistry, HarnessError> {
    let mut builder = HarnessRegistryBuilder::new();
    for factory in factories(extensions) {
        builder.register(factory)?;
    }
    builder.build(config)
}

fn factories(extensions: Option<Arc<ExtensionRegistry>>) -> Vec<Arc<dyn HarnessAdapterFactory>> {
    let mut factories: Vec<Arc<dyn HarnessAdapterFactory>> = Vec::new();
    if let Some(extensions) = extensions {
        factories.push(Arc::new(crate::harnesses::RemoteHarnessFactory::new(
            extensions,
        )));
    }
    factories
}

fn module_error(error: impl std::fmt::Display) -> ModuleError {
    ModuleError::new(error.to_string())
}
fn lifecycle(error: impl std::fmt::Display) -> HostError {
    HostError::Lifecycle(error.to_string())
}
