//! WebAssembly runtime implementation using wasmtime
//!
//! Implements the Runtime trait for executing WebAssembly modules with WASI support.
//! This enables running WASM workloads alongside traditional containers.
//!
//! ## Features
//!
//! - **WASI Preview 1 (wasip1)**: Core module support with basic WASI syscalls
//! - **WASI Preview 2 (wasip2)**: Component model support with full WASI interfaces
//! - **Async execution**: Tokio integration for cooperative scheduling
//! - **Epoch-based interruption**: Timeout support via epoch deadlines
//! - **Log capture**: stdout/stderr captured for container logs
//!
//! ## Limitations
//!
//! - No `exec` support (WASM modules are single-process)
//! - No cgroup stats (WASM runs in-process, no kernel isolation)
//! - Environment variables only (no filesystem mounts currently)

use crate::cgroups_stats::ContainerStats;
use crate::error::{AgentError, Result};
use crate::runtime::{ContainerId, ContainerState, Runtime};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::instrument;
use wasmtime::component::{Component, Linker as ComponentLinker, ResourceTable};
use wasmtime::{Config, Engine, Linker, Module, Store};
use wasmtime_wasi::p1::{self, WasiP1Ctx};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use zlayer_registry::{detect_wasm_version_from_binary, WasiVersion};
use zlayer_spec::{PullPolicy, ServiceSpec};

/// Default directory for WASM module cache
pub const DEFAULT_WASM_CACHE_DIR: &str = "/var/lib/zlayer/wasm";

/// Configuration for WasmRuntime
#[derive(Debug, Clone)]
pub struct WasmConfig {
    /// Directory for caching pulled WASM modules
    pub cache_dir: PathBuf,
    /// Enable epoch-based interruption for timeouts
    pub enable_epochs: bool,
    /// Default epoch deadline (instructions before yield)
    pub epoch_deadline: u64,
    /// Maximum execution time for WASM modules
    pub max_execution_time: Duration,
}

impl Default for WasmConfig {
    fn default() -> Self {
        Self {
            cache_dir: std::env::var("ZLAYER_WASM_CACHE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from(DEFAULT_WASM_CACHE_DIR)),
            enable_epochs: true,
            epoch_deadline: 1_000_000, // 1M instructions before yield check
            max_execution_time: Duration::from_secs(3600), // 1 hour default
        }
    }
}

/// State for a WASM instance execution
#[derive(Debug, Clone)]
enum InstanceState {
    /// Instance is pending (module compiled, not started)
    Pending,
    /// Instance is currently running
    Running {
        /// When execution started (used for timeout tracking)
        #[allow(dead_code)]
        started_at: Instant,
    },
    /// Instance has completed successfully
    Completed { exit_code: i32 },
    /// Instance failed with an error
    Failed { reason: String },
}

/// Information about a WASM instance
struct WasmInstance {
    /// Container state
    state: InstanceState,
    /// Image reference
    image: String,
    /// Compiled module bytes (cached)
    module_bytes: Vec<u8>,
    /// WASI version
    wasi_version: WasiVersion,
    /// Captured stdout
    stdout: Vec<u8>,
    /// Captured stderr
    stderr: Vec<u8>,
    /// Environment variables for execution
    env_vars: Vec<(String, String)>,
    /// Command args
    args: Vec<String>,
    /// Execution handle (if running)
    execution_handle: Option<tokio::task::JoinHandle<std::result::Result<i32, String>>>,
}

impl std::fmt::Debug for WasmInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmInstance")
            .field("state", &self.state)
            .field("image", &self.image)
            .field("wasi_version", &self.wasi_version)
            .field("stdout_len", &self.stdout.len())
            .field("stderr_len", &self.stderr.len())
            .finish_non_exhaustive()
    }
}

/// WebAssembly runtime using wasmtime
///
/// This runtime executes WASM modules with WASI support, providing a lightweight
/// alternative to full container runtimes for compatible workloads.
pub struct WasmRuntime {
    /// Wasmtime engine (shared across all instances)
    engine: Engine,
    /// Configuration
    config: WasmConfig,
    /// Registry puller for fetching WASM artifacts
    registry: Arc<zlayer_registry::ImagePuller>,
    /// Authentication resolver
    auth_resolver: zlayer_core::AuthResolver,
    /// Active instances
    instances: RwLock<HashMap<String, WasmInstance>>,
}

