//! Server functions for Leptos SSR + Hydration
//!
//! These server functions provide data access from Leptos components.
//! The `#[server]` macro automatically:
//! - On the server (SSR): runs the function body with full access to backend services
//! - On the client (hydrate): generates an HTTP call to the server function endpoint
//!
//! This enables seamless data fetching that works identically whether
//! the component is rendered on the server or hydrated on the client.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

// ============================================================================
// Response Types
// ============================================================================

/// System statistics for the dashboard
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SystemStats {
    /// Version of `ZLayer`
    pub version: String,
    /// Number of running containers
    pub running_containers: u64,
    /// Number of available images
    pub available_images: u64,
    /// Number of configured overlay networks
    pub overlay_networks: u64,
    /// Total number of registered services
    pub total_services: u64,
    /// Number of healthy services
    pub healthy_services: u64,
    /// Total CPU usage percentage across all services
    pub total_cpu_percent: f64,
    /// Total memory usage in bytes
    pub total_memory_bytes: u64,
    /// Total memory limit in bytes
    pub total_memory_limit: u64,
    /// Server uptime in seconds
    pub uptime_seconds: u64,
}

/// Statistics for a single service
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceStats {
    /// Service name
    pub name: String,
    /// Number of running replicas
    pub replicas: u32,
    /// CPU usage percentage
    pub cpu_percent: f64,
    /// Memory usage percentage
    pub memory_percent: f64,
    /// Health status (e.g., "healthy", "unhealthy", "unknown")
    pub health_status: String,
}

/// Container specification summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerSpecSummary {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: String,
    pub created_at: String,
}

/// Result of WASM execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmExecutionResult {
    /// Standard output from the WASM module
    pub stdout: String,
    /// Standard error from the WASM module
    pub stderr: String,
    /// Exit code (0 = success)
    pub exit_code: i32,
    /// Execution time in milliseconds
    pub execution_time_ms: u64,
    /// Any additional info or warnings
    pub info: Option<String>,
}

// ============================================================================
// Server Functions
// ============================================================================

/// Get system statistics
#[server(prefix = "/api/leptos")]
#[allow(clippy::similar_names)]
pub async fn get_system_stats() -> Result<SystemStats, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        use std::sync::Arc;

        use crate::server::state::WebState;

        // Try to get WebState from Leptos context
        let state: Option<Arc<WebState>> = leptos::prelude::use_context();

        let mut stats = SystemStats {
            version: env!("CARGO_PKG_VERSION").to_string(),
            running_containers: 0,
            available_images: 0,
            overlay_networks: 0,
            total_services: 0,
            healthy_services: 0,
            total_cpu_percent: 0.0,
            total_memory_bytes: 0,
            total_memory_limit: 0,
            uptime_seconds: 0,
        };

        if let Some(web_state) = state {
            // Always set uptime
            stats.uptime_seconds = web_state.uptime().as_secs();

            // Get service stats if service_manager is available
            if let Some(service_manager) = web_state.service_manager() {
                let manager = service_manager.read().await;
                let services = manager.list_services().await;
                stats.total_services = services.len() as u64;

                // Count running containers (sum of all replica counts)
                let mut total_containers: u64 = 0;
                for service_name in &services {
                    if let Ok(count) = manager.service_replica_count(service_name).await {
                        total_containers += count as u64;
                    }
                }
                stats.running_containers = total_containers;

                // Count healthy services from health states
                let health_states = manager.health_states();
                let states = health_states.read().await;
                stats.healthy_services = states
                    .values()
                    .filter(|s| matches!(s, zlayer_agent::health::HealthState::Healthy))
                    .count() as u64;
            }

            // Get aggregated metrics if metrics_collector is available
            if let Some(metrics_collector) = web_state.metrics_collector() {
                // If we have service manager, collect metrics for all services
                if let Some(service_manager) = web_state.service_manager() {
                    let manager = service_manager.read().await;
                    let services = manager.list_services().await;

                    let mut total_cpu = 0.0;
                    let mut service_count_with_metrics = 0;

                    for service_name in &services {
                        if let Ok(aggregated) = metrics_collector.collect(service_name).await {
                            total_cpu += aggregated.avg_cpu_percent;
                            service_count_with_metrics += 1;
                        }
                    }

                    if service_count_with_metrics > 0 {
                        stats.total_cpu_percent = total_cpu / f64::from(service_count_with_metrics);
                    }
                }
            }
        }

        Ok(stats)
    }

    #[cfg(not(feature = "ssr"))]
    {
        std::future::ready(Ok(SystemStats {
            version: env!("CARGO_PKG_VERSION").to_string(),
            running_containers: 0,
            available_images: 0,
            overlay_networks: 0,
            total_services: 0,
            healthy_services: 0,
            total_cpu_percent: 0.0,
            total_memory_bytes: 0,
            total_memory_limit: 0,
            uptime_seconds: 0,
        }))
        .await
    }
}

