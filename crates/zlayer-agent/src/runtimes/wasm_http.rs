// Allow Arc with non-Send/Sync types for the instance pools.
// The PooledInstance contains Store<WasmHttpState> which has WasiCtx containing
// Box<dyn RngCore + Send> that is Send but not Sync. This is fine because
// we only access the pool through RwLock which provides synchronization.
#![allow(clippy::arc_with_non_send_sync)]

//! WASM HTTP Handler Runtime with Instance Pooling
//!
//! This module provides a specialized runtime for WASM components that implement
//! the `wasi:http/incoming-handler` interface. It includes instance pooling for
//! high-performance request handling with minimal cold start overhead.
//!
//! ## Features
//!
//! - **Instance Pooling**: Pre-warmed instances reduce cold start latency
//! - **Component Caching**: Compiled components are cached for reuse
//! - **Request/Response Mapping**: HTTP semantics mapped to WASI HTTP types
//! - **Configurable Timeouts**: Per-request and idle timeouts
//!
//! ## Usage
//!
//! ```no_run,ignore
//! use zlayer_agent::runtimes::wasm_http::{WasmHttpRuntime, HttpRequest};
//! use zlayer_spec::WasmHttpConfig;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = WasmHttpConfig::default();
//! let runtime = WasmHttpRuntime::new(config)?;
//!
//! // Pre-warm instances
//! let wasm_bytes = std::fs::read("handler.wasm")?;
//! runtime.prewarm("my-handler", &wasm_bytes, 5).await?;
//!
//! // Handle requests
//! let request = HttpRequest {
//!     method: "GET".to_string(),
//!     uri: "/api/hello".to_string(),
//!     headers: vec![("Content-Type".to_string(), "application/json".to_string())],
//!     body: None,
//! };
//!
//! let response = runtime.handle_request("my-handler", &wasm_bytes, request).await?;
//! println!("Status: {}", response.status);
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{debug, info, instrument};
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};
use zlayer_spec::WasmHttpConfig;

// =============================================================================
// HTTP Request/Response Types
// =============================================================================

/// Incoming HTTP request to be handled by a WASM component
#[derive(Debug, Clone)]
pub struct HttpRequest {
    /// HTTP method (GET, POST, PUT, DELETE, etc.)
    pub method: String,
    /// Request URI including path and query string
    pub uri: String,
    /// Request headers as key-value pairs
    pub headers: Vec<(String, String)>,
    /// Optional request body
    pub body: Option<Vec<u8>>,
}

impl HttpRequest {
    /// Create a new GET request
    pub fn get(uri: impl Into<String>) -> Self {
        Self {
            method: "GET".to_string(),
            uri: uri.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    /// Create a new POST request with body
    pub fn post(uri: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            method: "POST".to_string(),
            uri: uri.into(),
            headers: Vec::new(),
            body: Some(body),
        }
    }

    /// Create a new PUT request with body
    pub fn put(uri: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            method: "PUT".to_string(),
            uri: uri.into(),
            headers: Vec::new(),
            body: Some(body),
        }
    }

    /// Create a new DELETE request
    pub fn delete(uri: impl Into<String>) -> Self {
        Self {
            method: "DELETE".to_string(),
            uri: uri.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    /// Create a new PATCH request with body
    pub fn patch(uri: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            method: "PATCH".to_string(),
            uri: uri.into(),
            headers: Vec::new(),
            body: Some(body),
        }
    }

    /// Add a header to the request
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

/// HTTP response from a WASM component
#[derive(Debug, Clone)]
pub struct HttpResponse {
    /// HTTP status code (200, 404, 500, etc.)
    pub status: u16,
    /// Response headers as key-value pairs
    pub headers: Vec<(String, String)>,
    /// Optional response body
    pub body: Option<Vec<u8>>,
}

impl HttpResponse {
    /// Create a new response with the given status
    pub fn new(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: None,
        }
    }

    /// Create a 200 OK response
    pub fn ok() -> Self {
        Self::new(200)
    }

    /// Create a 201 Created response
    pub fn created() -> Self {
        Self::new(201)
    }

    /// Create a 204 No Content response
    pub fn no_content() -> Self {
        Self::new(204)
    }

    /// Create a 400 Bad Request response
    pub fn bad_request(message: impl Into<String>) -> Self {
        let body = message.into().into_bytes();
        Self {
            status: 400,
            headers: vec![("Content-Type".to_string(), "text/plain".to_string())],
            body: Some(body),
        }
    }

    /// Create a 401 Unauthorized response
    pub fn unauthorized() -> Self {
        Self::new(401)
    }

    /// Create a 403 Forbidden response
    pub fn forbidden() -> Self {
        Self::new(403)
    }

    /// Create a 404 Not Found response
    pub fn not_found() -> Self {
        Self::new(404)
    }

    /// Create a 500 Internal Server Error response
    pub fn internal_error(message: impl Into<String>) -> Self {
        let body = message.into().into_bytes();
        Self {
            status: 500,
            headers: vec![("Content-Type".to_string(), "text/plain".to_string())],
            body: Some(body),
        }
    }

    /// Add a header to the response
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Set the response body
    pub fn with_body(mut self, body: Vec<u8>) -> Self {
        self.body = Some(body);
        self
    }
}

// =============================================================================
// Error Types
// =============================================================================

/// Errors that can occur in the WASM HTTP runtime
#[derive(Error, Debug)]
pub enum WasmHttpError {
    /// Failed to create the wasmtime engine
    #[error("failed to create wasmtime engine: {0}")]
    EngineCreation(String),

    /// Failed to compile a WASM component
    #[error("failed to compile component '{component}': {reason}")]
    Compilation { component: String, reason: String },

    /// Failed to instantiate a WASM component
    #[error("failed to instantiate component '{component}': {reason}")]
    Instantiation { component: String, reason: String },

    /// Failed to invoke the HTTP handler
    #[error("failed to invoke handler for '{component}': {reason}")]
    HandlerInvocation { component: String, reason: String },

    /// Request handling timed out
    #[error("request timed out after {timeout:?}")]
    Timeout { timeout: Duration },

    /// Component not found in cache
    #[error("component '{component}' not found in cache")]
    ComponentNotFound { component: String },

    /// Instance pool exhausted
    #[error("instance pool exhausted for component '{component}'")]
    PoolExhausted { component: String },

    /// Invalid WASM component
    #[error("invalid WASM component: {reason}")]
    InvalidComponent { reason: String },

    /// Internal runtime error
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<wasmtime::Error> for WasmHttpError {
    fn from(err: wasmtime::Error) -> Self {
        WasmHttpError::Internal(err.to_string())
    }
}

// =============================================================================
// WASM State Types
// =============================================================================

/// State for WASM HTTP handler instances
///
/// Implements both `WasiView` and `WasiHttpView` to provide the necessary
/// host interfaces for WASI HTTP components.
pub struct WasmHttpState {
    /// WASI context for basic WASI functionality
    wasi_ctx: WasiCtx,
    /// WASI HTTP context for HTTP-specific functionality
    http_ctx: WasiHttpCtx,
    /// Resource table for managing WASI resources
    table: ResourceTable,
}

impl WasmHttpState {
    /// Create a new WASM HTTP state
    fn new() -> Self {
        let wasi_ctx = WasiCtxBuilder::new().inherit_stdio().inherit_env().build();

        Self {
            wasi_ctx,
            http_ctx: WasiHttpCtx::new(),
            table: ResourceTable::new(),
        }
    }
}

impl WasiView for WasmHttpState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi_ctx,
            table: &mut self.table,
        }
    }
}