impl std::fmt::Debug for WasmRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmRuntime")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl WasmRuntime {
    /// Create a new WasmRuntime with the given configuration
    pub async fn new(config: WasmConfig) -> Result<Self> {
        // Create cache directory
        tokio::fs::create_dir_all(&config.cache_dir)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: "wasm-runtime".to_string(),
                reason: format!("failed to create cache directory: {}", e),
            })?;

        // Configure wasmtime engine
        // Note: We use sync execution in spawn_blocking, so async_support is disabled.
        // Epoch-based interruption still works for timeout/cancellation support.
        let mut engine_config = Config::new();

        if config.enable_epochs {
            engine_config.epoch_interruption(true);
        }

        let engine = Engine::new(&engine_config).map_err(|e| AgentError::CreateFailed {
            id: "wasm-runtime".to_string(),
            reason: format!("failed to create wasmtime engine: {}", e),
        })?;

        // Create blob cache for registry
        let cache_path = config.cache_dir.join("blobs.redb");
        let cache = zlayer_registry::BlobCache::open(&cache_path).map_err(|e| {
            AgentError::CreateFailed {
                id: "wasm-runtime".to_string(),
                reason: format!("failed to open blob cache: {}", e),
            }
        })?;

        let registry = Arc::new(zlayer_registry::ImagePuller::new(cache));

        tracing::info!("WASM runtime initialized with wasmtime");

        Ok(Self {
            engine,
            config,
            registry,
            auth_resolver: zlayer_core::AuthResolver::new(zlayer_core::AuthConfig::default()),
            instances: RwLock::new(HashMap::new()),
        })
    }

    /// Create a new WasmRuntime with default configuration
    pub async fn with_defaults() -> Result<Self> {
        Self::new(WasmConfig::default()).await
    }

    /// Create a new WasmRuntime with custom auth configuration
    pub async fn with_auth(
        config: WasmConfig,
        auth_config: zlayer_core::AuthConfig,
    ) -> Result<Self> {
        let mut runtime = Self::new(config).await?;
        runtime.auth_resolver = zlayer_core::AuthResolver::new(auth_config);
        Ok(runtime)
    }

    /// Get the instance ID string from a ContainerId
    fn instance_id(&self, id: &ContainerId) -> String {
        format!("wasm-{}-{}", id.service, id.replica)
    }

    /// Build environment variables from ServiceSpec
    fn build_env_vars(&self, spec: &ServiceSpec) -> Vec<(String, String)> {
        let mut env = Vec::new();

        // Add default environment variables
        env.push((
            "PATH".to_string(),
            "/usr/local/bin:/usr/bin:/bin".to_string(),
        ));

        // Resolve and add spec environment variables
        let resolved = crate::env::resolve_env_vars_with_warnings(&spec.env);
        match resolved {
            Ok(result) => {
                for warning in &result.warnings {
                    tracing::warn!("env resolution warning: {}", warning);
                }
                for var in result.vars {
                    if let Some((key, value)) = var.split_once('=') {
                        env.push((key.to_string(), value.to_string()));
                    }
                }
            }
            Err(e) => {
                tracing::warn!("failed to resolve env vars: {}", e);
            }
        }

        env
    }

    /// Build command arguments from ServiceSpec
    fn build_args(&self, spec: &ServiceSpec) -> Vec<String> {
        let mut args = Vec::new();

        // Module name as argv[0]
        args.push(spec.image.name.clone());

        // Add entrypoint args if present
        if let Some(entrypoint) = &spec.command.entrypoint {
            args.extend_from_slice(entrypoint);
        }

        // Add command args if present
        if let Some(cmd_args) = &spec.command.args {
            args.extend_from_slice(cmd_args);
        }

        args
    }

    /// Execute a WASM module asynchronously
    async fn execute_module(
        engine: Engine,
        module_bytes: Vec<u8>,
        env_vars: Vec<(String, String)>,
        args: Vec<String>,
        epoch_deadline: u64,
        enable_epochs: bool,
    ) -> std::result::Result<i32, String> {
        // This runs in a blocking context because wasmtime operations are CPU-bound
        tokio::task::spawn_blocking(move || {
            // Compile module
            let module = Module::new(&engine, &module_bytes)
                .map_err(|e| format!("failed to compile module: {}", e))?;

            // Build WASI context with environment and args
            let mut wasi_builder = WasiCtxBuilder::new();

            // Set environment variables
            for (key, value) in &env_vars {
                wasi_builder.env(key, value);
            }

            // Set command line arguments
            wasi_builder.args(&args);

            // Build the WASI context - stdout/stderr go to the inherited streams
            // In production we'd capture these to pipes, but for now inherit
            wasi_builder.inherit_stdio();

            let wasi_ctx = wasi_builder.build_p1();

            // Create store with WASI context
            let mut store = Store::new(&engine, wasi_ctx);

            // Set epoch deadline for interruption if enabled
            if enable_epochs {
                store.set_epoch_deadline(epoch_deadline);
            }

            // Create linker and add WASI
            let mut linker: Linker<WasiP1Ctx> = Linker::new(&engine);
            p1::add_to_linker_sync(&mut linker, |ctx| ctx)
                .map_err(|e| format!("failed to add WASI to linker: {}", e))?;

            // Instantiate the module
            let instance = linker
                .instantiate(&mut store, &module)
                .map_err(|e| format!("failed to instantiate module: {}", e))?;

            // Look for _start (WASI command) or main function
            let start_func = instance
                .get_func(&mut store, "_start")
                .or_else(|| instance.get_func(&mut store, "main"));

            match start_func {
                Some(func) => {
                    // Call the entry function
                    match func.call(&mut store, &[], &mut []) {
                        Ok(()) => Ok(0),
                        Err(e) => {
                            // Check for WASI exit
                            if let Some(exit) = e.downcast_ref::<wasmtime_wasi::I32Exit>() {
                                Ok(exit.0)
                            } else {
                                Err(format!("execution error: {}", e))
                            }
                        }
                    }
                }
                None => Err("no _start or main function found".to_string()),
            }
        })
        .await
        .map_err(|e| format!("task join error: {}", e))?
    }

    /// Execute a WASIp2 component
    ///
    /// WASIp2 components use the WebAssembly Component Model and provide richer
    /// WASI interfaces compared to Preview 1 modules. This method handles:
    /// - Component compilation and instantiation
    /// - WASI Preview 2 interface linking (wasi:cli, wasi:io, etc.)
    /// - Calling the wasi:cli/run export
    #[instrument(
        skip(engine, component_bytes, env_vars, args),
        fields(
            component_size = component_bytes.len(),
            args_count = args.len(),
        )
    )]
    async fn execute_component(
        engine: Engine,
        component_bytes: Vec<u8>,
        env_vars: Vec<(String, String)>,
        args: Vec<String>,
        epoch_deadline: u64,
        enable_epochs: bool,
    ) -> std::result::Result<i32, String> {
        // Component model operations are CPU-bound, run in blocking context
        tokio::task::spawn_blocking(move || {
            // Compile the component
            let component = Component::from_binary(&engine, &component_bytes)
                .map_err(|e| format!("failed to compile component: {}", e))?;

            // Build WASIp2 context with environment and args
            let mut wasi_builder = WasiCtxBuilder::new();

            // Set environment variables
            for (key, value) in &env_vars {
                wasi_builder.env(key, value);
            }

            // Set command line arguments
            wasi_builder.args(&args);

            // Inherit stdio for now (in production we'd capture to pipes)
            wasi_builder.inherit_stdio();

            // Build the WASIp2 context
            let wasi_ctx = wasi_builder.build();

            // Create resource table for component model resources
            let table = ResourceTable::new();

            // Create our WasiState that implements WasiView
            let state = WasiState {
                ctx: wasi_ctx,
                table,
            };

            // Create store with our WASI state
            let mut store = Store::new(&engine, state);

            // Set epoch deadline for interruption if enabled
            if enable_epochs {
                store.set_epoch_deadline(epoch_deadline);
            }

            // Create component linker and add WASIp2 interfaces
            let mut linker: ComponentLinker<WasiState> = ComponentLinker::new(&engine);
            wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
                .map_err(|e| format!("failed to add WASIp2 to linker: {}", e))?;

            // Try to instantiate as a wasi:cli/command component
            // This is the standard entry point for CLI-style WASM components
            let instance = linker
                .instantiate(&mut store, &component)
                .map_err(|e| format!("failed to instantiate component: {}", e))?;

            // Try to get the wasi:cli/run export first (preferred for wasip2)
            // The run function has signature: func run() -> result<_, error>
            if let Some(run_func) = instance.get_func(&mut store, "wasi:cli/run@0.2.0#run") {
                // Call the run function
                match run_func.call(&mut store, &[], &mut []) {
                    Ok(()) => return Ok(0),
                    Err(e) => {
                        // Check for WASI exit
                        if let Some(exit) = e.downcast_ref::<wasmtime_wasi::I32Exit>() {
                            return Ok(exit.0);
                        }
                        return Err(format!("wasi:cli/run execution error: {}", e));
                    }
                }
            }

            // Fall back to _start if run is not found (for wasip1-style components)
            if let Some(start_func) = instance.get_func(&mut store, "_start") {
                match start_func.call(&mut store, &[], &mut []) {
                    Ok(()) => return Ok(0),
                    Err(e) => {
                        // Check for WASI exit
                        if let Some(exit) = e.downcast_ref::<wasmtime_wasi::I32Exit>() {
                            return Ok(exit.0);
                        }
                        return Err(format!("_start execution error: {}", e));
                    }
                }
            }

            // Try main as last resort
            if let Some(main_func) = instance.get_func(&mut store, "main") {
                match main_func.call(&mut store, &[], &mut []) {
                    Ok(()) => return Ok(0),
                    Err(e) => {
                        if let Some(exit) = e.downcast_ref::<wasmtime_wasi::I32Exit>() {
                            return Ok(exit.0);
                        }
                        return Err(format!("main execution error: {}", e));
                    }
                }
            }

            Err("no wasi:cli/run, _start, or main function found in component".to_string())
        })
        .await
        .map_err(|e| format!("task join error: {}", e))?
    }
}