/// Get statistics for all services
///
/// Returns a vector of `ServiceStats` containing metrics for each registered service.
/// If no services are registered or WebState is not available, returns an empty vector.
#[server(prefix = "/api/leptos")]
#[allow(clippy::similar_names, clippy::cast_possible_truncation)]
pub async fn get_all_service_stats() -> Result<Vec<ServiceStats>, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        use std::sync::Arc;

        use crate::server::state::WebState;

        // Try to get WebState from Leptos context
        let state: Option<Arc<WebState>> = leptos::prelude::use_context();

        let Some(web_state) = state else {
            return Ok(Vec::new());
        };

        let Some(service_manager) = web_state.service_manager() else {
            return Ok(Vec::new());
        };

        let manager = service_manager.read().await;
        let services = manager.list_services().await;

        if services.is_empty() {
            return Ok(Vec::new());
        }

        // Get health states
        let health_states = manager.health_states();
        let health_map = health_states.read().await;

        let mut result = Vec::with_capacity(services.len());

        for service_name in services {
            // Get replica count
            let replicas = manager
                .service_replica_count(&service_name)
                .await
                .unwrap_or(0) as u32;

            // Get health status
            let health_status = health_map
                .get(&service_name)
                .map_or("unknown", |s| match s {
                    zlayer_agent::health::HealthState::Healthy => "healthy",
                    zlayer_agent::health::HealthState::Unhealthy { .. } => "unhealthy",
                    zlayer_agent::health::HealthState::Unknown => "unknown",
                    zlayer_agent::health::HealthState::Checking => "checking",
                })
                .to_string();

            // Get metrics if available
            let (cpu_percent, memory_percent) =
                if let Some(metrics_collector) = web_state.metrics_collector() {
                    metrics_collector
                        .collect(&service_name)
                        .await
                        .map(|m| (m.avg_cpu_percent, m.avg_memory_percent))
                        .unwrap_or((0.0, 0.0))
                } else {
                    (0.0, 0.0)
                };

            result.push(ServiceStats {
                name: service_name,
                replicas,
                cpu_percent,
                memory_percent,
                health_status,
            });
        }

        Ok(result)
    }

    #[cfg(not(feature = "ssr"))]
    {
        std::future::ready(Ok(Vec::new())).await
    }
}

/// Validate a container specification YAML
#[server(prefix = "/api/leptos")]
pub async fn validate_spec(yaml_content: String) -> Result<String, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        if yaml_content.trim().is_empty() {
            return std::future::ready(Err(ServerFnError::new("Specification cannot be empty")))
                .await;
        }

        // Use zlayer-spec for full validation
        std::future::ready(match zlayer_spec::from_yaml_str(&yaml_content) {
            Ok(spec) => {
                let service_count = spec.services.len();
                let service_names: Vec<_> = spec.services.keys().collect();
                Ok(format!(
                    "Valid ZLayer spec: deployment '{}' with {} service(s): {:?}",
                    spec.deployment, service_count, service_names
                ))
            }
            Err(e) => Err(ServerFnError::new(format!("Validation error: {e}"))),
        })
        .await
    }

    #[cfg(not(feature = "ssr"))]
    {
        let _ = yaml_content;
        std::future::ready(Err(ServerFnError::new(
            "Validation requires server-side rendering",
        )))
        .await
    }
}