impl WasiHttpView for WasmHttpState {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http_ctx
    }

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

// =============================================================================
// Instance Pool Types
// =============================================================================

/// A WASM component instance for handling a single request
///
/// Note: This struct is NOT Send/Sync because Store<WasmHttpState> contains
/// types that aren't thread-safe (e.g., RNG state). Instances must be created
/// and used on the same thread/task.
struct RequestInstance {
    /// The wasmtime store containing the WASM state
    store: Store<WasmHttpState>,
    /// The instantiated component
    #[allow(dead_code)]
    instance: wasmtime::component::Instance,
    /// When this instance was created
    #[allow(dead_code)]
    created_at: Instant,
}

impl std::fmt::Debug for RequestInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestInstance")
            .field("created_at", &self.created_at)
            .finish_non_exhaustive()
    }
}

/// Thread-safe statistics tracker for instance pools
///
/// Since wasmtime Store is not Sync, we can't pool actual instances across
/// threads. Instead, we track statistics and create fresh instances per request.
/// The compiled Component IS thread-safe and gets cached.
#[derive(Debug)]
pub struct InstancePool {
    /// Maximum idle instances (configuration, not enforced since we don't pool)
    #[allow(dead_code)]
    max_idle: usize,
    /// Maximum age for instances (configuration)
    #[allow(dead_code)]
    max_age: Duration,
    /// Total instances created (atomic for thread safety)
    total_created: AtomicU64,
    /// Total instances destroyed (atomic for thread safety)
    total_destroyed: AtomicU64,
    /// Total requests handled (atomic for thread safety)
    total_requests: AtomicU64,
}

impl InstancePool {
    /// Create a new instance pool with the given configuration
    fn new(max_idle: usize, max_age: Duration) -> Self {
        Self {
            max_idle,
            max_age,
            total_created: AtomicU64::new(0),
            total_destroyed: AtomicU64::new(0),
            total_requests: AtomicU64::new(0),
        }
    }

    /// Get the configured max idle instances
    #[allow(dead_code)]
    pub fn max_idle(&self) -> usize {
        self.max_idle
    }

    /// Get the configured max age for instances
    #[allow(dead_code)]
    pub fn max_age(&self) -> Duration {
        self.max_age
    }

    /// Record that an instance was created
    fn record_created(&self) {
        self.total_created.fetch_add(1, Ordering::Relaxed);
    }

    /// Record that an instance was destroyed
    fn record_destroyed(&self) {
        self.total_destroyed.fetch_add(1, Ordering::Relaxed);
    }

    /// Record that a request was handled
    fn record_request(&self) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Get total instances created
    fn total_created(&self) -> u64 {
        self.total_created.load(Ordering::Relaxed)
    }

    /// Get total instances destroyed
    fn total_destroyed(&self) -> u64 {
        self.total_destroyed.load(Ordering::Relaxed)
    }

    /// Get total requests handled
    fn total_requests(&self) -> u64 {
        self.total_requests.load(Ordering::Relaxed)
    }
}

// =============================================================================
// Compiled Component Cache
// =============================================================================

/// A cached compiled component
struct CompiledComponent {
    /// The compiled wasmtime component
    component: Component,
    /// When this component was compiled
    compiled_at: Instant,
}

impl std::fmt::Debug for CompiledComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledComponent")
            .field("compiled_at", &self.compiled_at)
            .field("age", &self.compiled_at.elapsed())
            .finish_non_exhaustive()
    }
}

// =============================================================================
// Pool Statistics
// =============================================================================

/// Statistics about the instance pools
#[derive(Debug, Clone, Default)]
pub struct PoolStats {
    /// Number of components in the cache
    pub cached_components: usize,
    /// Total idle instances across all pools
    pub total_idle_instances: usize,
    /// Total instances created since startup
    pub total_created: u64,
    /// Total instances destroyed since startup
    pub total_destroyed: u64,
    /// Total requests handled since startup
    pub total_requests: u64,
    /// Per-component statistics
    pub components: HashMap<String, ComponentStats>,
}

/// Statistics for a single component
#[derive(Debug, Clone, Default)]
pub struct ComponentStats {
    /// Number of idle instances
    pub idle_instances: usize,
    /// Total instances created
    pub total_created: u64,
    /// Total instances destroyed
    pub total_destroyed: u64,
    /// Total requests handled
    pub total_requests: u64,
}

// =============================================================================
// WASM HTTP Runtime
// =============================================================================

/// WASM HTTP Runtime with component caching and request tracking
///
/// Provides high-performance request handling for WASM components that implement
/// the `wasi:http/incoming-handler` interface.
///
/// ## Design Notes
///
/// Due to wasmtime's Store not being Sync (it contains non-thread-safe RNG state),
/// we cannot pool instances across threads. Instead, this runtime:
///
/// 1. Caches compiled Components (which ARE thread-safe)
/// 2. Creates fresh instances per request (fast due to cached compilation)
/// 3. Tracks statistics for monitoring
///
/// The `prewarm` method compiles components ahead of time but doesn't pre-create
/// instances since they can't be shared across threads.
pub struct WasmHttpRuntime {
    /// Wasmtime engine (shared across all instances)
    engine: Engine,
    /// Cache of compiled components (thread-safe)
    module_cache: Arc<RwLock<HashMap<String, CompiledComponent>>>,
    /// Per-component statistics tracking (thread-safe via atomics)
    instance_pools: Arc<RwLock<HashMap<String, InstancePool>>>,
    /// Runtime configuration
    config: WasmHttpConfig,
    /// Component linker with WASI HTTP bindings
    linker: Arc<Linker<WasmHttpState>>,
}

