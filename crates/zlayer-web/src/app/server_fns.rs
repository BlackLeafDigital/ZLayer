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
    /// Version of ZLayer
    pub version: String,
    /// Number of running containers
    pub running_containers: u64,
    /// Number of available images
    pub available_images: u64,
    /// Number of configured overlay networks
    pub overlay_networks: u64,
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
pub async fn get_system_stats() -> Result<SystemStats, ServerFnError> {
    // TODO: Connect to actual ZLayer services when available
    Ok(SystemStats {
        version: env!("CARGO_PKG_VERSION").to_string(),
        running_containers: 0,
        available_images: 0,
        overlay_networks: 0,
    })
}

/// Validate a container specification YAML
#[server(prefix = "/api/leptos")]
pub async fn validate_spec(yaml_content: String) -> Result<String, ServerFnError> {
    // TODO: Use zlayer-spec to validate when available
    if yaml_content.trim().is_empty() {
        return Err(ServerFnError::new("Specification cannot be empty"));
    }

    // Basic YAML parsing check
    match serde_yaml::from_str::<serde_json::Value>(&yaml_content) {
        Ok(_) => Ok("Specification is valid YAML".to_string()),
        Err(e) => Err(ServerFnError::new(format!("Invalid YAML: {}", e))),
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
    _input: String,
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
                "Language '{}' is not yet supported. Currently only 'wat' (WebAssembly Text) is supported.",
                language
            )));
        }

        // Convert WAT to WASM bytes
        let wasm_bytes = wat::parse_str(&code).map_err(|e| {
            ServerFnError::new(format!("WAT parse error: {}", e))
        })?;

        // Execute the WASM module
        let result = execute_wasm_bytes(&wasm_bytes).await.map_err(|e| {
            ServerFnError::new(format!("Execution error: {}", e))
        })?;

        let execution_time_ms = start.elapsed().as_millis() as u64;

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
        let _ = (code, language, _input);
        Err(ServerFnError::new("WASM execution requires server-side rendering"))
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

        let result = execute_wasm_bytes(&wasm_bytes).await.map_err(|e| {
            ServerFnError::new(format!("Execution error: {}", e))
        })?;

        let execution_time_ms = start.elapsed().as_millis() as u64;

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
        Err(ServerFnError::new("WASM execution requires server-side rendering"))
    }
}

/// Internal function to execute WASM bytes
#[cfg(feature = "ssr")]
async fn execute_wasm_bytes(wasm_bytes: &[u8]) -> Result<(String, String, i32), String> {
    use wasmtime::*;
    use wasmtime_wasi::preview1::{self, WasiP1Ctx};
    use wasmtime_wasi::pipe::MemoryOutputPipe;
    use wasmtime_wasi::WasiCtxBuilder;

    let wasm_bytes = wasm_bytes.to_vec();

    // Run in a blocking task since wasmtime operations are CPU-bound
    tokio::task::spawn_blocking(move || {
        // Create engine with default config
        let engine = Engine::default();

        // Compile module
        let module = Module::new(&engine, &wasm_bytes)
            .map_err(|e| format!("Failed to compile module: {}", e))?;

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
        preview1::add_to_linker_sync(&mut linker, |ctx| ctx)
            .map_err(|e| format!("Failed to add WASI to linker: {}", e))?;

        // Instantiate the module
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| format!("Failed to instantiate module: {}", e))?;

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
                            return Err(format!("Execution error: {}", e));
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
    .map_err(|e| format!("Task join error: {}", e))?
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
        use wasmtime::*;

        let start = Instant::now();

        if code.trim().is_empty() {
            return Err(ServerFnError::new("Code cannot be empty"));
        }

        // Convert WAT to WASM bytes
        let wasm_bytes = wat::parse_str(&code).map_err(|e| {
            ServerFnError::new(format!("WAT parse error: {}", e))
        })?;

        // Clone values for use after the blocking task
        let func_name_for_info = function_name.clone();
        let args_for_info = args.clone();

        // Execute in blocking task
        let result = tokio::task::spawn_blocking(move || {
            let engine = Engine::default();
            let module = Module::new(&engine, &wasm_bytes)
                .map_err(|e| format!("Failed to compile module: {}", e))?;

            let mut store = Store::new(&engine, ());
            store.set_fuel(10_000_000).ok();

            let instance = Instance::new(&mut store, &module, &[])
                .map_err(|e| format!("Failed to instantiate module: {}", e))?;

            let func = instance
                .get_func(&mut store, &function_name)
                .ok_or_else(|| format!("Function '{}' not found", function_name))?;

            // Prepare arguments
            let func_type = func.ty(&store);
            let params: Vec<Val> = args
                .iter()
                .zip(func_type.params())
                .map(|(arg, param_type)| match param_type {
                    ValType::I32 => Val::I32(*arg as i32),
                    ValType::I64 => Val::I64(*arg),
                    ValType::F32 => Val::F32((*arg as f32).to_bits()),
                    ValType::F64 => Val::F64((*arg as f64).to_bits()),
                    _ => Val::I32(*arg as i32),
                })
                .collect();

            // Prepare results buffer
            let mut results: Vec<Val> = func_type
                .results()
                .map(|t| match t {
                    ValType::I32 => Val::I32(0),
                    ValType::I64 => Val::I64(0),
                    ValType::F32 => Val::F32(0),
                    ValType::F64 => Val::F64(0),
                    _ => Val::I32(0),
                })
                .collect();

            // Call the function
            func.call(&mut store, &params, &mut results)
                .map_err(|e| {
                    if e.to_string().contains("fuel") {
                        "Execution timed out (resource limit exceeded)".to_string()
                    } else {
                        format!("Execution error: {}", e)
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
        .map_err(|e| ServerFnError::new(format!("Task error: {}", e)))?
        .map_err(ServerFnError::new)?;

        let execution_time_ms = start.elapsed().as_millis() as u64;

        Ok(WasmExecutionResult {
            stdout: format!("Result: {}", result),
            stderr: String::new(),
            exit_code: 0,
            execution_time_ms,
            info: Some(format!("Called {}({:?})", func_name_for_info, args_for_info)),
        })
    }

    #[cfg(not(feature = "ssr"))]
    {
        let _ = (code, function_name, args);
        Err(ServerFnError::new("WASM execution requires server-side rendering"))
    }
}