/// Execute WebAssembly Text (WAT) code
///
/// This server function accepts WAT (WebAssembly Text) format code,
/// compiles it to WASM bytes, and executes it using wasmtime.
#[server(prefix = "/api/leptos")]
pub async fn execute_wasm(
    code: String,
    language: String,
) -> Result<WasmExecutionResult, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        use std::time::Instant;

        let start = Instant::now();

        // Validate input
        if code.trim().is_empty() {
            return Err(ServerFnError::new("Code cannot be empty"));
        }

        // For now, we only support WAT format
        if language != "wat" {
            return Err(ServerFnError::new(format!(
                "Language '{language}' is not yet supported. Currently only 'wat' (WebAssembly Text) is supported."
            )));
        }

        // Convert WAT to WASM bytes
        let wasm_bytes = wat::parse_str(&code)
            .map_err(|e| ServerFnError::new(format!("WAT parse error: {e}")))?;

        // Execute the WASM module
        let result = execute_wasm_bytes(&wasm_bytes)
            .await
            .map_err(|e| ServerFnError::new(format!("Execution error: {e}")))?;

        let elapsed_millis = start.elapsed().as_millis();
        let execution_time_ms = u64::try_from(elapsed_millis).unwrap_or(u64::MAX);

        Ok(WasmExecutionResult {
            stdout: result.0,
            stderr: result.1,
            exit_code: result.2,
            execution_time_ms,
            info: Some(format!("Compiled {} bytes of WASM", wasm_bytes.len())),
        })
    }

    #[cfg(not(feature = "ssr"))]
    {
        let _ = (code, language);
        Err(ServerFnError::new(
            "WASM execution requires server-side rendering",
        ))
    }
}

/// Execute pre-compiled WASM bytes directly
#[server(prefix = "/api/leptos")]
pub async fn execute_wasm_bytes_api(
    wasm_bytes: Vec<u8>,
) -> Result<WasmExecutionResult, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        use std::time::Instant;

        let start = Instant::now();

        if wasm_bytes.is_empty() {
            return Err(ServerFnError::new("WASM bytes cannot be empty"));
        }

        let result = execute_wasm_bytes(&wasm_bytes)
            .await
            .map_err(|e| ServerFnError::new(format!("Execution error: {e}")))?;

        let elapsed_millis = start.elapsed().as_millis();
        let execution_time_ms = u64::try_from(elapsed_millis).unwrap_or(u64::MAX);

        Ok(WasmExecutionResult {
            stdout: result.0,
            stderr: result.1,
            exit_code: result.2,
            execution_time_ms,
            info: None,
        })
    }

    #[cfg(not(feature = "ssr"))]
    {
        let _ = wasm_bytes;
        Err(ServerFnError::new(
            "WASM execution requires server-side rendering",
        ))
    }
}