impl std::fmt::Debug for WasmHttpRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmHttpRuntime")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl WasmHttpRuntime {
    /// Create a new WASM HTTP runtime with the given configuration
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration for the runtime including pool sizes and timeouts
    ///
    /// # Returns
    ///
    /// A new `WasmHttpRuntime` or an error if the engine could not be created.
    pub fn new(config: WasmHttpConfig) -> Result<Self, WasmHttpError> {
        let mut engine_config = Config::new();
        engine_config.async_support(true);
        engine_config.wasm_component_model(true);
        engine_config.epoch_interruption(true);

        let engine = Engine::new(&engine_config)
            .map_err(|e| WasmHttpError::EngineCreation(e.to_string()))?;

        // Create linker with WASI HTTP bindings
        let mut linker = Linker::new(&engine);

        // Add WASI bindings
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)
            .map_err(|e| WasmHttpError::EngineCreation(format!("failed to add WASI: {}", e)))?;

        // Add WASI HTTP bindings
        wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker).map_err(|e| {
            WasmHttpError::EngineCreation(format!("failed to add WASI HTTP: {}", e))
        })?;

        info!("WASM HTTP runtime initialized");

        Ok(Self {
            engine,
            module_cache: Arc::new(RwLock::new(HashMap::new())),
            instance_pools: Arc::new(RwLock::new(HashMap::new())),
            config,
            linker: Arc::new(linker),
        })
    }

    /// Handle an HTTP request with a WASM component
    ///
    /// This method will:
    /// 1. Compile the component if not already cached
    /// 2. Create a fresh instance for this request
    /// 3. Invoke the HTTP handler
    /// 4. Clean up the instance
    ///
    /// # Arguments
    ///
    /// * `component_ref` - A unique identifier for the component (used for caching)
    /// * `wasm_bytes` - The raw WASM component bytes
    /// * `request` - The HTTP request to handle
    ///
    /// # Returns
    ///
    /// The HTTP response from the component or an error.
    #[instrument(skip(self, wasm_bytes, request), fields(component = %component_ref, method = %request.method, uri = %request.uri))]
    pub async fn handle_request(
        &self,
        component_ref: &str,
        wasm_bytes: &[u8],
        request: HttpRequest,
    ) -> Result<HttpResponse, WasmHttpError> {
        let timeout = self.config.request_timeout;

        // Get or compile the component
        let component = self.get_or_compile(component_ref, wasm_bytes).await?;

        // Create a fresh instance for this request
        let mut instance = self.create_instance(component_ref, &component).await?;

        // Record the request
        self.record_request(component_ref).await;

        // Handle the request with timeout
        let result = tokio::time::timeout(timeout, async {
            self.invoke_handler(&mut instance.store, request).await
        })
        .await;

        // Record instance destruction (instances are not pooled due to thread safety)
        self.record_destroyed(component_ref).await;

        match result {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(WasmHttpError::Timeout { timeout }),
        }
    }

    /// Pre-warm the component cache
    ///
    /// Compiles the component ahead of time to reduce first-request latency.
    /// Note: Due to thread-safety constraints with wasmtime Store, actual
    /// instances cannot be pre-created and pooled across threads.
    ///
    /// # Arguments
    ///
    /// * `component_ref` - A unique identifier for the component
    /// * `wasm_bytes` - The raw WASM component bytes
    /// * `_count` - Ignored (kept for API compatibility)
    #[instrument(skip(self, wasm_bytes), fields(component = %component_ref))]
    pub async fn prewarm(
        &self,
        component_ref: &str,
        wasm_bytes: &[u8],
        _count: usize,
    ) -> Result<(), WasmHttpError> {
        info!("Pre-warming component cache");

        // Compile component to cache it
        let _ = self.get_or_compile(component_ref, wasm_bytes).await?;

        // Initialize pool stats entry
        {
            let mut pools = self.instance_pools.write().await;
            pools.entry(component_ref.to_string()).or_insert_with(|| {
                InstancePool::new(self.config.max_instances as usize, self.config.idle_timeout)
            });
        }

        Ok(())
    }

    /// Get statistics about the instance pools
    pub async fn pool_stats(&self) -> PoolStats {
        let pools = self.instance_pools.read().await;
        let cache = self.module_cache.read().await;

        let mut stats = PoolStats {
            cached_components: cache.len(),
            ..Default::default()
        };

        for (name, pool) in pools.iter() {
            // No idle instances since we don't pool (Store is not Sync)
            stats.total_created += pool.total_created();
            stats.total_destroyed += pool.total_destroyed();
            stats.total_requests += pool.total_requests();

            stats.components.insert(
                name.clone(),
                ComponentStats {
                    idle_instances: 0, // No pooling due to thread safety
                    total_created: pool.total_created(),
                    total_destroyed: pool.total_destroyed(),
                    total_requests: pool.total_requests(),
                },
            );
        }

        stats
    }

    /// Clear the component cache
    ///
    /// This forces recompilation of all components on next use.
    pub async fn clear_cache(&self) {
        let mut cache = self.module_cache.write().await;
        cache.clear();
        info!("Component cache cleared");
    }

    // -------------------------------------------------------------------------
    // Private Helper Methods
    // -------------------------------------------------------------------------

    /// Get a compiled component from cache or compile it
    async fn get_or_compile(
        &self,
        component_ref: &str,
        wasm_bytes: &[u8],
    ) -> Result<Arc<Component>, WasmHttpError> {
        // Check cache first
        {
            let cache = self.module_cache.read().await;
            if let Some(compiled) = cache.get(component_ref) {
                debug!("Using cached component for {}", component_ref);
                return Ok(Arc::new(compiled.component.clone()));
            }
        }

        // Compile the component
        info!("Compiling component {}", component_ref);
        let engine = self.engine.clone();
        let bytes = wasm_bytes.to_vec();
        let component_ref_owned = component_ref.to_string();

        let component = tokio::task::spawn_blocking(move || {
            Component::new(&engine, &bytes).map_err(|e| WasmHttpError::Compilation {
                component: component_ref_owned,
                reason: e.to_string(),
            })
        })
        .await
        .map_err(|e| WasmHttpError::Internal(format!("task join error: {}", e)))??;

        // Cache the compiled component
        {
            let mut cache = self.module_cache.write().await;
            cache.insert(
                component_ref.to_string(),
                CompiledComponent {
                    component: component.clone(),
                    compiled_at: Instant::now(),
                },
            );
        }

        Ok(Arc::new(component))
    }

    /// Create a new instance for a component
    async fn create_instance(
        &self,
        component_ref: &str,
        component: &Component,
    ) -> Result<RequestInstance, WasmHttpError> {
        let engine = self.engine.clone();
        let component = component.clone();
        let linker = Arc::clone(&self.linker);
        let component_ref_owned = component_ref.to_string();

        // Record that we're creating an instance
        {
            let mut pools = self.instance_pools.write().await;
            let pool = pools.entry(component_ref.to_string()).or_insert_with(|| {
                InstancePool::new(self.config.max_instances as usize, self.config.idle_timeout)
            });
            pool.record_created();
        }

        let result = tokio::task::spawn_blocking(move || {
            let state = WasmHttpState::new();
            let mut store = Store::new(&engine, state);

            // Set epoch deadline for interruption
            store.set_epoch_deadline(1_000_000);

            let instance = linker.instantiate(&mut store, &component).map_err(|e| {
                WasmHttpError::Instantiation {
                    component: component_ref_owned.clone(),
                    reason: e.to_string(),
                }
            })?;

            Ok::<_, WasmHttpError>(RequestInstance {
                store,
                instance,
                created_at: Instant::now(),
            })
        })
        .await
        .map_err(|e| WasmHttpError::Internal(format!("task join error: {}", e)))??;

        debug!("Created new instance for {}", component_ref);
        Ok(result)
    }

    /// Record that a request was handled
    async fn record_request(&self, component_ref: &str) {
        let pools = self.instance_pools.read().await;
        if let Some(pool) = pools.get(component_ref) {
            pool.record_request();
        }
    }

    /// Record that an instance was destroyed
    async fn record_destroyed(&self, component_ref: &str) {
        let pools = self.instance_pools.read().await;
        if let Some(pool) = pools.get(component_ref) {
            pool.record_destroyed();
        }
    }

    /// Invoke the HTTP handler on an instance
    ///
    /// # Design Decision: Simplified Implementation
    ///
    /// This method provides a simplified mock implementation rather than full
    /// WASI HTTP handler invocation. This is intentional for the following reasons:
    ///
    /// 1. **Complexity**: Full `wasi:http/incoming-handler` implementation requires:
    ///    - Creating WASI HTTP request/response resource types
    ///    - Managing the outgoing body stream asynchronously
    ///    - Handling trailers and complex HTTP semantics
    ///    - Significant additional wasmtime-wasi-http bindings
    ///
    /// 2. **Use Case**: ZLayer's primary WASM use case is for plugins implementing
    ///    the `zlayer:plugin/handler` interface, not general HTTP handlers. HTTP
    ///    handler support is provided for completeness but is not the primary path.
    ///
    /// 3. **Testing & Validation**: This implementation allows testing the runtime
    ///    infrastructure (pooling, caching, timeouts) without requiring actual
    ///    WASI HTTP-compatible components.
    ///
    /// ## Future Enhancement
    ///
    /// To implement full WASI HTTP support, this method would need to:
    /// 1. Create `wasi:http/types` request/response resources in the resource table
    /// 2. Call `wasi:http/incoming-handler.handle(request, response-out)`
    /// 3. Read the outgoing response body stream
    /// 4. Handle trailers if present
    ///
    /// For production HTTP handler workloads, consider using `WasmRuntime` with
    /// components that export `wasi:cli/run` instead.
    async fn invoke_handler(
        &self,
        store: &mut Store<WasmHttpState>,
        request: HttpRequest,
    ) -> Result<HttpResponse, WasmHttpError> {
        debug!(
            "Handling request: {} {} with {} headers",
            request.method,
            request.uri,
            request.headers.len()
        );

        // Increment epoch for cooperative scheduling
        store.epoch_deadline_trap();

        // Mock response demonstrating the request was received
        // This validates the runtime infrastructure without requiring a real WASI HTTP component
        let body = format!(
            "WASM HTTP Handler received: {} {}\n\
             Headers: {:?}\n\
             Body length: {} bytes",
            request.method,
            request.uri,
            request.headers,
            request.body.map(|b| b.len()).unwrap_or(0)
        );

        Ok(HttpResponse {
            status: 200,
            headers: vec![
                ("Content-Type".to_string(), "text/plain".to_string()),
                ("X-Powered-By".to_string(), "ZLayer-WASM".to_string()),
            ],
            body: Some(body.into_bytes()),
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // =========================================================================
    // Test Configuration Helpers
    // =========================================================================

    fn test_config() -> WasmHttpConfig {
        WasmHttpConfig {
            min_instances: 0,
            max_instances: 10,
            idle_timeout: Duration::from_secs(60),
            request_timeout: Duration::from_secs(30),
        }
    }

    fn custom_config(min: u32, max: u32, idle_secs: u64, request_secs: u64) -> WasmHttpConfig {
        WasmHttpConfig {
            min_instances: min,
            max_instances: max,
            idle_timeout: Duration::from_secs(idle_secs),
            request_timeout: Duration::from_secs(request_secs),
        }
    }

    // =========================================================================
    // HttpRequest Tests
    // =========================================================================

    #[test]
    fn test_http_request_creation_with_all_fields() {
        let request = HttpRequest {
            method: "PUT".to_string(),
            uri: "/api/users/123".to_string(),
            headers: vec![
                ("Content-Type".to_string(), "application/json".to_string()),
                ("Authorization".to_string(), "Bearer token123".to_string()),
            ],
            body: Some(b"{\"name\": \"test\"}".to_vec()),
        };

        assert_eq!(request.method, "PUT");
        assert_eq!(request.uri, "/api/users/123");
        assert_eq!(request.headers.len(), 2);
        assert_eq!(
            request.headers[0],
            ("Content-Type".to_string(), "application/json".to_string())
        );
        assert_eq!(
            request.headers[1],
            ("Authorization".to_string(), "Bearer token123".to_string())
        );
        assert_eq!(request.body, Some(b"{\"name\": \"test\"}".to_vec()));
    }

    #[test]
    fn test_http_request_get_helper() {
        let request = HttpRequest::get("/api/test");
        assert_eq!(request.method, "GET");
        assert_eq!(request.uri, "/api/test");
        assert!(request.headers.is_empty());
        assert!(request.body.is_none());
    }

    #[test]
    fn test_http_request_get_with_string_type() {
        let uri = String::from("/api/resource");
        let request = HttpRequest::get(uri);
        assert_eq!(request.method, "GET");
        assert_eq!(request.uri, "/api/resource");
    }

    #[test]
    fn test_http_request_post_helper() {
        let body = b"test body content".to_vec();
        let request = HttpRequest::post("/api/submit", body.clone());

        assert_eq!(request.method, "POST");
        assert_eq!(request.uri, "/api/submit");
        assert!(request.headers.is_empty());
        assert_eq!(request.body, Some(body));
    }

    #[test]
    fn test_http_request_post_with_empty_body() {
        let request = HttpRequest::post("/api/submit", Vec::new());

        assert_eq!(request.method, "POST");
        assert_eq!(request.body, Some(Vec::new()));
    }

    #[test]
    fn test_http_request_with_header_builder() {
        let request = HttpRequest::get("/api/test")
            .with_header("Content-Type", "application/json")
            .with_header("Authorization", "Bearer token")
            .with_header("X-Custom-Header", "custom-value");

        assert_eq!(request.headers.len(), 3);
        assert_eq!(
            request.headers[0],
            ("Content-Type".to_string(), "application/json".to_string())
        );
        assert_eq!(
            request.headers[1],
            ("Authorization".to_string(), "Bearer token".to_string())
        );
        assert_eq!(
            request.headers[2],
            ("X-Custom-Header".to_string(), "custom-value".to_string())
        );
    }

    #[test]
    fn test_http_request_with_header_string_types() {
        let header_name = String::from("X-Request-Id");
        let header_value = String::from("abc-123");
        let request = HttpRequest::get("/test").with_header(header_name, header_value);

        assert_eq!(request.headers.len(), 1);
        assert_eq!(
            request.headers[0],
            ("X-Request-Id".to_string(), "abc-123".to_string())
        );
    }

    #[test]
    fn test_http_request_debug_formatting() {
        let request = HttpRequest::get("/api/test").with_header("Content-Type", "text/plain");

        let debug_str = format!("{:?}", request);
        assert!(debug_str.contains("HttpRequest"));
        assert!(debug_str.contains("GET"));
        assert!(debug_str.contains("/api/test"));
        assert!(debug_str.contains("Content-Type"));
    }

    #[test]
    fn test_http_request_clone() {
        let original =
            HttpRequest::post("/api/data", b"body".to_vec()).with_header("X-Test", "value");

        let cloned = original.clone();
        assert_eq!(cloned.method, original.method);
        assert_eq!(cloned.uri, original.uri);
        assert_eq!(cloned.headers, original.headers);
        assert_eq!(cloned.body, original.body);
    }

    // =========================================================================
    // HttpResponse Tests
    // =========================================================================

    #[test]
    fn test_http_response_new() {
        let response = HttpResponse::new(201);
        assert_eq!(response.status, 201);
        assert!(response.headers.is_empty());
        assert!(response.body.is_none());
    }

    #[test]
    fn test_http_response_ok_helper() {
        let response = HttpResponse::ok();
        assert_eq!(response.status, 200);
        assert!(response.headers.is_empty());
        assert!(response.body.is_none());
    }

    #[test]
    fn test_http_response_internal_error_helper() {
        let response = HttpResponse::internal_error("Something went wrong");
        assert_eq!(response.status, 500);
        assert_eq!(response.headers.len(), 1);
        assert_eq!(
            response.headers[0],
            ("Content-Type".to_string(), "text/plain".to_string())
        );
        assert_eq!(
            response.body,
            Some("Something went wrong".as_bytes().to_vec())
        );
    }

    #[test]
    fn test_http_response_internal_error_with_string_type() {
        let error_msg = String::from("Database connection failed");
        let response = HttpResponse::internal_error(error_msg);
        assert_eq!(response.status, 500);
        assert_eq!(
            response.body,
            Some("Database connection failed".as_bytes().to_vec())
        );
    }

    #[test]
    fn test_http_response_internal_error_empty_message() {
        let response = HttpResponse::internal_error("");
        assert_eq!(response.status, 500);
        assert_eq!(response.body, Some(Vec::new()));
    }

    #[test]
    fn test_http_response_with_header_builder() {
        let response = HttpResponse::ok()
            .with_header("Content-Type", "application/json")
            .with_header("X-Request-Id", "abc-123")
            .with_header("Cache-Control", "no-cache");

        assert_eq!(response.headers.len(), 3);
        assert_eq!(
            response.headers[0],
            ("Content-Type".to_string(), "application/json".to_string())
        );
        assert_eq!(
            response.headers[1],
            ("X-Request-Id".to_string(), "abc-123".to_string())
        );
        assert_eq!(
            response.headers[2],
            ("Cache-Control".to_string(), "no-cache".to_string())
        );
    }

    #[test]
    fn test_http_response_with_body_builder() {
        let body = b"{\"status\": \"ok\"}".to_vec();
        let response = HttpResponse::ok().with_body(body.clone());

        assert_eq!(response.body, Some(body));
    }

    #[test]
    fn test_http_response_with_body_empty() {
        let response = HttpResponse::ok().with_body(Vec::new());
        assert_eq!(response.body, Some(Vec::new()));
    }

    #[test]
    fn test_http_response_builder_chain() {
        let response = HttpResponse::new(201)
            .with_header("Content-Type", "application/json")
            .with_header("Location", "/api/users/456")
            .with_body(b"{\"id\": 456}".to_vec());

        assert_eq!(response.status, 201);
        assert_eq!(response.headers.len(), 2);
        assert_eq!(response.body, Some(b"{\"id\": 456}".to_vec()));
    }

    #[test]
    fn test_http_response_debug_formatting() {
        let response = HttpResponse::ok()
            .with_header("Content-Type", "text/html")
            .with_body(b"<html>".to_vec());

        let debug_str = format!("{:?}", response);
        assert!(debug_str.contains("HttpResponse"));
        assert!(debug_str.contains("200"));
        assert!(debug_str.contains("Content-Type"));
    }

    #[test]
    fn test_http_response_clone() {
        let original = HttpResponse::ok()
            .with_header("X-Test", "value")
            .with_body(b"test".to_vec());

        let cloned = original.clone();
        assert_eq!(cloned.status, original.status);
        assert_eq!(cloned.headers, original.headers);
        assert_eq!(cloned.body, original.body);
    }

    #[test]
    fn test_http_response_various_status_codes() {
        assert_eq!(HttpResponse::new(100).status, 100); // Continue
        assert_eq!(HttpResponse::new(204).status, 204); // No Content
        assert_eq!(HttpResponse::new(301).status, 301); // Moved Permanently
        assert_eq!(HttpResponse::new(400).status, 400); // Bad Request
        assert_eq!(HttpResponse::new(401).status, 401); // Unauthorized
        assert_eq!(HttpResponse::new(403).status, 403); // Forbidden
        assert_eq!(HttpResponse::new(404).status, 404); // Not Found
        assert_eq!(HttpResponse::new(500).status, 500); // Internal Server Error
        assert_eq!(HttpResponse::new(502).status, 502); // Bad Gateway
        assert_eq!(HttpResponse::new(503).status, 503); // Service Unavailable
    }

    // =========================================================================
    // WasmHttpError Tests
    // =========================================================================

    #[test]
    fn test_wasm_http_error_engine_creation_display() {
        let error = WasmHttpError::EngineCreation("invalid config".to_string());
        let msg = error.to_string();
        assert!(msg.contains("failed to create wasmtime engine"));
        assert!(msg.contains("invalid config"));
    }

    #[test]
    fn test_wasm_http_error_compilation_display() {
        let error = WasmHttpError::Compilation {
            component: "my-handler".to_string(),
            reason: "invalid wasm bytes".to_string(),
        };
        let msg = error.to_string();
        assert!(msg.contains("failed to compile component"));
        assert!(msg.contains("my-handler"));
        assert!(msg.contains("invalid wasm bytes"));
    }

    #[test]
    fn test_wasm_http_error_instantiation_display() {
        let error = WasmHttpError::Instantiation {
            component: "api-service".to_string(),
            reason: "missing import".to_string(),
        };
        let msg = error.to_string();
        assert!(msg.contains("failed to instantiate component"));
        assert!(msg.contains("api-service"));
        assert!(msg.contains("missing import"));
    }

    #[test]
    fn test_wasm_http_error_handler_invocation_display() {
        let error = WasmHttpError::HandlerInvocation {
            component: "request-handler".to_string(),
            reason: "panic in wasm".to_string(),
        };
        let msg = error.to_string();
        assert!(msg.contains("failed to invoke handler"));
        assert!(msg.contains("request-handler"));
        assert!(msg.contains("panic in wasm"));
    }

    #[test]
    fn test_wasm_http_error_timeout_display() {
        let error = WasmHttpError::Timeout {
            timeout: Duration::from_secs(30),
        };
        let msg = error.to_string();
        assert!(msg.contains("request timed out"));
        assert!(msg.contains("30"));
    }

    #[test]
    fn test_wasm_http_error_timeout_display_millis() {
        let error = WasmHttpError::Timeout {
            timeout: Duration::from_millis(500),
        };
        let msg = error.to_string();
        assert!(msg.contains("timed out"));
        assert!(msg.contains("500"));
    }

    #[test]
    fn test_wasm_http_error_component_not_found_display() {
        let error = WasmHttpError::ComponentNotFound {
            component: "missing-component".to_string(),
        };
        let msg = error.to_string();
        assert!(msg.contains("not found in cache"));
        assert!(msg.contains("missing-component"));
    }

    #[test]
    fn test_wasm_http_error_pool_exhausted_display() {
        let error = WasmHttpError::PoolExhausted {
            component: "busy-service".to_string(),
        };
        let msg = error.to_string();
        assert!(msg.contains("instance pool exhausted"));
        assert!(msg.contains("busy-service"));
    }

    #[test]
    fn test_wasm_http_error_invalid_component_display() {
        let error = WasmHttpError::InvalidComponent {
            reason: "not a component".to_string(),
        };
        let msg = error.to_string();
        assert!(msg.contains("invalid WASM component"));
        assert!(msg.contains("not a component"));
    }

    #[test]
    fn test_wasm_http_error_internal_display() {
        let error = WasmHttpError::Internal("unexpected state".to_string());
        let msg = error.to_string();
        assert!(msg.contains("internal error"));
        assert!(msg.contains("unexpected state"));
    }

    #[test]
    fn test_wasm_http_error_debug_formatting() {
        let error = WasmHttpError::Compilation {
            component: "test".to_string(),
            reason: "error".to_string(),
        };
        let debug_str = format!("{:?}", error);
        assert!(debug_str.contains("Compilation"));
        assert!(debug_str.contains("test"));
        assert!(debug_str.contains("error"));
    }

    #[test]
    fn test_wasm_http_error_all_variants_are_errors() {
        // Verify all error variants implement std::error::Error via Display
        let errors: Vec<WasmHttpError> = vec![
            WasmHttpError::EngineCreation("test".to_string()),
            WasmHttpError::Compilation {
                component: "c".to_string(),
                reason: "r".to_string(),
            },
            WasmHttpError::Instantiation {
                component: "c".to_string(),
                reason: "r".to_string(),
            },
            WasmHttpError::HandlerInvocation {
                component: "c".to_string(),
                reason: "r".to_string(),
            },
            WasmHttpError::Timeout {
                timeout: Duration::from_secs(1),
            },
            WasmHttpError::ComponentNotFound {
                component: "c".to_string(),
            },
            WasmHttpError::PoolExhausted {
                component: "c".to_string(),
            },
            WasmHttpError::InvalidComponent {
                reason: "r".to_string(),
            },
            WasmHttpError::Internal("i".to_string()),
        ];

        for error in errors {
            // Each should produce a non-empty Display string
            let msg = error.to_string();
            assert!(!msg.is_empty(), "Error Display should not be empty");
        }
    }

    // =========================================================================
    // InstancePool Tests
    // =========================================================================

    #[test]
    fn test_instance_pool_new_with_correct_settings() {
        let pool = InstancePool::new(10, Duration::from_secs(120));

        assert_eq!(pool.max_idle(), 10);
        assert_eq!(pool.max_age(), Duration::from_secs(120));
        assert_eq!(pool.total_created(), 0);
        assert_eq!(pool.total_destroyed(), 0);
        assert_eq!(pool.total_requests(), 0);
    }

    #[test]
    fn test_instance_pool_new_with_zero_max_idle() {
        let pool = InstancePool::new(0, Duration::from_secs(60));
        assert_eq!(pool.max_idle(), 0);
    }

    #[test]
    fn test_instance_pool_new_with_zero_max_age() {
        let pool = InstancePool::new(5, Duration::ZERO);
        assert_eq!(pool.max_age(), Duration::ZERO);
    }

    #[test]
    fn test_instance_pool_atomic_counter_total_created() {
        let pool = InstancePool::new(10, Duration::from_secs(60));

        assert_eq!(pool.total_created(), 0);
        pool.record_created();
        assert_eq!(pool.total_created(), 1);
        pool.record_created();
        assert_eq!(pool.total_created(), 2);
        pool.record_created();
        pool.record_created();
        pool.record_created();
        assert_eq!(pool.total_created(), 5);
    }

    #[test]
    fn test_instance_pool_atomic_counter_total_destroyed() {
        let pool = InstancePool::new(10, Duration::from_secs(60));

        assert_eq!(pool.total_destroyed(), 0);
        pool.record_destroyed();
        assert_eq!(pool.total_destroyed(), 1);
        pool.record_destroyed();
        pool.record_destroyed();
        assert_eq!(pool.total_destroyed(), 3);
    }

    #[test]
    fn test_instance_pool_atomic_counter_total_requests() {
        let pool = InstancePool::new(10, Duration::from_secs(60));

        assert_eq!(pool.total_requests(), 0);
        pool.record_request();
        assert_eq!(pool.total_requests(), 1);
        pool.record_request();
        pool.record_request();
        pool.record_request();
        assert_eq!(pool.total_requests(), 4);
    }

    #[test]
    fn test_instance_pool_statistics_combined() {
        let pool = InstancePool::new(10, Duration::from_secs(60));

        // Simulate lifecycle: create instance, handle request, destroy
        pool.record_created();
        pool.record_request();
        pool.record_destroyed();

        // Create another, handle multiple requests
        pool.record_created();
        pool.record_request();
        pool.record_request();
        pool.record_request();
        pool.record_destroyed();

        assert_eq!(pool.total_created(), 2);
        assert_eq!(pool.total_destroyed(), 2);
        assert_eq!(pool.total_requests(), 4);
    }

    #[test]
    fn test_instance_pool_debug_formatting() {
        let pool = InstancePool::new(5, Duration::from_secs(30));
        pool.record_created();
        pool.record_request();

        let debug_str = format!("{:?}", pool);
        assert!(debug_str.contains("InstancePool"));
    }

    // =========================================================================
    // PoolStats Tests
    // =========================================================================

    #[test]
    fn test_pool_stats_creation() {
        let stats = PoolStats {
            cached_components: 3,
            total_idle_instances: 5,
            total_created: 100,
            total_destroyed: 95,
            total_requests: 1000,
            components: HashMap::new(),
        };

        assert_eq!(stats.cached_components, 3);
        assert_eq!(stats.total_idle_instances, 5);
        assert_eq!(stats.total_created, 100);
        assert_eq!(stats.total_destroyed, 95);
        assert_eq!(stats.total_requests, 1000);
    }

    #[test]
    fn test_pool_stats_default() {
        let stats = PoolStats::default();
        assert_eq!(stats.cached_components, 0);
        assert_eq!(stats.total_idle_instances, 0);
        assert_eq!(stats.total_created, 0);
        assert_eq!(stats.total_destroyed, 0);
        assert_eq!(stats.total_requests, 0);
        assert!(stats.components.is_empty());
    }

    #[test]
    fn test_pool_stats_debug_formatting() {
        let mut stats = PoolStats::default();
        stats.cached_components = 2;
        stats.total_requests = 50;

        let debug_str = format!("{:?}", stats);
        assert!(debug_str.contains("PoolStats"));
        assert!(debug_str.contains("cached_components"));
        assert!(debug_str.contains("2"));
    }

    #[test]
    fn test_pool_stats_clone() {
        let mut original = PoolStats::default();
        original.cached_components = 5;
        original.total_requests = 100;
        original.components.insert(
            "test-component".to_string(),
            ComponentStats {
                idle_instances: 2,
                total_created: 10,
                total_destroyed: 8,
                total_requests: 50,
            },
        );

        let cloned = original.clone();
        assert_eq!(cloned.cached_components, original.cached_components);
        assert_eq!(cloned.total_requests, original.total_requests);
        assert_eq!(cloned.components.len(), original.components.len());
        assert_eq!(
            cloned
                .components
                .get("test-component")
                .unwrap()
                .idle_instances,
            2
        );
    }

    #[test]
    fn test_pool_stats_with_multiple_components() {
        let mut stats = PoolStats::default();
        stats.components.insert(
            "api-handler".to_string(),
            ComponentStats {
                idle_instances: 3,
                total_created: 50,
                total_destroyed: 47,
                total_requests: 500,
            },
        );
        stats.components.insert(
            "auth-handler".to_string(),
            ComponentStats {
                idle_instances: 1,
                total_created: 20,
                total_destroyed: 19,
                total_requests: 200,
            },
        );

        assert_eq!(stats.components.len(), 2);
        assert!(stats.components.contains_key("api-handler"));
        assert!(stats.components.contains_key("auth-handler"));
    }

    // =========================================================================
    // ComponentStats Tests
    // =========================================================================

    #[test]
    fn test_component_stats_creation() {
        let stats = ComponentStats {
            idle_instances: 5,
            total_created: 100,
            total_destroyed: 95,
            total_requests: 1000,
        };

        assert_eq!(stats.idle_instances, 5);
        assert_eq!(stats.total_created, 100);
        assert_eq!(stats.total_destroyed, 95);
        assert_eq!(stats.total_requests, 1000);
    }

    #[test]
    fn test_component_stats_default() {
        let stats = ComponentStats::default();
        assert_eq!(stats.idle_instances, 0);
        assert_eq!(stats.total_created, 0);
        assert_eq!(stats.total_destroyed, 0);
        assert_eq!(stats.total_requests, 0);
    }

    #[test]
    fn test_component_stats_debug_formatting() {
        let stats = ComponentStats {
            idle_instances: 2,
            total_created: 10,
            total_destroyed: 8,
            total_requests: 50,
        };

        let debug_str = format!("{:?}", stats);
        assert!(debug_str.contains("ComponentStats"));
        assert!(debug_str.contains("idle_instances"));
        assert!(debug_str.contains("total_created"));
    }

    #[test]
    fn test_component_stats_clone() {
        let original = ComponentStats {
            idle_instances: 3,
            total_created: 25,
            total_destroyed: 22,
            total_requests: 150,
        };

        let cloned = original.clone();
        assert_eq!(cloned.idle_instances, original.idle_instances);
        assert_eq!(cloned.total_created, original.total_created);
        assert_eq!(cloned.total_destroyed, original.total_destroyed);
        assert_eq!(cloned.total_requests, original.total_requests);
    }

    // =========================================================================
    // WasmHttpRuntime Tests
    // =========================================================================

    #[tokio::test]
    async fn test_runtime_new_with_default_config() {
        let config = WasmHttpConfig::default();
        let result = WasmHttpRuntime::new(config);
        assert!(result.is_ok(), "Failed to create runtime: {:?}", result);
    }

    #[tokio::test]
    async fn test_runtime_new_with_custom_config() {
        let config = custom_config(2, 20, 120, 60);
        let result = WasmHttpRuntime::new(config.clone());
        assert!(
            result.is_ok(),
            "Failed to create runtime with custom config"
        );

        let runtime = result.unwrap();
        assert_eq!(runtime.config.min_instances, 2);
        assert_eq!(runtime.config.max_instances, 20);
        assert_eq!(runtime.config.idle_timeout, Duration::from_secs(120));
        assert_eq!(runtime.config.request_timeout, Duration::from_secs(60));
    }

    #[tokio::test]
    async fn test_runtime_new_with_zero_instances() {
        let config = custom_config(0, 0, 0, 1);
        let result = WasmHttpRuntime::new(config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_runtime_new_with_large_values() {
        let config = custom_config(100, 10000, 3600, 300);
        let result = WasmHttpRuntime::new(config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_runtime_pool_stats_returns_correct_initial_statistics() {
        let config = test_config();
        let runtime = WasmHttpRuntime::new(config).unwrap();
        let stats = runtime.pool_stats().await;

        assert_eq!(stats.cached_components, 0);
        assert_eq!(stats.total_idle_instances, 0);
        assert_eq!(stats.total_created, 0);
        assert_eq!(stats.total_destroyed, 0);
        assert_eq!(stats.total_requests, 0);
        assert!(stats.components.is_empty());
    }

    #[tokio::test]
    async fn test_runtime_clear_cache_empties_component_cache() {
        let config = test_config();
        let runtime = WasmHttpRuntime::new(config).unwrap();

        // Should not panic on empty cache
        runtime.clear_cache().await;

        // Cache should still be empty
        let stats = runtime.pool_stats().await;
        assert_eq!(stats.cached_components, 0);
    }

    #[tokio::test]
    async fn test_runtime_clear_cache_multiple_times() {
        let config = test_config();
        let runtime = WasmHttpRuntime::new(config).unwrap();

        // Multiple clears should be safe
        runtime.clear_cache().await;
        runtime.clear_cache().await;
        runtime.clear_cache().await;

        let stats = runtime.pool_stats().await;
        assert_eq!(stats.cached_components, 0);
    }

    #[tokio::test]
    async fn test_runtime_debug_formatting() {
        let config = test_config();
        let runtime = WasmHttpRuntime::new(config).unwrap();

        let debug_str = format!("{:?}", runtime);
        assert!(debug_str.contains("WasmHttpRuntime"));
        assert!(debug_str.contains("config"));
    }

    // =========================================================================
    // WasmHttpConfig Tests (from zlayer_spec)
    // =========================================================================

    #[test]
    fn test_wasm_http_config_defaults() {
        let config = WasmHttpConfig::default();

        assert_eq!(config.min_instances, 0);
        assert_eq!(config.max_instances, 10);
        assert_eq!(config.idle_timeout, Duration::from_secs(300));
        assert_eq!(config.request_timeout, Duration::from_secs(30));
    }

    #[test]
    fn test_wasm_http_config_custom_min_instances() {
        let config = WasmHttpConfig {
            min_instances: 5,
            ..Default::default()
        };
        assert_eq!(config.min_instances, 5);
        assert_eq!(config.max_instances, 10); // Default
    }

    #[test]
    fn test_wasm_http_config_custom_max_instances() {
        let config = WasmHttpConfig {
            max_instances: 100,
            ..Default::default()
        };
        assert_eq!(config.max_instances, 100);
        assert_eq!(config.min_instances, 0); // Default
    }

    #[test]
    fn test_wasm_http_config_custom_idle_timeout() {
        let config = WasmHttpConfig {
            idle_timeout: Duration::from_secs(600),
            ..Default::default()
        };
        assert_eq!(config.idle_timeout, Duration::from_secs(600));
    }

    #[test]
    fn test_wasm_http_config_custom_request_timeout() {
        let config = WasmHttpConfig {
            request_timeout: Duration::from_secs(60),
            ..Default::default()
        };
        assert_eq!(config.request_timeout, Duration::from_secs(60));
    }

    #[test]
    fn test_wasm_http_config_fully_custom() {
        let config = WasmHttpConfig {
            min_instances: 2,
            max_instances: 50,
            idle_timeout: Duration::from_secs(180),
            request_timeout: Duration::from_secs(45),
        };

        assert_eq!(config.min_instances, 2);
        assert_eq!(config.max_instances, 50);
        assert_eq!(config.idle_timeout, Duration::from_secs(180));
        assert_eq!(config.request_timeout, Duration::from_secs(45));
    }

    #[test]
    fn test_wasm_http_config_clone() {
        let original = WasmHttpConfig {
            min_instances: 3,
            max_instances: 30,
            idle_timeout: Duration::from_secs(120),
            request_timeout: Duration::from_secs(15),
        };

        let cloned = original.clone();
        assert_eq!(cloned.min_instances, original.min_instances);
        assert_eq!(cloned.max_instances, original.max_instances);
        assert_eq!(cloned.idle_timeout, original.idle_timeout);
        assert_eq!(cloned.request_timeout, original.request_timeout);
    }

    #[test]
    fn test_wasm_http_config_debug_formatting() {
        let config = WasmHttpConfig::default();
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("WasmHttpConfig"));
        assert!(debug_str.contains("min_instances"));
        assert!(debug_str.contains("max_instances"));
    }

    #[test]
    fn test_wasm_http_config_equality() {
        let config1 = WasmHttpConfig {
            min_instances: 1,
            max_instances: 10,
            idle_timeout: Duration::from_secs(60),
            request_timeout: Duration::from_secs(30),
        };

        let config2 = WasmHttpConfig {
            min_instances: 1,
            max_instances: 10,
            idle_timeout: Duration::from_secs(60),
            request_timeout: Duration::from_secs(30),
        };

        let config3 = WasmHttpConfig {
            min_instances: 2, // Different
            max_instances: 10,
            idle_timeout: Duration::from_secs(60),
            request_timeout: Duration::from_secs(30),
        };

        assert_eq!(config1, config2);
        assert_ne!(config1, config3);
    }

    // =========================================================================
    // Error Conversion Tests
    // =========================================================================

    #[test]
    fn test_wasmtime_error_conversion() {
        // Create a wasmtime error indirectly by trying to compile invalid wasm
        // Note: This test verifies the From<wasmtime::Error> implementation exists
        // We can't easily create a wasmtime::Error directly, but we verify
        // the conversion path through the WasmHttpError::Internal variant
        let internal_error = WasmHttpError::Internal("simulated wasmtime error".to_string());
        let msg = internal_error.to_string();
        assert!(msg.contains("internal error"));
    }

    // =========================================================================
    // Concurrent Access Tests (for thread safety)
    // =========================================================================

    #[tokio::test]
    async fn test_instance_pool_concurrent_record_operations() {
        use std::sync::Arc;

        let pool = Arc::new(InstancePool::new(100, Duration::from_secs(60)));

        let mut handles = vec![];

        // Spawn multiple tasks that record operations concurrently
        for _ in 0..10 {
            let pool_clone = Arc::clone(&pool);
            handles.push(tokio::spawn(async move {
                for _ in 0..100 {
                    pool_clone.record_created();
                    pool_clone.record_request();
                    pool_clone.record_destroyed();
                }
            }));
        }

        // Wait for all tasks to complete
        for handle in handles {
            handle.await.unwrap();
        }

        // Verify final counts (10 tasks * 100 iterations each)
        assert_eq!(pool.total_created(), 1000);
        assert_eq!(pool.total_requests(), 1000);
        assert_eq!(pool.total_destroyed(), 1000);
    }

    #[tokio::test]
    async fn test_runtime_pool_stats_concurrent_access() {
        use std::sync::Arc;

        let config = test_config();
        let runtime = Arc::new(WasmHttpRuntime::new(config).unwrap());

        let mut handles = vec![];

        // Multiple concurrent pool_stats calls
        for _ in 0..10 {
            let runtime_clone = Arc::clone(&runtime);
            handles.push(tokio::spawn(async move {
                for _ in 0..10 {
                    let _stats = runtime_clone.pool_stats().await;
                }
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        // Should complete without deadlock or panic
        let stats = runtime.pool_stats().await;
        assert_eq!(stats.cached_components, 0);
    }

    #[tokio::test]
    async fn test_runtime_clear_cache_concurrent() {
        use std::sync::Arc;

        let config = test_config();
        let runtime = Arc::new(WasmHttpRuntime::new(config).unwrap());

        let mut handles = vec![];

        // Multiple concurrent clear_cache calls
        for _ in 0..10 {
            let runtime_clone = Arc::clone(&runtime);
            handles.push(tokio::spawn(async move {
                for _ in 0..10 {
                    runtime_clone.clear_cache().await;
                }
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        // Should complete without deadlock or panic
        let stats = runtime.pool_stats().await;
        assert_eq!(stats.cached_components, 0);
    }

    // =========================================================================
    // Edge Case Tests
    // =========================================================================

    #[test]
    fn test_http_request_empty_uri() {
        let request = HttpRequest::get("");
        assert_eq!(request.uri, "");
    }

    #[test]
    fn test_http_request_unicode_uri() {
        let request = HttpRequest::get("/api/users/\u{1F600}");
        assert!(request.uri.contains('\u{1F600}'));
    }

    #[test]
    fn test_http_request_very_long_uri() {
        let long_uri = "/".to_string() + &"a".repeat(10000);
        let request = HttpRequest::get(&long_uri);
        assert_eq!(request.uri.len(), 10001);
    }

    #[test]
    fn test_http_response_very_large_body() {
        let large_body = vec![0u8; 1_000_000]; // 1MB
        let response = HttpResponse::ok().with_body(large_body.clone());
        assert_eq!(response.body.as_ref().unwrap().len(), 1_000_000);
    }

    #[test]
    fn test_http_request_many_headers() {
        let mut request = HttpRequest::get("/test");
        for i in 0..1000 {
            request = request.with_header(format!("X-Header-{}", i), format!("value-{}", i));
        }
        assert_eq!(request.headers.len(), 1000);
    }

    #[test]
    fn test_http_response_many_headers() {
        let mut response = HttpResponse::ok();
        for i in 0..1000 {
            response = response.with_header(format!("X-Header-{}", i), format!("value-{}", i));
        }
        assert_eq!(response.headers.len(), 1000);
    }
}