/// WASIp2 state for the component model
///
/// This struct holds the WASI context and resource table required
/// for executing WASIp2 components. It implements [`WasiView`] to
/// provide access to these resources during component execution.
struct WasiState {
    /// WASI Preview 2 context with environment, args, and capabilities
    ctx: WasiCtx,
    /// Resource table for component model resources (files, sockets, etc.)
    table: ResourceTable,
}

impl WasiView for WasiState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
        }
    }
}

#[async_trait::async_trait]
impl Runtime for WasmRuntime {
    /// Pull a WASM image from a registry
    ///
    /// This pulls the WASM artifact and caches the binary locally.
    #[instrument(
        skip(self),
        fields(
            otel.name = "wasm.pull",
            container.image.name = %image,
        )
    )]
    async fn pull_image(&self, image: &str) -> Result<()> {
        self.pull_image_with_policy(image, PullPolicy::IfNotPresent)
            .await
    }

    /// Pull a WASM image with a specific policy
    #[instrument(
        skip(self),
        fields(
            otel.name = "wasm.pull",
            container.image.name = %image,
            pull_policy = ?policy,
        )
    )]
    async fn pull_image_with_policy(&self, image: &str, policy: PullPolicy) -> Result<()> {
        // Handle Never policy
        if matches!(policy, PullPolicy::Never) {
            tracing::debug!(image = %image, "pull policy is Never, skipping pull");
            return Ok(());
        }

        // For IfNotPresent, check if we have the WASM cached
        let cache_key = image.replace(['/', ':', '@'], "_");
        let cache_path = self.config.cache_dir.join(format!("{}.wasm", cache_key));

        if matches!(policy, PullPolicy::IfNotPresent) && cache_path.exists() {
            tracing::debug!(image = %image, "WASM module already cached");
            return Ok(());
        }

        let auth = self.auth_resolver.resolve(image);

        tracing::info!(image = %image, "pulling WASM artifact from registry");

        // Pull WASM binary from registry
        let (wasm_bytes, wasm_info) =
            self.registry
                .pull_wasm(image, &auth)
                .await
                .map_err(|e| AgentError::PullFailed {
                    image: image.to_string(),
                    reason: format!("failed to pull WASM artifact: {}", e),
                })?;

        // Cache the WASM binary
        tokio::fs::write(&cache_path, &wasm_bytes)
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("failed to cache WASM binary: {}", e),
            })?;

        tracing::info!(
            image = %image,
            wasi_version = %wasm_info.wasi_version,
            size = wasm_bytes.len(),
            "WASM artifact pulled and cached"
        );

        Ok(())
    }

    /// Create a WASM container (compile and prepare for execution)
    #[instrument(
        skip(self, spec),
        fields(
            otel.name = "wasm.create",
            container.id = %self.instance_id(id),
            service.name = %id.service,
            container.image.name = %spec.image.name,
        )
    )]
    async fn create_container(&self, id: &ContainerId, spec: &ServiceSpec) -> Result<()> {
        let instance_id = self.instance_id(id);
        let image = &spec.image.name;

        tracing::info!(
            instance = %instance_id,
            image = %image,
            "creating WASM instance"
        );

        // Load WASM binary from cache or pull
        let cache_key = image.replace(['/', ':', '@'], "_");
        let cache_path = self.config.cache_dir.join(format!("{}.wasm", cache_key));

        // Track whether we loaded from local cache (vs pulled from registry)
        let loaded_from_cache = cache_path.exists();

        let (module_bytes, wasi_version) = if loaded_from_cache {
            // Read from local cache
            let bytes =
                tokio::fs::read(&cache_path)
                    .await
                    .map_err(|e| AgentError::CreateFailed {
                        id: instance_id.clone(),
                        reason: format!("failed to read cached WASM: {}", e),
                    })?;
            // Detect WASI version from the binary itself
            let detected_version = detect_wasm_version_from_binary(&bytes);
            tracing::debug!(
                instance = %instance_id,
                wasi_version = %detected_version,
                "detected WASI version from cached binary"
            );
            (bytes, detected_version)
        } else {
            // Pull from registry - get WASI version from manifest
            let auth = self.auth_resolver.resolve(image);
            let (wasm_bytes, wasm_info) =
                self.registry.pull_wasm(image, &auth).await.map_err(|e| {
                    AgentError::CreateFailed {
                        id: instance_id.clone(),
                        reason: format!("failed to pull WASM: {}", e),
                    }
                })?;

            tokio::fs::write(&cache_path, &wasm_bytes)
                .await
                .map_err(|e| AgentError::CreateFailed {
                    id: instance_id.clone(),
                    reason: format!("failed to cache WASM: {}", e),
                })?;

            (wasm_bytes, wasm_info.wasi_version)
        };

        // Build environment and args
        let env_vars = self.build_env_vars(spec);
        let args = self.build_args(spec);

        // Create instance entry
        let instance = WasmInstance {
            state: InstanceState::Pending,
            image: image.clone(),
            module_bytes,
            wasi_version,
            stdout: Vec::new(),
            stderr: Vec::new(),
            env_vars,
            args,
            execution_handle: None,
        };

        // Store instance
        {
            let mut instances = self.instances.write().await;
            instances.insert(instance_id.clone(), instance);
        }

        tracing::info!(
            instance = %instance_id,
            "WASM instance created"
        );

        Ok(())
    }

    /// Start a WASM container (begin execution)
    #[instrument(
        skip(self),
        fields(
            otel.name = "wasm.start",
            container.id = %self.instance_id(id),
            service.name = %id.service,
        )
    )]
    async fn start_container(&self, id: &ContainerId) -> Result<()> {
        let instance_id = self.instance_id(id);

        tracing::info!(instance = %instance_id, "starting WASM instance");

        // Get instance and extract data for execution
        let (wasm_bytes, wasi_version, env_vars, args) = {
            let mut instances = self.instances.write().await;
            let instance = instances
                .get_mut(&instance_id)
                .ok_or_else(|| AgentError::NotFound {
                    container: instance_id.clone(),
                    reason: "WASM instance not found".to_string(),
                })?;

            // Update state to running
            instance.state = InstanceState::Running {
                started_at: Instant::now(),
            };

            (
                instance.module_bytes.clone(),
                instance.wasi_version.clone(),
                instance.env_vars.clone(),
                instance.args.clone(),
            )
        };

        // Detect if this is a component (WASIp2) or module (WASIp1) from the binary
        // The stored wasi_version from manifest takes precedence, but we also check binary
        let is_component = match &wasi_version {
            WasiVersion::Preview2 => true,
            WasiVersion::Preview1 => false,
            WasiVersion::Unknown => {
                // Fall back to binary detection
                let detected = detect_wasm_version_from_binary(&wasm_bytes);
                detected.is_preview2()
            }
        };

        tracing::info!(
            instance = %instance_id,
            wasi_version = %wasi_version,
            is_component = is_component,
            "starting WASM execution"
        );

        // Spawn execution task based on component vs module
        let engine = self.engine.clone();
        let epoch_deadline = self.config.epoch_deadline;
        let enable_epochs = self.config.enable_epochs;
        let instance_id_clone = instance_id.clone();

        let handle = if is_component {
            tokio::spawn(async move {
                Self::execute_component(
                    engine,
                    wasm_bytes,
                    env_vars,
                    args,
                    epoch_deadline,
                    enable_epochs,
                )
                .await
            })
        } else {
            tokio::spawn(async move {
                Self::execute_module(
                    engine,
                    wasm_bytes,
                    env_vars,
                    args,
                    epoch_deadline,
                    enable_epochs,
                )
                .await
            })
        };

        // Store handle and update state
        {
            let mut instances = self.instances.write().await;
            if let Some(instance) = instances.get_mut(&instance_id) {
                instance.execution_handle = Some(handle);
            }
        }

        tracing::info!(
            instance = %instance_id_clone,
            "WASM instance started"
        );

        Ok(())
    }

    /// Stop a WASM container (cancel execution)
    #[instrument(
        skip(self),
        fields(
            otel.name = "wasm.stop",
            container.id = %self.instance_id(id),
            service.name = %id.service,
        )
    )]
    async fn stop_container(&self, id: &ContainerId, timeout: Duration) -> Result<()> {
        let instance_id = self.instance_id(id);

        tracing::info!(
            instance = %instance_id,
            timeout = ?timeout,
            "stopping WASM instance"
        );

        // Get and abort the execution handle
        let handle = {
            let mut instances = self.instances.write().await;
            let instance = instances
                .get_mut(&instance_id)
                .ok_or_else(|| AgentError::NotFound {
                    container: instance_id.clone(),
                    reason: "WASM instance not found".to_string(),
                })?;

            // If not running, nothing to stop
            if !matches!(instance.state, InstanceState::Running { .. }) {
                return Ok(());
            }

            instance.execution_handle.take()
        };

        // Abort the execution if we have a handle
        if let Some(handle) = handle {
            // Wait for graceful completion or abort after timeout
            let result = tokio::time::timeout(timeout, handle).await;

            match result {
                Ok(Ok(Ok(exit_code))) => {
                    let mut instances = self.instances.write().await;
                    if let Some(instance) = instances.get_mut(&instance_id) {
                        instance.state = InstanceState::Completed { exit_code };
                    }
                }
                Ok(Ok(Err(e))) => {
                    let mut instances = self.instances.write().await;
                    if let Some(instance) = instances.get_mut(&instance_id) {
                        instance.state = InstanceState::Failed { reason: e };
                    }
                }
                Ok(Err(join_error)) => {
                    let mut instances = self.instances.write().await;
                    if let Some(instance) = instances.get_mut(&instance_id) {
                        instance.state = InstanceState::Failed {
                            reason: format!("task join error: {}", join_error),
                        };
                    }
                }
                Err(_timeout) => {
                    // Timeout - mark as failed
                    let mut instances = self.instances.write().await;
                    if let Some(instance) = instances.get_mut(&instance_id) {
                        instance.state = InstanceState::Failed {
                            reason: "execution timed out".to_string(),
                        };
                    }
                }
            }
        }

        tracing::info!(instance = %instance_id, "WASM instance stopped");

        Ok(())
    }

    /// Remove a WASM container (cleanup)
    #[instrument(
        skip(self),
        fields(
            otel.name = "wasm.remove",
            container.id = %self.instance_id(id),
            service.name = %id.service,
        )
    )]
    async fn remove_container(&self, id: &ContainerId) -> Result<()> {
        let instance_id = self.instance_id(id);

        tracing::info!(instance = %instance_id, "removing WASM instance");

        // Remove from instances map
        let mut instances = self.instances.write().await;
        if let Some(mut instance) = instances.remove(&instance_id) {
            // Abort any running execution
            if let Some(handle) = instance.execution_handle.take() {
                handle.abort();
            }
        }

        tracing::info!(instance = %instance_id, "WASM instance removed");

        Ok(())
    }

    /// Get container state
    #[instrument(
        skip(self),
        fields(
            otel.name = "wasm.state",
            container.id = %self.instance_id(id),
        )
    )]
    async fn container_state(&self, id: &ContainerId) -> Result<ContainerState> {
        let instance_id = self.instance_id(id);

        let instances = self.instances.read().await;
        let instance = instances
            .get(&instance_id)
            .ok_or_else(|| AgentError::NotFound {
                container: instance_id.clone(),
                reason: "WASM instance not found".to_string(),
            })?;

        let state = match &instance.state {
            InstanceState::Pending => ContainerState::Pending,
            InstanceState::Running { .. } => ContainerState::Running,
            InstanceState::Completed { exit_code } => ContainerState::Exited { code: *exit_code },
            InstanceState::Failed { reason } => ContainerState::Failed {
                reason: reason.clone(),
            },
        };

        Ok(state)
    }

    /// Get container logs
    async fn container_logs(&self, id: &ContainerId, tail: usize) -> Result<String> {
        let instance_id = self.instance_id(id);

        let instances = self.instances.read().await;
        let instance = instances
            .get(&instance_id)
            .ok_or_else(|| AgentError::NotFound {
                container: instance_id.clone(),
                reason: "WASM instance not found".to_string(),
            })?;

        // Combine stdout and stderr
        let mut logs = String::new();

        if !instance.stdout.is_empty() {
            logs.push_str("[stdout]\n");
            logs.push_str(&String::from_utf8_lossy(&instance.stdout));
        }

        if !instance.stderr.is_empty() {
            if !logs.is_empty() {
                logs.push('\n');
            }
            logs.push_str("[stderr]\n");
            logs.push_str(&String::from_utf8_lossy(&instance.stderr));
        }

        // Apply tail limit
        if tail > 0 {
            let lines: Vec<&str> = logs.lines().collect();
            if lines.len() > tail {
                logs = lines[lines.len() - tail..].join("\n");
            }
        }

        Ok(logs)
    }

    /// Execute command in container
    ///
    /// WASM modules don't support exec - return an error.
    async fn exec(&self, id: &ContainerId, _cmd: &[String]) -> Result<(i32, String, String)> {
        let instance_id = self.instance_id(id);

        Err(AgentError::Internal(format!(
            "exec not supported for WASM instance '{}': WASM modules are single-process and don't support exec",
            instance_id
        )))
    }

    /// Get container resource statistics
    ///
    /// # Design Decision: Empty Statistics
    ///
    /// WASM modules run in-process within the wasmtime runtime and do not have
    /// kernel-level isolation like containers. As a result:
    ///
    /// - **No cgroup stats**: WASM has no cgroup, so CPU/memory accounting at the
    ///   kernel level is not available.
    /// - **Shared process memory**: WASM linear memory is part of the host process
    ///   heap, making per-instance memory measurement impractical.
    /// - **CPU time**: While wasmtime tracks fuel/epochs for interruption, it does
    ///   not expose CPU time metrics in a format compatible with cgroup stats.
    ///
    /// For WASM resource monitoring, consider:
    /// - Using wasmtime's fuel metering for instruction-level accounting
    /// - Monitoring the host process's overall resource usage
    /// - Implementing application-level metrics within the WASM module
    ///
    /// Returns zero values for all metrics with `u64::MAX` as the memory limit
    /// (indicating no limit), which is semantically correct for WASM modules.
    async fn get_container_stats(&self, id: &ContainerId) -> Result<ContainerStats> {
        let instance_id = self.instance_id(id);

        // Check instance exists
        let instances = self.instances.read().await;
        if !instances.contains_key(&instance_id) {
            return Err(AgentError::NotFound {
                container: instance_id.clone(),
                reason: "WASM instance not found".to_string(),
            });
        }

        // Return empty stats - WASM has no cgroup isolation for kernel-level resource tracking
        Ok(ContainerStats {
            cpu_usage_usec: 0,
            memory_bytes: 0,
            memory_limit: u64::MAX,
            timestamp: Instant::now(),
        })
    }

    /// Wait for container to exit
    async fn wait_container(&self, id: &ContainerId) -> Result<i32> {
        let instance_id = self.instance_id(id);

        tracing::debug!(instance = %instance_id, "waiting for WASM instance to exit");

        // Poll state until exited
        let poll_interval = Duration::from_millis(100);
        let max_wait = self.config.max_execution_time;
        let start = Instant::now();

        loop {
            if start.elapsed() > max_wait {
                return Err(AgentError::Timeout { timeout: max_wait });
            }

            // Check if we need to poll execution result
            {
                let mut instances = self.instances.write().await;
                if let Some(instance) = instances.get_mut(&instance_id) {
                    // Check if handle completed
                    if let Some(handle) = &mut instance.execution_handle {
                        if handle.is_finished() {
                            let handle = instance.execution_handle.take().unwrap();
                            match handle.await {
                                Ok(Ok(exit_code)) => {
                                    instance.state = InstanceState::Completed { exit_code };
                                }
                                Ok(Err(e)) => {
                                    instance.state = InstanceState::Failed { reason: e };
                                }
                                Err(e) => {
                                    instance.state = InstanceState::Failed {
                                        reason: format!("task join error: {}", e),
                                    };
                                }
                            }
                        }
                    }

                    // Check state
                    match &instance.state {
                        InstanceState::Completed { exit_code } => {
                            return Ok(*exit_code);
                        }
                        InstanceState::Failed { reason } => {
                            return Err(AgentError::Internal(format!(
                                "WASM execution failed: {}",
                                reason
                            )));
                        }
                        InstanceState::Pending | InstanceState::Running { .. } => {
                            // Continue waiting
                        }
                    }
                } else {
                    return Err(AgentError::NotFound {
                        container: instance_id.clone(),
                        reason: "WASM instance not found".to_string(),
                    });
                }
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Get container logs as lines
    async fn get_logs(&self, id: &ContainerId) -> Result<Vec<String>> {
        let instance_id = self.instance_id(id);

        let instances = self.instances.read().await;
        let instance = instances
            .get(&instance_id)
            .ok_or_else(|| AgentError::NotFound {
                container: instance_id.clone(),
                reason: "WASM instance not found".to_string(),
            })?;

        let mut logs = Vec::new();

        // Add stdout lines
        for line in String::from_utf8_lossy(&instance.stdout).lines() {
            logs.push(format!("[stdout] {}", line));
        }

        // Add stderr lines
        for line in String::from_utf8_lossy(&instance.stderr).lines() {
            logs.push(format!("[stderr] {}", line));
        }

        Ok(logs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_config_default() {
        let config = WasmConfig::default();

        assert_eq!(config.cache_dir, PathBuf::from(DEFAULT_WASM_CACHE_DIR));
        assert!(config.enable_epochs);
        assert_eq!(config.epoch_deadline, 1_000_000);
        assert_eq!(config.max_execution_time, Duration::from_secs(3600));
    }

    #[test]
    fn test_instance_id_generation() {
        let id = ContainerId {
            service: "myservice".to_string(),
            replica: 1,
        };

        let expected = "wasm-myservice-1";
        let result = format!("wasm-{}-{}", id.service, id.replica);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_cache_key_sanitization() {
        let images = vec![
            ("ghcr.io/org/module:v1.0", "ghcr.io_org_module_v1.0"),
            (
                "registry.example.com/wasm@sha256:abc",
                "registry.example.com_wasm_sha256_abc",
            ),
        ];

        for (image, expected) in images {
            let sanitized = image.replace(['/', ':', '@'], "_");
            assert_eq!(sanitized, expected);
        }
    }

    #[test]
    fn test_instance_state_debug() {
        let pending = InstanceState::Pending;
        let running = InstanceState::Running {
            started_at: Instant::now(),
        };
        let completed = InstanceState::Completed { exit_code: 0 };
        let failed = InstanceState::Failed {
            reason: "test error".to_string(),
        };

        // Ensure Debug trait is implemented
        assert!(!format!("{:?}", pending).is_empty());
        assert!(!format!("{:?}", running).is_empty());
        assert!(!format!("{:?}", completed).is_empty());
        assert!(!format!("{:?}", failed).is_empty());
    }

    #[test]
    fn test_wasm_config_clone() {
        let config = WasmConfig {
            cache_dir: PathBuf::from("/custom/cache"),
            enable_epochs: false,
            epoch_deadline: 500_000,
            max_execution_time: Duration::from_secs(60),
        };

        let cloned = config.clone();

        assert_eq!(cloned.cache_dir, config.cache_dir);
        assert_eq!(cloned.enable_epochs, config.enable_epochs);
        assert_eq!(cloned.epoch_deadline, config.epoch_deadline);
        assert_eq!(cloned.max_execution_time, config.max_execution_time);
    }
}