/// Internal function to execute WASM bytes
#[cfg(feature = "ssr")]
async fn execute_wasm_bytes(wasm_bytes: &[u8]) -> Result<(String, String, i32), String> {
    use wasmtime::{Engine, Linker, Module, Store};
    use wasmtime_wasi::p1::{self, WasiP1Ctx};
    use wasmtime_wasi::p2::pipe::MemoryOutputPipe;
    use wasmtime_wasi::WasiCtxBuilder;

    let wasm_bytes = wasm_bytes.to_vec();

    // Run in a blocking task since wasmtime operations are CPU-bound
    tokio::task::spawn_blocking(move || {
        // Create engine with default config
        let engine = Engine::default();

        // Compile module
        let module = Module::new(&engine, &wasm_bytes)
            .map_err(|e| format!("Failed to compile module: {e}"))?;

        // Create pipes to capture stdout/stderr
        let stdout_pipe = MemoryOutputPipe::new(4096);
        let stderr_pipe = MemoryOutputPipe::new(4096);

        // Build WASI context
        let mut wasi_builder = WasiCtxBuilder::new();
        wasi_builder.stdout(stdout_pipe.clone());
        wasi_builder.stderr(stderr_pipe.clone());
        wasi_builder.args(&["wasm-playground"]);

        let wasi_ctx = wasi_builder.build_p1();

        // Create store
        let mut store = Store::new(&engine, wasi_ctx);

        // Set a reasonable timeout (5 seconds worth of fuel)
        store.set_fuel(10_000_000).ok();

        // Create linker and add WASI
        let mut linker: Linker<WasiP1Ctx> = Linker::new(&engine);
        p1::add_to_linker_sync(&mut linker, |ctx| ctx)
            .map_err(|e| format!("Failed to add WASI to linker: {e}"))?;

        // Instantiate the module
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| format!("Failed to instantiate module: {e}"))?;

        // Look for entry point
        let start_func = instance
            .get_func(&mut store, "_start")
            .or_else(|| instance.get_func(&mut store, "main"));

        let exit_code = match start_func {
            Some(func) => {
                match func.call(&mut store, &[], &mut []) {
                    Ok(()) => 0,
                    Err(e) => {
                        // Check for WASI exit
                        if let Some(exit) = e.downcast_ref::<wasmtime_wasi::I32Exit>() {
                            exit.0
                        } else {
                            // Check if it's an out-of-fuel error
                            if e.to_string().contains("fuel") {
                                return Err("Execution timed out (resource limit exceeded)".to_string());
                            }
                            return Err(format!("Execution error: {e}"));
                        }
                    }
                }
            }
            None => {
                // No _start or main - check for exported functions to run
                // For modules that just export functions like `fib`, we can't run them directly
                // without knowing the parameters
                return Err("No _start or main function found. For library modules, use the 'Run Function' feature.".to_string());
            }
        };

        // Collect output
        let stdout_bytes: Vec<u8> = stdout_pipe.try_into_inner()
            .ok_or_else(|| "Failed to get stdout".to_string())?
            .into();
        let stderr_bytes: Vec<u8> = stderr_pipe.try_into_inner()
            .ok_or_else(|| "Failed to get stderr".to_string())?
            .into();

        let stdout = String::from_utf8_lossy(&stdout_bytes).to_string();
        let stderr = String::from_utf8_lossy(&stderr_bytes).to_string();

        Ok((stdout, stderr, exit_code))
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

/// Convert an i64 argument to a WASM Val type with proper validation.
///
/// For I32: Returns error if value doesn't fit in i32 range.
/// For F32/F64: Converts the integer to float. Note that large integers may lose
/// precision when converted to floating point (f32 has 23-bit mantissa, f64 has 52-bit).
/// This is the expected behavior when passing integer arguments to float parameters.
#[cfg(feature = "ssr")]
fn convert_i64_to_wasm_val(
    arg: i64,
    param_type: &wasmtime::ValType,
    arg_index: usize,
) -> Result<wasmtime::Val, String> {
    use wasmtime::{Val, ValType};

    match *param_type {
        ValType::I32 => {
            let val = i32::try_from(arg).map_err(|_| {
                format!(
                    "Argument {} value {} is out of range for i32 (valid range: {} to {})",
                    arg_index,
                    arg,
                    i32::MIN,
                    i32::MAX
                )
            })?;
            Ok(Val::I32(val))
        }
        ValType::I64 => Ok(Val::I64(arg)),
        ValType::F32 => {
            // Convert i64 to f32. Precision loss is expected for large values.
            // We use an explicit conversion to acknowledge the precision change.
            #[allow(clippy::cast_precision_loss)]
            let float_val = arg as f32;
            Ok(Val::F32(float_val.to_bits()))
        }
        ValType::F64 => {
            // Convert i64 to f64. Precision loss is expected for values outside
            // the exact representable range (integers > 2^53).
            #[allow(clippy::cast_precision_loss)]
            let float_val = arg as f64;
            Ok(Val::F64(float_val.to_bits()))
        }
        _ => {
            // For other types (V128, FuncRef, ExternRef), default to I32 with validation
            let val = i32::try_from(arg).map_err(|_| {
                format!(
                    "Argument {} value {} is out of range for i32 (valid range: {} to {})",
                    arg_index,
                    arg,
                    i32::MIN,
                    i32::MAX
                )
            })?;
            Ok(Val::I32(val))
        }
    }
}

/// Execute a specific exported function from a WASM module
#[server(prefix = "/api/leptos")]
pub async fn execute_wasm_function(
    code: String,
    function_name: String,
    args: Vec<i64>,
) -> Result<WasmExecutionResult, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        use std::time::Instant;
        use wasmtime::{Engine, Instance, Module, Store, Val, ValType};

        let start = Instant::now();

        if code.trim().is_empty() {
            return Err(ServerFnError::new("Code cannot be empty"));
        }

        // Convert WAT to WASM bytes
        let wasm_bytes = wat::parse_str(&code)
            .map_err(|e| ServerFnError::new(format!("WAT parse error: {e}")))?;

        // Clone values for use after the blocking task
        let func_name_for_info = function_name.clone();
        let args_for_info = args.clone();

        // Execute in blocking task
        let result = tokio::task::spawn_blocking(move || {
            let engine = Engine::default();
            let module = Module::new(&engine, &wasm_bytes)
                .map_err(|e| format!("Failed to compile module: {e}"))?;

            let mut store = Store::new(&engine, ());
            store.set_fuel(10_000_000).ok();

            let instance = Instance::new(&mut store, &module, &[])
                .map_err(|e| format!("Failed to instantiate module: {e}"))?;

            let func = instance
                .get_func(&mut store, &function_name)
                .ok_or_else(|| format!("Function '{function_name}' not found"))?;

            // Prepare arguments with proper type conversion and validation
            let func_type = func.ty(&store);
            let params: Vec<Val> = args
                .iter()
                .zip(func_type.params())
                .enumerate()
                .map(|(idx, (arg, param_type))| convert_i64_to_wasm_val(*arg, &param_type, idx))
                .collect::<Result<Vec<_>, _>>()?;

            // Prepare results buffer - initialize with zero values
            let mut results: Vec<Val> = func_type
                .results()
                .map(|t| match t {
                    ValType::I64 => Val::I64(0),
                    ValType::F32 => Val::F32(0),
                    ValType::F64 => Val::F64(0),
                    // I32, V128, FuncRef, ExternRef all use I32 as default
                    _ => Val::I32(0),
                })
                .collect();

            // Call the function
            func.call(&mut store, &params, &mut results).map_err(|e| {
                if e.to_string().contains("fuel") {
                    "Execution timed out (resource limit exceeded)".to_string()
                } else {
                    format!("Execution error: {e}")
                }
            })?;

            // Format results
            let result_str = results
                .iter()
                .map(|v| match v {
                    Val::I32(n) => n.to_string(),
                    Val::I64(n) => n.to_string(),
                    Val::F32(bits) => f32::from_bits(*bits).to_string(),
                    Val::F64(bits) => f64::from_bits(*bits).to_string(),
                    _ => "?".to_string(),
                })
                .collect::<Vec<_>>()
                .join(", ");

            Ok::<_, String>(result_str)
        })
        .await
        .map_err(|e| ServerFnError::new(format!("Task error: {e}")))?
        .map_err(ServerFnError::new)?;

        let elapsed_millis = start.elapsed().as_millis();
        let execution_time_ms = u64::try_from(elapsed_millis).unwrap_or(u64::MAX);

        Ok(WasmExecutionResult {
            stdout: format!("Result: {result}"),
            stderr: String::new(),
            exit_code: 0,
            execution_time_ms,
            info: Some(format!("Called {func_name_for_info}({args_for_info:?})")),
        })
    }

    #[cfg(not(feature = "ssr"))]
    {
        let _ = (code, function_name, args);
        Err(ServerFnError::new(
            "WASM execution requires server-side rendering",
        ))
    }
}
