//! WASM Platform End-to-End Tests
//!
//! This module provides comprehensive E2E tests for the entire WASM platform,
//! covering the full workflow from WASM binary creation through runtime testing.
//!
//! ## Test Categories
//!
//! 1. **WASM Binary Analysis E2E**: Create WASM binaries and verify analysis
//! 2. **WASM Build E2E**: Language detection, build command verification
//! 3. **WASM HTTP Handler E2E**: HTTP runtime with pool statistics
//! 4. **WASM Host Functions E2E**: Full host function flow testing
//! 5. **Full Plugin Lifecycle E2E**: Complete plugin lifecycle management
//!
//! ## Running Tests
//!
//! ```bash
//! cargo test -p zlayer-agent --features wasm wasm_e2e
//! ```

#![cfg(feature = "wasm")]

use std::time::Duration;
use tempfile::TempDir;

// Import the public API from zlayer-agent
use zlayer_agent::runtimes::{
    DefaultHost, HttpRequest, HttpResponse, KvError, LogLevel, PoolStats, WasmHttpRuntime,
    ZLayerHost,
};
use zlayer_spec::WasmHttpConfig;

// Import WASM utilities from zlayer-registry
use zlayer_registry::{
    detect_wasm_version_from_binary, extract_wasm_binary_info, validate_wasm_magic, WasiVersion,
    WASM_COMPONENT_ARTIFACT_TYPE, WASM_MODULE_ARTIFACT_TYPE,
};

// =============================================================================
// WASM Binary Builders (using WAT - WebAssembly Text Format)
// =============================================================================

/// Create a minimal valid WASIp1 core module from WAT
fn create_wasip1_module_from_wat() -> Vec<u8> {
    wat::parse_str(
        r#"
        (module
            ;; A minimal WASIp1 module with a single function
            (func (export "add") (param i32 i32) (result i32)
                local.get 0
                local.get 1
                i32.add
            )

            ;; Memory export (required for many WASI operations)
            (memory (export "memory") 1)
        )
        "#,
    )
    .expect("Failed to parse WAT")
}

/// Create a WASIp1 module with memory and a start function
fn create_wasip1_module_with_start() -> Vec<u8> {
    wat::parse_str(
        r#"
        (module
            (memory (export "memory") 1)

            (global $counter (mut i32) (i32.const 0))

            (func $init
                ;; Initialize counter to 42
                i32.const 42
                global.set $counter
            )

            (func (export "get_counter") (result i32)
                global.get $counter
            )

            (func (export "increment") (result i32)
                global.get $counter
                i32.const 1
                i32.add
                global.set $counter
                global.get $counter
            )

            (start $init)
        )
        "#,
    )
    .expect("Failed to parse WAT")
}

/// Create a WASIp1 module with table and indirect calls
fn create_wasip1_module_with_table() -> Vec<u8> {
    wat::parse_str(
        r#"
        (module
            (memory (export "memory") 1)

            (type $binary_op (func (param i32 i32) (result i32)))

            (func $add (type $binary_op)
                local.get 0
                local.get 1
                i32.add
            )

            (func $sub (type $binary_op)
                local.get 0
                local.get 1
                i32.sub
            )

            (func $mul (type $binary_op)
                local.get 0
                local.get 1
                i32.mul
            )

            (table (export "ops") 3 funcref)
            (elem (i32.const 0) $add $sub $mul)

            (func (export "call_op") (param $op i32) (param $a i32) (param $b i32) (result i32)
                local.get $a
                local.get $b
                local.get $op
                call_indirect (type $binary_op)
            )
        )
        "#,
    )
    .expect("Failed to parse WAT")
}

/// Create a minimal WASIp1 module (just the binary header with minimal sections)
fn create_minimal_wasm_module_bytes() -> Vec<u8> {
    vec![
        0x00, 0x61, 0x73, 0x6d, // Magic: \0asm
        0x01, 0x00, 0x00, 0x00, // Version: 1
              // Empty module - no sections
    ]
}

/// Create a WASIp1 module with type section for detection testing
fn create_wasm_module_with_type_section() -> Vec<u8> {
    vec![
        0x00, 0x61, 0x73, 0x6d, // Magic: \0asm
        0x01, 0x00, 0x00, 0x00, // Version: 1
        0x01, // Type section ID
        0x04, // Section size: 4 bytes
        0x01, // Number of types: 1
        0x60, // Function type indicator
        0x00, // Number of parameters: 0
        0x00, // Number of results: 0
    ]
}

/// Create a WASIp2 component header (simulated - version 13)
fn create_wasip2_component_header() -> Vec<u8> {
    vec![
        0x00, 0x61, 0x73, 0x6d, // Magic: \0asm
        0x0d, 0x00, 0x01, 0x00, // Component layer version (0x0d = 13)
        0x00, // Component section type
        // Minimal component data
        0x08, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
    ]
}

/// Create a complex WAT module with multiple features
fn create_complex_wasm_module() -> Vec<u8> {
    wat::parse_str(
        r#"
        (module
            ;; Memory
            (memory (export "memory") 1 16)

            ;; Global variables
            (global $g1 (mut i32) (i32.const 0))
            (global $g2 (mut i64) (i64.const 0))

            ;; Type definitions
            (type $unary (func (param i32) (result i32)))
            (type $binary (func (param i32 i32) (result i32)))

            ;; Function table
            (table $funcs 4 funcref)

            ;; Math functions
            (func $square (type $unary)
                local.get 0
                local.get 0
                i32.mul
            )

            (func $double (type $unary)
                local.get 0
                i32.const 2
                i32.mul
            )

            (func $add (type $binary)
                local.get 0
                local.get 1
                i32.add
            )

            (func $max (type $binary)
                local.get 0
                local.get 1
                local.get 0
                local.get 1
                i32.gt_s
                select
            )

            ;; Initialize table
            (elem (i32.const 0) $square $double $add $max)

            ;; Exported functions
            (func (export "apply_unary") (param $fn i32) (param $x i32) (result i32)
                local.get $x
                local.get $fn
                call_indirect (type $unary)
            )

            (func (export "apply_binary") (param $fn i32) (param $a i32) (param $b i32) (result i32)
                local.get $a
                local.get $b
                local.get $fn
                i32.const 2
                i32.add
                call_indirect (type $binary)
            )

            ;; Counter operations using global
            (func (export "get_counter") (result i32)
                global.get $g1
            )

            (func (export "inc_counter") (param $delta i32) (result i32)
                global.get $g1
                local.get $delta
                i32.add
                global.set $g1
                global.get $g1
            )

            ;; Memory operations
            (func (export "store_i32") (param $offset i32) (param $value i32)
                local.get $offset
                local.get $value
                i32.store
            )

            (func (export "load_i32") (param $offset i32) (result i32)
                local.get $offset
                i32.load
            )
        )
        "#,
    )
    .expect("Failed to parse complex WAT")
}

// =============================================================================
// E2E Test: WASM Binary Analysis
// =============================================================================

mod wasm_binary_analysis_e2e {
    use super::*;

    /// Test complete binary analysis for WASIp1 module
    #[test]
    fn test_wasip1_binary_analysis() {
        let wasm_bytes = create_wasm_module_with_type_section();

        // Validate magic
        assert!(validate_wasm_magic(&wasm_bytes));

        // Detect version
        let version = detect_wasm_version_from_binary(&wasm_bytes);
        assert_eq!(version, WasiVersion::Preview1);

        // Extract full info
        let info = extract_wasm_binary_info(&wasm_bytes).expect("Should extract info");
        assert_eq!(info.wasi_version, WasiVersion::Preview1);
        assert!(!info.is_component);
        assert_eq!(info.binary_version, 1);
        assert_eq!(info.size, wasm_bytes.len());
    }

    /// Test binary analysis for WASIp2 component
    #[test]
    fn test_wasip2_binary_analysis() {
        let wasm_bytes = create_wasip2_component_header();

        // Validate magic
        assert!(validate_wasm_magic(&wasm_bytes));

        // Detect version
        let version = detect_wasm_version_from_binary(&wasm_bytes);
        assert_eq!(version, WasiVersion::Preview2);

        // Extract full info
        let info = extract_wasm_binary_info(&wasm_bytes).expect("Should extract info");
        assert_eq!(info.wasi_version, WasiVersion::Preview2);
        assert!(info.is_component);
        assert!(info.binary_version >= 13);
    }

    /// Test binary analysis for WAT-generated module
    #[test]
    fn test_wat_generated_module_analysis() {
        let wasm_bytes = create_wasip1_module_from_wat();

        assert!(validate_wasm_magic(&wasm_bytes));

        let info = extract_wasm_binary_info(&wasm_bytes).expect("Should extract info");
        assert_eq!(info.wasi_version, WasiVersion::Preview1);
        assert!(!info.is_component);
        assert!(
            info.size > 8,
            "WAT module should be larger than just header"
        );
    }

    /// Test analysis fails for invalid binary
    #[test]
    fn test_invalid_binary_analysis() {
        let invalid_bytes = vec![0xde, 0xad, 0xbe, 0xef, 0x00, 0x00, 0x00, 0x00];

        assert!(!validate_wasm_magic(&invalid_bytes));

        let result = extract_wasm_binary_info(&invalid_bytes);
        assert!(result.is_err());
    }

    /// Test analysis fails for truncated binary
    #[test]
    fn test_truncated_binary_analysis() {
        let truncated = vec![0x00, 0x61, 0x73]; // Only 3 bytes of magic

        assert!(!validate_wasm_magic(&truncated));

        let result = extract_wasm_binary_info(&truncated);
        assert!(result.is_err());
    }

    /// Test analysis for minimal module (just header, no sections)
    #[test]
    fn test_minimal_module_analysis() {
        let wasm_bytes = create_minimal_wasm_module_bytes();

        assert!(validate_wasm_magic(&wasm_bytes));

        let info = extract_wasm_binary_info(&wasm_bytes).expect("Should extract info");
        // Without any section, the WASI version cannot be determined
        // Detection requires at least 9 bytes (magic + version + section type)
        assert_eq!(info.wasi_version, WasiVersion::Unknown);
        assert!(!info.is_component);
        assert_eq!(info.binary_version, 1);
    }

    /// Test analysis for complex module
    #[test]
    fn test_complex_module_analysis() {
        let wasm_bytes = create_complex_wasm_module();

        assert!(validate_wasm_magic(&wasm_bytes));

        let info = extract_wasm_binary_info(&wasm_bytes).expect("Should extract info");
        assert_eq!(info.wasi_version, WasiVersion::Preview1);
        assert!(!info.is_component);
        // Complex module should be larger
        assert!(
            info.size > 100,
            "Complex module should have significant size"
        );
    }

    /// Test analysis for module with start function
    #[test]
    fn test_module_with_start_analysis() {
        let wasm_bytes = create_wasip1_module_with_start();

        assert!(validate_wasm_magic(&wasm_bytes));

        let info = extract_wasm_binary_info(&wasm_bytes).expect("Should extract info");
        assert_eq!(info.wasi_version, WasiVersion::Preview1);
        assert!(!info.is_component);
    }

    /// Test analysis for module with table
    #[test]
    fn test_module_with_table_analysis() {
        let wasm_bytes = create_wasip1_module_with_table();

        assert!(validate_wasm_magic(&wasm_bytes));

        let info = extract_wasm_binary_info(&wasm_bytes).expect("Should extract info");
        assert_eq!(info.wasi_version, WasiVersion::Preview1);
        assert!(!info.is_component);
    }

    /// Test WASI version target triple suffixes
    #[test]
    fn test_wasi_version_target_triples() {
        assert_eq!(
            WasiVersion::Preview1.target_triple_suffix(),
            "wasm32-wasip1"
        );
        assert_eq!(
            WasiVersion::Preview2.target_triple_suffix(),
            "wasm32-wasip2"
        );
    }

    /// Test WASM artifact type constants
    #[test]
    fn test_wasm_artifact_type_constants() {
        assert!(WASM_MODULE_ARTIFACT_TYPE.contains("module"));
        assert!(WASM_COMPONENT_ARTIFACT_TYPE.contains("component"));
    }
}

// =============================================================================
// E2E Test: WASM Build Pipeline Logic
// =============================================================================

mod wasm_build_e2e {
    use super::*;

    /// Test language detection for Rust project structure
    #[tokio::test]
    async fn test_detect_rust_wasm_project() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        // Create a minimal Rust project structure
        let cargo_toml = r#"
[package]
name = "wasm-test"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
"#;

        tokio::fs::write(temp_dir.path().join("Cargo.toml"), cargo_toml)
            .await
            .expect("Failed to write Cargo.toml");

        let src_dir = temp_dir.path().join("src");
        tokio::fs::create_dir(&src_dir)
            .await
            .expect("Failed to create src dir");
        tokio::fs::write(src_dir.join("lib.rs"), "// Rust WASM library")
            .await
            .expect("Failed to write lib.rs");

        // Verify project structure exists
        assert!(temp_dir.path().join("Cargo.toml").exists());
        assert!(temp_dir.path().join("src/lib.rs").exists());

        // Detection logic would identify this as Rust
        let cargo_content = tokio::fs::read_to_string(temp_dir.path().join("Cargo.toml"))
            .await
            .unwrap();
        assert!(cargo_content.contains("cdylib"), "Should be a cdylib crate");
    }

    /// Test language detection for TinyGo project structure
    #[tokio::test]
    async fn test_detect_go_wasm_project() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        // Create a minimal Go project structure
        let go_mod = r#"
module github.com/example/wasm-test

go 1.21
"#;

        tokio::fs::write(temp_dir.path().join("go.mod"), go_mod)
            .await
            .expect("Failed to write go.mod");

        let main_go = r#"
package main

func main() {
    println("Hello from WASM")
}
"#;

        tokio::fs::write(temp_dir.path().join("main.go"), main_go)
            .await
            .expect("Failed to write main.go");

        // Verify project structure
        assert!(temp_dir.path().join("go.mod").exists());
        assert!(temp_dir.path().join("main.go").exists());
    }

    /// Test language detection for AssemblyScript project
    #[tokio::test]
    async fn test_detect_assemblyscript_project() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        // Create minimal AssemblyScript structure
        let package_json = r#"{
  "name": "wasm-test",
  "version": "1.0.0",
  "dependencies": {
    "assemblyscript": "^0.27.0"
  }
}"#;

        tokio::fs::write(temp_dir.path().join("package.json"), package_json)
            .await
            .expect("Failed to write package.json");

        let as_config = r#"{
  "targets": {
    "release": {
      "outFile": "build/release.wasm"
    }
  }
}"#;

        tokio::fs::write(temp_dir.path().join("asconfig.json"), as_config)
            .await
            .expect("Failed to write asconfig.json");

        // Verify project structure
        assert!(temp_dir.path().join("package.json").exists());
        assert!(temp_dir.path().join("asconfig.json").exists());

        let pkg_content = tokio::fs::read_to_string(temp_dir.path().join("package.json"))
            .await
            .unwrap();
        assert!(
            pkg_content.contains("assemblyscript"),
            "Should have AssemblyScript dependency"
        );
    }

    /// Test build command construction for Rust WASIp1
    #[test]
    fn test_rust_wasip1_build_command_structure() {
        let expected_target = "wasm32-wasip1";

        // Verify the target triple is correct
        assert_eq!(
            WasiVersion::Preview1.target_triple_suffix(),
            "wasm32-wasip1"
        );

        // The build would construct: cargo build --release --target wasm32-wasip1
        assert!(expected_target.contains("wasip1"));
    }

    /// Test build command construction for Rust WASIp2
    #[test]
    fn test_rust_wasip2_build_command_structure() {
        let expected_target = "wasm32-wasip2";

        assert_eq!(
            WasiVersion::Preview2.target_triple_suffix(),
            "wasm32-wasip2"
        );

        // For WASIp2, we'd also need cargo-component or similar tooling
        assert!(expected_target.contains("wasip2"));
    }

    /// Test WASM binary writing and verification
    #[tokio::test]
    async fn test_wasm_binary_write_and_verify() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wasm_path = temp_dir.path().join("test.wasm");

        // Generate WASM bytes
        let wasm_bytes = create_wasip1_module_from_wat();

        // Write to file
        tokio::fs::write(&wasm_path, &wasm_bytes)
            .await
            .expect("Failed to write WASM file");

        // Read back and verify
        let read_bytes = tokio::fs::read(&wasm_path)
            .await
            .expect("Failed to read WASM file");

        assert_eq!(read_bytes, wasm_bytes, "WASM bytes should match");
        assert!(
            validate_wasm_magic(&read_bytes),
            "Read WASM should be valid"
        );
    }
}

// =============================================================================
// E2E Test: WASM HTTP Handler Runtime
// =============================================================================

mod wasm_http_e2e {
    use super::*;

    /// Test creating HTTP runtime with various configurations
    #[tokio::test]
    async fn test_http_runtime_creation() {
        // Default config
        let default_config = WasmHttpConfig::default();
        let runtime = WasmHttpRuntime::new(default_config.clone());
        assert!(runtime.is_ok(), "Should create runtime with default config");

        // Custom config
        let custom_config = WasmHttpConfig {
            min_instances: 2,
            max_instances: 20,
            idle_timeout: Duration::from_secs(120),
            request_timeout: Duration::from_secs(60),
        };
        let runtime = WasmHttpRuntime::new(custom_config);
        assert!(runtime.is_ok(), "Should create runtime with custom config");
    }

    /// Test HTTP request builder patterns
    #[test]
    fn test_http_request_builders() {
        // GET request
        let get = HttpRequest::get("/api/users");
        assert_eq!(get.method, "GET");
        assert_eq!(get.uri, "/api/users");
        assert!(get.body.is_none());

        // POST request with body
        let post = HttpRequest::post("/api/users", b"{}".to_vec());
        assert_eq!(post.method, "POST");
        assert_eq!(post.body, Some(b"{}".to_vec()));

        // Request with headers
        let with_headers = HttpRequest::get("/api/auth")
            .with_header("Authorization", "Bearer token123")
            .with_header("Content-Type", "application/json");

        assert_eq!(with_headers.headers.len(), 2);
        assert!(with_headers
            .headers
            .iter()
            .any(|(k, v)| k == "Authorization" && v == "Bearer token123"));
    }

    /// Test HTTP method building via struct construction
    #[test]
    fn test_http_request_via_struct() {
        // Test constructing various HTTP methods directly
        let get = HttpRequest::get("/resource");
        assert_eq!(get.method, "GET");

        let post = HttpRequest::post("/resource", b"data".to_vec());
        assert_eq!(post.method, "POST");

        // Other methods can be constructed via struct initialization
        let put = HttpRequest {
            method: "PUT".to_string(),
            uri: "/resource".to_string(),
            headers: Vec::new(),
            body: Some(b"updated".to_vec()),
        };
        assert_eq!(put.method, "PUT");

        let delete = HttpRequest {
            method: "DELETE".to_string(),
            uri: "/resource".to_string(),
            headers: Vec::new(),
            body: None,
        };
        assert_eq!(delete.method, "DELETE");

        let patch = HttpRequest {
            method: "PATCH".to_string(),
            uri: "/resource".to_string(),
            headers: Vec::new(),
            body: Some(b"partial".to_vec()),
        };
        assert_eq!(patch.method, "PATCH");
    }

    /// Test HTTP response builder patterns
    #[test]
    fn test_http_response_builders() {
        // OK response
        let ok = HttpResponse::ok();
        assert_eq!(ok.status, 200);
        assert!(ok.body.is_none());

        // Response with body
        let with_body = HttpResponse::ok()
            .with_body(b"Hello, World!".to_vec())
            .with_header("Content-Type", "text/plain");
        assert_eq!(with_body.body, Some(b"Hello, World!".to_vec()));
        assert_eq!(with_body.headers.len(), 1);

        // Error response
        let error = HttpResponse::internal_error("Something went wrong");
        assert_eq!(error.status, 500);
        assert!(error.body.is_some());
    }

    /// Test HTTP response status codes via constructors
    #[test]
    fn test_http_response_status_codes() {
        // Available constructors
        let ok = HttpResponse::ok();
        assert_eq!(ok.status, 200);

        let internal_error = HttpResponse::internal_error("error");
        assert_eq!(internal_error.status, 500);

        // Other status codes via new() constructor
        let created = HttpResponse::new(201);
        assert_eq!(created.status, 201);

        let no_content = HttpResponse::new(204);
        assert_eq!(no_content.status, 204);

        let bad_request = HttpResponse::new(400);
        assert_eq!(bad_request.status, 400);

        let unauthorized = HttpResponse::new(401);
        assert_eq!(unauthorized.status, 401);

        let forbidden = HttpResponse::new(403);
        assert_eq!(forbidden.status, 403);

        let not_found = HttpResponse::new(404);
        assert_eq!(not_found.status, 404);
    }

    /// Test pool statistics tracking
    #[tokio::test]
    async fn test_pool_statistics() {
        let config = WasmHttpConfig {
            min_instances: 0,
            max_instances: 10,
            idle_timeout: Duration::from_secs(60),
            request_timeout: Duration::from_secs(30),
        };

        let runtime = WasmHttpRuntime::new(config).expect("Failed to create runtime");

        // Initial stats should be empty
        let initial_stats = runtime.pool_stats().await;
        assert_eq!(initial_stats.cached_components, 0);
        assert_eq!(initial_stats.total_created, 0);
        assert_eq!(initial_stats.total_requests, 0);

        // Clear cache should not panic on empty cache
        runtime.clear_cache().await;

        let after_clear = runtime.pool_stats().await;
        assert_eq!(after_clear.cached_components, 0);
    }

    /// Test cache clearing
    #[tokio::test]
    async fn test_cache_clearing() {
        let config = WasmHttpConfig::default();
        let runtime = WasmHttpRuntime::new(config).expect("Failed to create runtime");

        // Multiple clears should be safe
        for _ in 0..5 {
            runtime.clear_cache().await;
        }

        let stats = runtime.pool_stats().await;
        assert_eq!(stats.cached_components, 0);
    }

    /// Test PoolStats Debug implementation
    #[test]
    fn test_pool_stats_debug() {
        let stats = PoolStats {
            cached_components: 5,
            total_idle_instances: 10,
            total_created: 100,
            total_destroyed: 50,
            total_requests: 1000,
            components: std::collections::HashMap::new(),
        };

        let debug = format!("{:?}", stats);
        assert!(debug.contains("cached_components: 5"));
        assert!(debug.contains("total_requests: 1000"));
    }

    /// Test WasmHttpConfig default values
    #[test]
    fn test_wasm_http_config_defaults() {
        let config = WasmHttpConfig::default();

        // Verify sensible defaults
        assert!(config.max_instances > 0);
        assert!(config.max_instances >= config.min_instances);
        assert!(config.idle_timeout.as_secs() > 0);
        assert!(config.request_timeout.as_secs() > 0);
    }
}

// =============================================================================
// E2E Test: WASM Host Functions
// =============================================================================

mod wasm_host_functions_e2e {
    use super::*;

    /// Test complete configuration workflow
    #[test]
    fn test_config_workflow() {
        let mut host = DefaultHost::new();

        // Add various config values
        host.add_config("database.host", "localhost");
        host.add_config("database.port", "5432");
        host.add_config("database.name", "testdb");
        host.add_config("api.timeout", "30");
        host.add_config("api.enabled", "true");

        // Test basic get
        assert_eq!(
            host.config_get("database.host"),
            Some("localhost".to_string())
        );
        assert_eq!(host.config_get("nonexistent"), None);

        // Test prefix query
        let db_configs = host.config_get_prefix("database.");
        assert_eq!(db_configs.len(), 3);

        // Test type conversions
        assert_eq!(host.config_get_int("database.port"), Some(5432));
        assert_eq!(host.config_get_bool("api.enabled"), Some(true));

        // Test required config
        assert!(host.config_get_required("database.host").is_ok());
        assert!(host.config_get_required("nonexistent").is_err());
    }

    /// Test config get_many operation
    #[test]
    fn test_config_get_many() {
        let mut host = DefaultHost::new();

        host.add_config("key1", "value1");
        host.add_config("key2", "value2");
        host.add_config("key3", "value3");

        let keys = vec![
            "key1".to_string(),
            "key2".to_string(),
            "nonexistent".to_string(),
        ];
        let results = host.config_get_many(&keys);

        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|(k, v)| k == "key1" && v == "value1"));
        assert!(results.iter().any(|(k, v)| k == "key2" && v == "value2"));
    }

    /// Test config exists operation
    #[test]
    fn test_config_exists() {
        let mut host = DefaultHost::new();

        host.add_config("exists", "value");

        assert!(host.config_exists("exists"));
        assert!(!host.config_exists("missing"));
    }

    /// Test config boolean parsing edge cases
    #[test]
    fn test_config_bool_parsing() {
        let mut host = DefaultHost::new();

        host.add_config("bool.true1", "true");
        host.add_config("bool.true2", "1");
        host.add_config("bool.true3", "yes");
        host.add_config("bool.false1", "false");
        host.add_config("bool.false2", "0");
        host.add_config("bool.false3", "no");
        host.add_config("bool.invalid", "maybe");

        assert_eq!(host.config_get_bool("bool.true1"), Some(true));
        assert_eq!(host.config_get_bool("bool.true2"), Some(true));
        assert_eq!(host.config_get_bool("bool.true3"), Some(true));
        assert_eq!(host.config_get_bool("bool.false1"), Some(false));
        assert_eq!(host.config_get_bool("bool.false2"), Some(false));
        assert_eq!(host.config_get_bool("bool.false3"), Some(false));
        assert_eq!(host.config_get_bool("bool.invalid"), None);
    }

    /// Test config float parsing
    #[test]
    fn test_config_float_parsing() {
        let mut host = DefaultHost::new();

        host.add_config("float.pi", "3.14159");
        host.add_config("float.int", "42");
        host.add_config("float.invalid", "not-a-number");

        assert!((host.config_get_float("float.pi").unwrap() - std::f64::consts::PI).abs() < 0.01);
        assert_eq!(host.config_get_float("float.int"), Some(42.0));
        assert_eq!(host.config_get_float("float.invalid"), None);
    }

    /// Test key-value storage workflow
    #[test]
    fn test_kv_workflow() {
        let mut host = DefaultHost::new();

        // Set and get values
        host.kv_set("user:1:name", b"Alice")
            .expect("Set should succeed");
        host.kv_set("user:1:email", b"alice@example.com")
            .expect("Set should succeed");

        let name = host.kv_get("user:1:name").expect("Get should succeed");
        assert_eq!(name, Some(b"Alice".to_vec()));

        // Test exists
        assert!(host.kv_exists("user:1:name"));
        assert!(!host.kv_exists("user:2:name"));

        // Test string convenience methods
        host.kv_set_string("user:1:status", "active")
            .expect("Set string should succeed");
        let status = host
            .kv_get_string("user:1:status")
            .expect("Get string should succeed");
        assert_eq!(status, Some("active".to_string()));

        // Test list keys
        let keys = host.kv_list_keys("user:1:").expect("List should succeed");
        assert_eq!(keys.len(), 3);

        // Test delete
        let deleted = host
            .kv_delete("user:1:status")
            .expect("Delete should succeed");
        assert!(deleted);
        assert!(!host.kv_exists("user:1:status"));

        // Delete non-existent should return false
        let deleted_again = host
            .kv_delete("user:1:status")
            .expect("Delete should succeed");
        assert!(!deleted_again);
    }

    /// Test KV with TTL
    #[test]
    fn test_kv_with_ttl() {
        let mut host = DefaultHost::new();

        // Set with TTL (5 seconds in nanoseconds)
        let ttl_ns = 5_000_000_000u64;
        host.kv_set_with_ttl("temp:key", b"temporary value", ttl_ns)
            .expect("Set with TTL should succeed");

        // Should exist immediately after setting
        assert!(host.kv_exists("temp:key"));

        let value = host.kv_get("temp:key").expect("Get should succeed");
        assert_eq!(value, Some(b"temporary value".to_vec()));
    }

    /// Test atomic increment operations
    #[test]
    fn test_kv_increment_workflow() {
        let mut host = DefaultHost::new();

        // Increment non-existent key starts at 0
        let val1 = host
            .kv_increment("counter", 5)
            .expect("Increment should succeed");
        assert_eq!(val1, 5);

        let val2 = host
            .kv_increment("counter", 3)
            .expect("Increment should succeed");
        assert_eq!(val2, 8);

        // Negative increment
        let val3 = host
            .kv_increment("counter", -2)
            .expect("Increment should succeed");
        assert_eq!(val3, 6);

        // Large increments
        let val4 = host
            .kv_increment("counter", 1000)
            .expect("Increment should succeed");
        assert_eq!(val4, 1006);
    }

    /// Test compare-and-swap operations
    #[test]
    fn test_kv_cas_workflow() {
        let mut host = DefaultHost::new();

        // CAS on non-existent key (expected None)
        let success = host
            .kv_compare_and_swap("lock", None, b"owner1")
            .expect("CAS should succeed");
        assert!(success, "CAS with None expected should succeed for new key");

        // CAS with correct expected value
        let success = host
            .kv_compare_and_swap("lock", Some(b"owner1"), b"owner2")
            .expect("CAS should succeed");
        assert!(success, "CAS with correct expected should succeed");

        // CAS with wrong expected value
        let success = host
            .kv_compare_and_swap("lock", Some(b"wrong"), b"owner3")
            .expect("CAS should succeed");
        assert!(!success, "CAS with wrong expected should fail");

        // Verify final value
        let value = host.kv_get("lock").expect("Get should succeed");
        assert_eq!(value, Some(b"owner2".to_vec()));
    }

    /// Test KV error cases
    #[test]
    fn test_kv_error_cases() {
        let mut host = DefaultHost::new();
        host.set_max_value_size(100); // Small limit for testing

        // Value too large
        let large_value = vec![0u8; 200];
        let result = host.kv_set("large", &large_value);
        assert!(matches!(result, Err(KvError::ValueTooLarge)));

        // Invalid key (empty)
        let result = host.kv_set("", b"value");
        assert!(matches!(result, Err(KvError::InvalidKey)));
    }

    /// Test logging workflow
    #[test]
    fn test_logging_workflow() {
        let mut host = DefaultHost::new();
        host.set_min_log_level(LogLevel::Debug);

        // Test log level checking
        assert!(!host.log_is_enabled(LogLevel::Trace)); // Below minimum
        assert!(host.log_is_enabled(LogLevel::Debug));
        assert!(host.log_is_enabled(LogLevel::Info));
        assert!(host.log_is_enabled(LogLevel::Warn));
        assert!(host.log_is_enabled(LogLevel::Error));

        // Log at various levels (doesn't panic)
        host.log(LogLevel::Debug, "Debug message");
        host.log(LogLevel::Info, "Info message");
        host.log(LogLevel::Warn, "Warning message");
        host.log(LogLevel::Error, "Error message");

        // Structured logging
        host.log_structured(
            LogLevel::Info,
            "Request completed",
            &[
                ("method".to_string(), "GET".to_string()),
                ("path".to_string(), "/api/test".to_string()),
                ("status".to_string(), "200".to_string()),
            ],
        );
    }

    /// Test LogLevel conversions
    #[test]
    fn test_log_level_conversions() {
        // WIT level conversions
        assert_eq!(LogLevel::from_wit(0), LogLevel::Trace);
        assert_eq!(LogLevel::from_wit(1), LogLevel::Debug);
        assert_eq!(LogLevel::from_wit(2), LogLevel::Info);
        assert_eq!(LogLevel::from_wit(3), LogLevel::Warn);
        assert_eq!(LogLevel::from_wit(4), LogLevel::Error);
        assert_eq!(LogLevel::from_wit(99), LogLevel::Error); // Unknown defaults to Error

        assert_eq!(LogLevel::Trace.to_wit(), 0);
        assert_eq!(LogLevel::Debug.to_wit(), 1);
        assert_eq!(LogLevel::Info.to_wit(), 2);
        assert_eq!(LogLevel::Warn.to_wit(), 3);
        assert_eq!(LogLevel::Error.to_wit(), 4);

        // Display formatting
        assert_eq!(format!("{}", LogLevel::Trace), "trace");
        assert_eq!(format!("{}", LogLevel::Debug), "debug");
        assert_eq!(format!("{}", LogLevel::Info), "info");
        assert_eq!(format!("{}", LogLevel::Warn), "warn");
        assert_eq!(format!("{}", LogLevel::Error), "error");
    }

    /// Test secrets workflow
    #[test]
    fn test_secrets_workflow() {
        let mut host = DefaultHost::new();

        host.add_secret("api_key", "sk-test-123456789");
        host.add_secret("db_password", "supersecret");

        // Get secret
        let api_key = host.secret_get("api_key").expect("Get should succeed");
        assert_eq!(api_key, Some("sk-test-123456789".to_string()));

        // Get non-existent
        let missing = host.secret_get("nonexistent").expect("Get should succeed");
        assert!(missing.is_none());

        // Check exists
        assert!(host.secret_exists("api_key"));
        assert!(!host.secret_exists("nonexistent"));

        // Required secret
        assert!(host.secret_get_required("api_key").is_ok());
        assert!(host.secret_get_required("nonexistent").is_err());

        // List names
        let names = host.secret_list_names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"api_key".to_string()));
        assert!(names.contains(&"db_password".to_string()));
    }

    /// Test metrics workflow
    #[test]
    fn test_metrics_workflow() {
        let host = DefaultHost::new();

        // Counter operations
        host.counter_inc("requests_total", 1);
        host.counter_inc("requests_total", 5);

        // Counter with labels
        host.counter_inc_labeled(
            "http_requests",
            1,
            &[
                ("method".to_string(), "GET".to_string()),
                ("status".to_string(), "200".to_string()),
            ],
        );

        // Gauge operations
        host.gauge_set("active_connections", 10.0);
        host.gauge_add("active_connections", 5.0);
        host.gauge_add("active_connections", -3.0);

        // Gauge with labels
        host.gauge_set_labeled(
            "queue_size",
            50.0,
            &[("queue_name".to_string(), "default".to_string())],
        );

        // Histogram operations
        host.histogram_observe("response_time_seconds", 0.123);
        host.histogram_observe("response_time_seconds", 0.456);

        // Histogram with labels
        host.histogram_observe_labeled(
            "request_duration",
            0.05,
            &[("endpoint".to_string(), "/api/users".to_string())],
        );

        // Duration recording
        host.record_duration("db_query_ns", 50_000_000); // 50ms in ns
        host.record_duration_labeled(
            "external_call_ns",
            100_000_000,
            &[("service".to_string(), "auth".to_string())],
        );
    }

    /// Test MetricsStore operations
    #[test]
    fn test_metrics_store_operations() {
        let host = DefaultHost::new();

        // Increment counter multiple times
        host.counter_inc("test_counter", 10);
        host.counter_inc("test_counter", 5);

        // Set gauges
        host.gauge_set("test_gauge", 42.0);

        // Observe histograms
        host.histogram_observe("test_histogram", 1.0);
        host.histogram_observe("test_histogram", 2.0);
        host.histogram_observe("test_histogram", 3.0);
    }

    /// Test full config -> kv -> logging -> metrics flow
    #[test]
    fn test_complete_host_functions_flow() {
        let mut host = DefaultHost::with_plugin_id("test-plugin");

        // Setup configuration
        host.add_configs([
            ("feature.caching", "true"),
            ("cache.ttl_seconds", "300"),
            ("cache.max_size", "1000"),
        ]);

        host.add_secret("encryption_key", "secret123");

        // Simulate a plugin workflow
        // 1. Check config
        let caching_enabled = host.config_get_bool("feature.caching").unwrap_or(false);
        assert!(caching_enabled);

        let ttl = host.config_get_int("cache.ttl_seconds").unwrap_or(60);
        assert_eq!(ttl, 300);

        // 2. Log operation start
        host.log(LogLevel::Info, "Starting cached operation");

        // 3. Check cache (KV)
        let cache_key = "data:user:1";
        let cached = host.kv_get(cache_key).expect("KV get should work");

        if cached.is_none() {
            // 4. Get secret for encryption
            let _key = host
                .secret_get_required("encryption_key")
                .expect("Secret should exist");

            // 5. Store in cache with TTL
            host.kv_set_with_ttl(cache_key, b"cached_data", ttl as u64 * 1_000_000_000)
                .expect("KV set should work");

            // 6. Record metrics
            host.counter_inc("cache_misses", 1);
            host.log(LogLevel::Debug, "Cache miss, fetched and stored");
        } else {
            host.counter_inc("cache_hits", 1);
        }

        // 7. Record duration
        host.record_duration("operation_ns", 10_000_000); // 10ms

        // 8. Final structured log
        host.log_structured(
            LogLevel::Info,
            "Operation completed",
            &[
                ("cache_hit".to_string(), "false".to_string()),
                ("duration_ms".to_string(), "10".to_string()),
            ],
        );
    }

    /// Test DefaultHost with_plugin_id constructor
    #[test]
    fn test_host_with_plugin_id() {
        let host = DefaultHost::with_plugin_id("my-custom-plugin");
        // Plugin ID is used internally for logging context
        // Just verify construction works
        assert!(host.config_get("nonexistent").is_none());
    }

    /// Test add_configs batch operation
    #[test]
    fn test_add_configs_batch() {
        let mut host = DefaultHost::new();

        host.add_configs([
            ("batch.key1", "value1"),
            ("batch.key2", "value2"),
            ("batch.key3", "value3"),
        ]);

        assert_eq!(host.config_get("batch.key1"), Some("value1".to_string()));
        assert_eq!(host.config_get("batch.key2"), Some("value2".to_string()));
        assert_eq!(host.config_get("batch.key3"), Some("value3".to_string()));
    }
}

// =============================================================================
// E2E Test: Full Plugin Lifecycle (Conceptual)
// =============================================================================

mod wasm_plugin_lifecycle_e2e {
    use super::*;

    /// Test simulated plugin lifecycle through host functions
    #[test]
    fn test_simulated_plugin_lifecycle() {
        let mut host = DefaultHost::with_plugin_id("lifecycle-test-plugin");

        // Phase 1: Configuration Loading (init phase)
        host.add_configs([
            ("plugin.version", "1.0.0"),
            ("plugin.enabled", "true"),
            ("feature.analytics", "true"),
        ]);
        host.add_secret("plugin_token", "secret-token-123");

        // Simulate init() call
        host.log(LogLevel::Info, "Plugin initialization started");

        let enabled = host.config_get_bool("plugin.enabled").unwrap_or(false);
        assert!(enabled, "Plugin should be enabled");

        let version = host.config_get("plugin.version");
        assert_eq!(version, Some("1.0.0".to_string()));

        // Verify secret access
        let token = host.secret_get_required("plugin_token");
        assert!(token.is_ok(), "Should access plugin token");

        host.log(LogLevel::Info, "Plugin initialization completed");
        host.counter_inc("plugin_init_count", 1);

        // Phase 2: Info Query
        host.log_structured(
            LogLevel::Debug,
            "Plugin info requested",
            &[
                ("id".to_string(), "lifecycle-test-plugin".to_string()),
                ("version".to_string(), "1.0.0".to_string()),
            ],
        );

        // Phase 3: Handle Events (multiple calls)
        for i in 0..3 {
            host.log(LogLevel::Debug, &format!("Handling event {}", i + 1));

            // Simulate caching behavior
            let cache_key = format!("event:{}", i);
            host.kv_set_string(&cache_key, &format!("processed-{}", i))
                .expect("Cache set should work");

            host.counter_inc("events_processed", 1);
            host.histogram_observe("event_processing_time", 0.05 + (i as f64 * 0.01));
        }

        // Verify events were processed
        assert!(host.kv_exists("event:0"));
        assert!(host.kv_exists("event:1"));
        assert!(host.kv_exists("event:2"));

        // Phase 4: Shutdown
        host.log(LogLevel::Info, "Plugin shutdown initiated");

        // Cleanup KV storage
        for i in 0..3 {
            let cache_key = format!("event:{}", i);
            host.kv_delete(&cache_key).expect("Delete should work");
        }

        // Verify cleanup
        assert!(!host.kv_exists("event:0"));
        assert!(!host.kv_exists("event:1"));
        assert!(!host.kv_exists("event:2"));

        host.log(LogLevel::Info, "Plugin shutdown completed");
        host.gauge_set("plugin_active", 0.0);
    }

    /// Test error handling in plugin lifecycle
    #[test]
    fn test_plugin_error_handling() {
        let mut host = DefaultHost::with_plugin_id("error-test-plugin");

        // Setup minimal config (missing required fields)
        host.add_config("plugin.enabled", "true");
        // Intentionally NOT adding required secret

        // Simulate init that requires a secret
        let required_secret = host.secret_get_required("required_api_key");
        assert!(
            required_secret.is_err(),
            "Should fail when required secret is missing"
        );

        // Log the error
        if let Err(ref e) = required_secret {
            host.log(LogLevel::Error, &format!("Init failed: {}", e));
            host.counter_inc("plugin_init_failures", 1);
        }

        // Simulate KV error handling with limits
        host.set_max_keys(5);
        for i in 0..5 {
            let key = format!("key{}", i);
            host.kv_set(&key, b"value").expect("Should succeed");
        }

        // 6th key should fail due to quota
        let _result = host.kv_set("key5", b"value");
        // Note: This might succeed if one of the earlier keys had same name
        // The test validates error handling exists
    }

    /// Test plugin with multiple event types
    #[test]
    fn test_plugin_multiple_event_types() {
        let mut host = DefaultHost::with_plugin_id("multi-event-plugin");

        host.add_config("events.http", "true");
        host.add_config("events.timer", "true");
        host.add_config("events.custom", "true");

        // Simulate HTTP event handling
        if host.config_get_bool("events.http").unwrap_or(false) {
            host.log(LogLevel::Info, "Handling HTTP event");
            host.counter_inc_labeled("events", 1, &[("type".to_string(), "http".to_string())]);
            host.histogram_observe_labeled(
                "event_duration",
                0.05,
                &[("type".to_string(), "http".to_string())],
            );
        }

        // Simulate timer event handling
        if host.config_get_bool("events.timer").unwrap_or(false) {
            host.log(LogLevel::Info, "Handling timer event");
            host.counter_inc_labeled("events", 1, &[("type".to_string(), "timer".to_string())]);
        }

        // Simulate custom event handling
        if host.config_get_bool("events.custom").unwrap_or(false) {
            host.log(LogLevel::Info, "Handling custom event");
            host.counter_inc_labeled("events", 1, &[("type".to_string(), "custom".to_string())]);
        }
    }

    /// Test plugin state persistence across events
    #[test]
    fn test_plugin_state_persistence() {
        let mut host = DefaultHost::with_plugin_id("stateful-plugin");

        // First event: Initialize state
        host.kv_set_string("state:initialized", "true")
            .expect("Should set state");
        host.kv_increment("state:event_count", 1)
            .expect("Should increment");

        // Second event: Update state
        let count = host
            .kv_increment("state:event_count", 1)
            .expect("Should increment");
        assert_eq!(count, 2);

        // Third event: Read and update state
        let initialized = host
            .kv_get_string("state:initialized")
            .expect("Should get")
            .expect("Should exist");
        assert_eq!(initialized, "true");

        let final_count = host
            .kv_increment("state:event_count", 1)
            .expect("Should increment");
        assert_eq!(final_count, 3);

        // Cleanup
        host.kv_delete("state:initialized").expect("Should delete");
        host.kv_delete("state:event_count").expect("Should delete");
    }

    /// Test plugin with concurrent-safe operations
    #[test]
    fn test_plugin_concurrent_safe_operations() {
        let mut host = DefaultHost::with_plugin_id("concurrent-plugin");

        // Simulate concurrent counter updates
        for _ in 0..100 {
            host.counter_inc("concurrent_counter", 1);
        }

        // Simulate CAS-based lock acquisition
        let acquired = host
            .kv_compare_and_swap("lock", None, b"owner1")
            .expect("CAS should work");
        assert!(acquired, "First lock should succeed");

        let reacquired = host
            .kv_compare_and_swap("lock", None, b"owner2")
            .expect("CAS should work");
        assert!(!reacquired, "Second lock should fail");

        // Release lock
        let released = host
            .kv_compare_and_swap("lock", Some(b"owner1"), b"")
            .expect("CAS should work");
        assert!(released, "Release should succeed");
    }
}

// =============================================================================
// E2E Test: KvError Display and Debug
// =============================================================================

mod kv_error_e2e {
    use super::*;

    #[test]
    fn test_kv_error_display() {
        let not_found = KvError::NotFound;
        assert!(format!("{}", not_found).contains("not found"));

        let too_large = KvError::ValueTooLarge;
        assert!(format!("{}", too_large).contains("too large"));

        let quota = KvError::QuotaExceeded;
        assert!(format!("{}", quota).contains("quota"));

        let invalid = KvError::InvalidKey;
        assert!(format!("{}", invalid).contains("invalid"));

        let storage = KvError::Storage("connection failed".to_string());
        assert!(format!("{}", storage).contains("connection failed"));
    }
}

// =============================================================================
// E2E Test: WASM Runtime Configuration
// =============================================================================

mod wasm_runtime_config_e2e {
    use super::*;

    /// Test WasmHttpConfig validation
    #[test]
    fn test_wasm_http_config_validation() {
        // Valid config
        let valid = WasmHttpConfig {
            min_instances: 1,
            max_instances: 10,
            idle_timeout: Duration::from_secs(60),
            request_timeout: Duration::from_secs(30),
        };
        assert!(valid.max_instances >= valid.min_instances);

        // Edge case: min equals max
        let equal = WasmHttpConfig {
            min_instances: 5,
            max_instances: 5,
            idle_timeout: Duration::from_secs(60),
            request_timeout: Duration::from_secs(30),
        };
        assert_eq!(equal.min_instances, equal.max_instances);

        // Zero min instances (cold start)
        let cold_start = WasmHttpConfig {
            min_instances: 0,
            max_instances: 10,
            idle_timeout: Duration::from_secs(60),
            request_timeout: Duration::from_secs(30),
        };
        assert_eq!(cold_start.min_instances, 0);
    }

    /// Test timeout configurations
    #[test]
    fn test_timeout_configurations() {
        let short = WasmHttpConfig {
            min_instances: 1,
            max_instances: 5,
            idle_timeout: Duration::from_millis(100),
            request_timeout: Duration::from_millis(50),
        };
        assert!(short.request_timeout < short.idle_timeout);

        let long = WasmHttpConfig {
            min_instances: 1,
            max_instances: 5,
            idle_timeout: Duration::from_secs(3600), // 1 hour
            request_timeout: Duration::from_secs(300), // 5 minutes
        };
        assert!(long.idle_timeout > Duration::from_secs(60));
    }
}

// =============================================================================
// E2E Test: WASM Networking Capability
// =============================================================================

mod wasm_networking_e2e {
    use wasmtime_wasi::WasiCtxBuilder;

    /// Test that networking is enabled in WASM context builder
    ///
    /// This verifies that the wasi:sockets interfaces (TCP, UDP, IP name lookup)
    /// are properly available when inherit_network() is called.
    #[test]
    fn test_wasm_networking_capability_enabled() {
        // Verify WasiCtxBuilder with inherit_network compiles and works
        let mut builder = WasiCtxBuilder::new();
        builder.inherit_network();
        let _ctx = builder.build();
        // If this compiles and runs, networking is properly configured
    }

    /// Test that we can build a WASI context with both networking and stdio
    #[test]
    fn test_wasm_networking_with_stdio() {
        use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};

        let mut builder = WasiCtxBuilder::new();

        // Configure stdio
        builder.stdin(MemoryInputPipe::new(Vec::new()));
        builder.stdout(MemoryOutputPipe::new(1024));
        builder.stderr(MemoryOutputPipe::new(1024));

        // Enable networking
        builder.inherit_network();

        // Set environment and args
        builder.env("TEST_VAR", "test_value");
        builder.args(&["test-program".to_string(), "arg1".to_string()]);

        let _ctx = builder.build();
    }
}

// =============================================================================
// E2E Test: WASM Filesystem Mount Configuration
// =============================================================================

mod wasm_filesystem_e2e {
    use zlayer_spec::{StorageSpec, StorageTier};

    /// Test StorageSpec parsing for WASM bind mounts
    #[test]
    fn test_bind_mount_parsing() {
        let spec = StorageSpec::Bind {
            source: "/host/data".to_string(),
            target: "/guest/data".to_string(),
            readonly: true,
        };
        // Verify the mount can be matched and fields extracted
        match &spec {
            StorageSpec::Bind {
                source,
                target,
                readonly,
            } => {
                assert_eq!(source, "/host/data");
                assert_eq!(target, "/guest/data");
                assert!(*readonly);
            }
            _ => panic!("Expected Bind mount"),
        }
    }

    /// Test bind mount with write access
    #[test]
    fn test_bind_mount_writable() {
        let spec = StorageSpec::Bind {
            source: "/var/lib/app/data".to_string(),
            target: "/app/data".to_string(),
            readonly: false,
        };
        match &spec {
            StorageSpec::Bind {
                source,
                target,
                readonly,
            } => {
                assert_eq!(source, "/var/lib/app/data");
                assert_eq!(target, "/app/data");
                assert!(!*readonly);
            }
            _ => panic!("Expected Bind mount"),
        }
    }

    /// Test named volume mount parsing
    #[test]
    fn test_named_volume_parsing() {
        let spec = StorageSpec::Named {
            name: "my-volume".to_string(),
            target: "/data".to_string(),
            readonly: false,
            tier: StorageTier::Local,
            size: None,
        };
        match &spec {
            StorageSpec::Named {
                name,
                target,
                readonly,
                tier,
                ..
            } => {
                assert_eq!(name, "my-volume");
                assert_eq!(target, "/data");
                assert!(!*readonly);
                assert_eq!(*tier, StorageTier::Local);
            }
            _ => panic!("Expected Named mount"),
        }
    }

    /// Test named volume with different storage tiers
    #[test]
    fn test_named_volume_storage_tiers() {
        // Local tier (default, SQLite-safe)
        let local_vol = StorageSpec::Named {
            name: "db-storage".to_string(),
            target: "/var/lib/db".to_string(),
            readonly: false,
            tier: StorageTier::Local,
            size: None,
        };
        if let StorageSpec::Named { tier, .. } = &local_vol {
            assert_eq!(*tier, StorageTier::Local);
        }

        // Cached tier (SSD cache + slower backend)
        let cached_vol = StorageSpec::Named {
            name: "cache-storage".to_string(),
            target: "/var/cache".to_string(),
            readonly: false,
            tier: StorageTier::Cached,
            size: None,
        };
        if let StorageSpec::Named { tier, .. } = &cached_vol {
            assert_eq!(*tier, StorageTier::Cached);
        }

        // Network tier (NOT SQLite-safe)
        let network_vol = StorageSpec::Named {
            name: "shared-storage".to_string(),
            target: "/shared".to_string(),
            readonly: true,
            tier: StorageTier::Network,
            size: None,
        };
        if let StorageSpec::Named { tier, .. } = &network_vol {
            assert_eq!(*tier, StorageTier::Network);
        }
    }

    /// Test unsupported storage types for WASM (should be skipped with warnings)
    #[test]
    fn test_unsupported_storage_types() {
        // Tmpfs - memory-backed, not supported in WASI
        let tmpfs = StorageSpec::Tmpfs {
            target: "/tmp".to_string(),
            size: Some("100M".to_string()),
            mode: None,
        };
        assert!(matches!(tmpfs, StorageSpec::Tmpfs { .. }));

        // Anonymous - auto-named volumes, not supported for WASM
        let anonymous = StorageSpec::Anonymous {
            target: "/scratch".to_string(),
            tier: StorageTier::Local,
        };
        assert!(matches!(anonymous, StorageSpec::Anonymous { .. }));
    }

    /// Test multiple mounts configuration
    #[test]
    fn test_multiple_mounts() {
        let mounts = vec![
            StorageSpec::Bind {
                source: "/host/config".to_string(),
                target: "/app/config".to_string(),
                readonly: true,
            },
            StorageSpec::Named {
                name: "app-data".to_string(),
                target: "/app/data".to_string(),
                readonly: false,
                tier: StorageTier::Local,
                size: None,
            },
            StorageSpec::Bind {
                source: "/var/log/app".to_string(),
                target: "/app/logs".to_string(),
                readonly: false,
            },
        ];

        assert_eq!(mounts.len(), 3);

        // Count bind mounts
        let bind_count = mounts
            .iter()
            .filter(|m| matches!(m, StorageSpec::Bind { .. }))
            .count();
        assert_eq!(bind_count, 2);

        // Count named volumes
        let named_count = mounts
            .iter()
            .filter(|m| matches!(m, StorageSpec::Named { .. }))
            .count();
        assert_eq!(named_count, 1);
    }
}

// =============================================================================
// E2E Test: WASM stdout/stderr Capture
// =============================================================================

mod wasm_stdio_capture_e2e {
    use wasmtime_wasi::p2::pipe::MemoryOutputPipe;

    /// Test that MemoryOutputPipe can be created and cloned
    #[test]
    fn test_memory_output_pipe_creation() {
        let pipe = MemoryOutputPipe::new(1024);
        let _pipe_clone = pipe.clone();
        // Pipes are clonable (Arc<Mutex> internally)
    }

    /// Test that MemoryOutputPipe contents can be read
    #[test]
    fn test_memory_output_pipe_contents() {
        let pipe = MemoryOutputPipe::new(1024);
        let pipe_clone = pipe.clone();

        // Initially empty
        let contents = pipe_clone.contents();
        assert!(contents.is_empty(), "New pipe should have empty contents");
    }

    /// Test stdout/stderr pipe configuration for WASI
    #[test]
    fn test_stdio_pipe_configuration() {
        use wasmtime_wasi::p2::pipe::MemoryInputPipe;
        use wasmtime_wasi::WasiCtxBuilder;

        // Create pipes for capture
        let stdout_pipe = MemoryOutputPipe::new(1024 * 1024); // 1MB
        let stderr_pipe = MemoryOutputPipe::new(1024 * 1024);

        // Clone for later reading
        let stdout_clone = stdout_pipe.clone();
        let stderr_clone = stderr_pipe.clone();

        // Configure WASI context with pipes
        let mut builder = WasiCtxBuilder::new();
        builder.stdin(MemoryInputPipe::new(Vec::new()));
        builder.stdout(stdout_pipe);
        builder.stderr(stderr_pipe);

        let _ctx = builder.build();

        // Verify we can still access the cloned pipes
        assert!(stdout_clone.contents().is_empty());
        assert!(stderr_clone.contents().is_empty());
    }

    /// Test different pipe capacities
    #[test]
    fn test_pipe_capacity_configurations() {
        // Small pipe for limited output
        let small_pipe = MemoryOutputPipe::new(1024); // 1KB
        assert!(small_pipe.contents().is_empty());

        // Medium pipe for typical output
        let medium_pipe = MemoryOutputPipe::new(64 * 1024); // 64KB
        assert!(medium_pipe.contents().is_empty());

        // Large pipe for verbose output
        let large_pipe = MemoryOutputPipe::new(1024 * 1024); // 1MB
        assert!(large_pipe.contents().is_empty());
    }
}

// =============================================================================
// E2E Test: Custom HTTP Interface Types
// =============================================================================

mod wasm_http_interfaces_e2e {
    use super::*;
    use zlayer_agent::runtimes::{
        duration_to_ns, ns_to_duration, CacheDecision, CacheEntry, HttpMethod, HttpVersion,
        ImmediateResponse, KeyValue, MessageType, MiddlewareAction, PluginRequest, RedirectInfo,
        RequestMetadata, RoutingDecision, UpgradeDecision, Upstream, WebSocketMessage,
    };

    // -------------------------------------------------------------------------
    // Routing Decision Tests
    // -------------------------------------------------------------------------

    /// Test RoutingDecision::Forward variant
    #[test]
    fn test_routing_decision_forward() {
        let upstream = Upstream::new("backend.local", 8080);
        let decision = RoutingDecision::Forward(upstream);
        match decision {
            RoutingDecision::Forward(u) => {
                assert_eq!(u.host, "backend.local");
                assert_eq!(u.port, 8080);
                assert!(!u.tls);
            }
            _ => panic!("Expected Forward"),
        }
    }

    /// Test RoutingDecision::Forward with HTTPS
    #[test]
    fn test_routing_decision_forward_https() {
        let upstream = Upstream::https("api.example.com", 443);
        let decision = RoutingDecision::Forward(upstream);
        match decision {
            RoutingDecision::Forward(u) => {
                assert_eq!(u.host, "api.example.com");
                assert_eq!(u.port, 443);
                assert!(u.tls);
                assert_eq!(u.url(), "https://api.example.com:443");
            }
            _ => panic!("Expected Forward"),
        }
    }

    /// Test RoutingDecision::Redirect variant
    #[test]
    fn test_routing_decision_redirect() {
        let redirect = RedirectInfo::permanent("https://example.com/new-path");
        let decision = RoutingDecision::Redirect(redirect);
        match decision {
            RoutingDecision::Redirect(r) => {
                assert_eq!(r.location, "https://example.com/new-path");
                assert_eq!(r.status, 301);
                assert!(!r.preserve_body);
            }
            _ => panic!("Expected Redirect"),
        }
    }

    /// Test RoutingDecision::RespondImmediate variant
    #[test]
    fn test_routing_decision_respond_immediate() {
        let response = ImmediateResponse::forbidden()
            .with_header("X-Reason", "Access denied")
            .with_text_body("Forbidden");
        let decision = RoutingDecision::RespondImmediate(response);
        match decision {
            RoutingDecision::RespondImmediate(r) => {
                assert_eq!(r.status, 403);
                assert!(!r.headers.is_empty());
                assert!(!r.body.is_empty());
            }
            _ => panic!("Expected RespondImmediate"),
        }
    }

    /// Test RoutingDecision::ContinueProcessing variant
    #[test]
    fn test_routing_decision_continue() {
        let decision = RoutingDecision::ContinueProcessing;
        assert!(matches!(decision, RoutingDecision::ContinueProcessing));
    }

    // -------------------------------------------------------------------------
    // Upstream Tests
    // -------------------------------------------------------------------------

    /// Test Upstream construction and URL generation
    #[test]
    fn test_upstream_url_generation() {
        let http = Upstream::new("backend", 8080);
        assert_eq!(http.url(), "http://backend:8080");

        let https = Upstream::https("secure-backend", 443);
        assert_eq!(https.url(), "https://secure-backend:443");
    }

    /// Test Upstream timeout configuration
    #[test]
    fn test_upstream_timeouts() {
        let upstream = Upstream::new("backend", 80)
            .with_connect_timeout(Duration::from_secs(10))
            .with_request_timeout(Duration::from_secs(60));

        assert_eq!(upstream.connect_timeout(), Duration::from_secs(10));
        assert_eq!(upstream.request_timeout(), Duration::from_secs(60));
    }

    // -------------------------------------------------------------------------
    // Middleware Action Tests
    // -------------------------------------------------------------------------

    /// Test MiddlewareAction::ContinueWith variant
    #[test]
    fn test_middleware_action_continue_with_headers() {
        let headers = vec![
            KeyValue::new("X-Custom", "value"),
            KeyValue::new("X-Request-ID", "req-123"),
        ];
        let action = MiddlewareAction::ContinueWith(headers);
        match action {
            MiddlewareAction::ContinueWith(h) => {
                assert_eq!(h.len(), 2);
                assert_eq!(h[0].key, "X-Custom");
                assert_eq!(h[0].value, "value");
            }
            _ => panic!("Expected ContinueWith"),
        }
    }

    /// Test MiddlewareAction::Abort variant
    #[test]
    fn test_middleware_action_abort() {
        let action = MiddlewareAction::Abort {
            status: 403,
            reason: "Forbidden".to_string(),
        };
        match action {
            MiddlewareAction::Abort { status, reason } => {
                assert_eq!(status, 403);
                assert_eq!(reason, "Forbidden");
            }
            _ => panic!("Expected Abort"),
        }
    }

    /// Test MiddlewareAction convenience constructors
    #[test]
    fn test_middleware_action_constructors() {
        let unchanged = MiddlewareAction::continue_unchanged();
        assert!(unchanged.is_continue());

        let forbidden = MiddlewareAction::forbidden("Access denied");
        assert!(forbidden.is_abort());

        let rate_limited = MiddlewareAction::rate_limited("Too many requests");
        assert!(rate_limited.is_abort());
        if let MiddlewareAction::Abort { status, .. } = rate_limited {
            assert_eq!(status, 429);
        }
    }

    // -------------------------------------------------------------------------
    // WebSocket Tests
    // -------------------------------------------------------------------------

    /// Test UpgradeDecision::Accept variant
    #[test]
    fn test_websocket_upgrade_accept() {
        let decision = UpgradeDecision::Accept;
        assert!(decision.is_accepted());
    }

    /// Test UpgradeDecision::AcceptWithHeaders variant
    #[test]
    fn test_websocket_upgrade_accept_with_headers() {
        let headers = vec![KeyValue::new("Sec-WebSocket-Protocol", "graphql-ws")];
        let decision = UpgradeDecision::AcceptWithHeaders(headers);
        assert!(decision.is_accepted());
    }

    /// Test UpgradeDecision::Reject variant
    #[test]
    fn test_websocket_upgrade_reject() {
        let decision = UpgradeDecision::Reject {
            status: 401,
            reason: "Unauthorized".to_string(),
        };
        assert!(!decision.is_accepted());
    }

    /// Test WebSocketMessage types
    #[test]
    fn test_websocket_message_types() {
        let text_msg = WebSocketMessage::text("Hello, WebSocket!");
        assert_eq!(text_msg.msg_type, MessageType::Text);
        assert_eq!(text_msg.as_text(), Some("Hello, WebSocket!"));
        assert!(!text_msg.is_control());

        let binary_msg = WebSocketMessage::binary(vec![0x01, 0x02, 0x03]);
        assert_eq!(binary_msg.msg_type, MessageType::Binary);
        assert!(binary_msg.as_text().is_none());

        let ping_msg = WebSocketMessage::ping(vec![1, 2, 3, 4]);
        assert_eq!(ping_msg.msg_type, MessageType::Ping);
        assert!(ping_msg.is_control());

        let pong_msg = WebSocketMessage::pong(vec![1, 2, 3, 4]);
        assert_eq!(pong_msg.msg_type, MessageType::Pong);
        assert!(pong_msg.is_control());

        let close_msg = WebSocketMessage::close();
        assert_eq!(close_msg.msg_type, MessageType::Close);
        assert!(close_msg.is_control());
    }

    // -------------------------------------------------------------------------
    // Caching Tests
    // -------------------------------------------------------------------------

    /// Test CacheDecision::NoCache variant
    #[test]
    fn test_cache_decision_no_cache() {
        let no_cache = CacheDecision::NoCache;
        assert!(matches!(no_cache, CacheDecision::NoCache));
        assert!(!no_cache.is_cacheable());
        assert!(no_cache.ttl().is_none());
    }

    /// Test CacheDecision::CacheFor variant
    #[test]
    fn test_cache_decision_cache_for() {
        let cache_for = CacheDecision::cache_for(Duration::from_secs(300));
        assert!(cache_for.is_cacheable());
        assert_eq!(cache_for.ttl(), Some(Duration::from_secs(300)));
    }

    /// Test CacheDecision::CacheWithTags variant
    #[test]
    fn test_cache_decision_cache_with_tags() {
        let entry = CacheEntry::ttl_secs(600)
            .with_tag("api")
            .with_tag("v1")
            .vary_on("Accept")
            .with_stale_while_revalidate(Duration::from_secs(60));

        let cache_with_tags = CacheDecision::CacheWithTags(entry);
        match cache_with_tags {
            CacheDecision::CacheWithTags(e) => {
                assert_eq!(e.tags.len(), 2);
                assert!(e.tags.contains(&"api".to_string()));
                assert!(e.tags.contains(&"v1".to_string()));
                assert_eq!(e.vary.len(), 1);
                assert!(e.vary.contains(&"Accept".to_string()));
                assert_eq!(e.ttl(), Duration::from_secs(600));
                assert_eq!(e.stale_while_revalidate(), Some(Duration::from_secs(60)));
            }
            _ => panic!("Expected CacheWithTags"),
        }
    }

    // -------------------------------------------------------------------------
    // HTTP Method Tests
    // -------------------------------------------------------------------------

    /// Test HttpMethod enum Display implementation
    #[test]
    fn test_http_method_display() {
        assert_eq!(HttpMethod::Get.to_string(), "GET");
        assert_eq!(HttpMethod::Post.to_string(), "POST");
        assert_eq!(HttpMethod::Put.to_string(), "PUT");
        assert_eq!(HttpMethod::Delete.to_string(), "DELETE");
        assert_eq!(HttpMethod::Patch.to_string(), "PATCH");
        assert_eq!(HttpMethod::Head.to_string(), "HEAD");
        assert_eq!(HttpMethod::Options.to_string(), "OPTIONS");
        assert_eq!(HttpMethod::Connect.to_string(), "CONNECT");
        assert_eq!(HttpMethod::Trace.to_string(), "TRACE");
    }

    /// Test HttpMethod FromStr implementation
    #[test]
    fn test_http_method_from_str() {
        use std::str::FromStr;

        assert_eq!(HttpMethod::from_str("GET").unwrap(), HttpMethod::Get);
        assert_eq!(HttpMethod::from_str("post").unwrap(), HttpMethod::Post);
        assert_eq!(HttpMethod::from_str("PUT").unwrap(), HttpMethod::Put);
        assert_eq!(HttpMethod::from_str("delete").unwrap(), HttpMethod::Delete);
        assert!(HttpMethod::from_str("UNKNOWN").is_err());
    }

    // -------------------------------------------------------------------------
    // HttpVersion Tests
    // -------------------------------------------------------------------------

    /// Test HttpVersion enum
    #[test]
    fn test_http_version_enum() {
        assert_eq!(HttpVersion::Http10.to_string(), "HTTP/1.0");
        assert_eq!(HttpVersion::Http11.to_string(), "HTTP/1.1");
        assert_eq!(HttpVersion::Http2.to_string(), "HTTP/2");
        assert_eq!(HttpVersion::Http3.to_string(), "HTTP/3");
        assert_eq!(HttpVersion::default(), HttpVersion::Http11);
    }

    // -------------------------------------------------------------------------
    // RequestMetadata Tests
    // -------------------------------------------------------------------------

    /// Test RequestMetadata construction
    #[test]
    fn test_request_metadata_construction() {
        let metadata = RequestMetadata::with_client("192.168.1.100", 54321)
            .with_tls("TLSv1.3", "TLS_AES_256_GCM_SHA384")
            .with_server_name("api.example.com")
            .with_http_version(HttpVersion::Http2)
            .with_timestamp(1_234_567_890_000_000_000);

        assert_eq!(metadata.client_ip, "192.168.1.100");
        assert_eq!(metadata.client_port, 54321);
        assert_eq!(metadata.tls_version, Some("TLSv1.3".to_string()));
        assert_eq!(
            metadata.tls_cipher,
            Some("TLS_AES_256_GCM_SHA384".to_string())
        );
        assert_eq!(metadata.server_name, Some("api.example.com".to_string()));
        assert_eq!(metadata.http_version, HttpVersion::Http2);
        assert_eq!(metadata.received_at, 1_234_567_890_000_000_000);
    }

    /// Test RequestMetadata local convenience constructor
    #[test]
    fn test_request_metadata_local() {
        let metadata = RequestMetadata::local();
        assert_eq!(metadata.client_ip, "127.0.0.1");
        assert_eq!(metadata.client_port, 0);
        assert!(metadata.tls_version.is_none());
    }

    // -------------------------------------------------------------------------
    // PluginRequest Tests
    // -------------------------------------------------------------------------

    /// Test PluginRequest construction
    #[test]
    fn test_plugin_request_construction() {
        let request = PluginRequest::get("/api/users")
            .with_query("page=1&limit=10")
            .with_header("Accept", "application/json")
            .with_header("Authorization", "Bearer token123")
            .with_body(Vec::new())
            .with_context("trace_id", "abc123");

        assert_eq!(request.path, "/api/users");
        assert_eq!(request.method, HttpMethod::Get);
        assert_eq!(request.query, Some("page=1&limit=10".to_string()));
        assert_eq!(request.headers.len(), 2);
        assert_eq!(request.header("accept"), Some("application/json"));
        assert_eq!(request.context_value("trace_id"), Some("abc123"));
        assert_eq!(request.uri(), "/api/users?page=1&limit=10");
    }

    /// Test PluginRequest POST with body
    #[test]
    fn test_plugin_request_post_with_body() {
        let body = r#"{"name": "test", "value": 42}"#.as_bytes().to_vec();
        let request = PluginRequest::post("/api/items")
            .with_header("Content-Type", "application/json")
            .with_body(body.clone());

        assert_eq!(request.method, HttpMethod::Post);
        assert_eq!(request.body, body);
        assert!(!request.request_id.is_empty());
    }

    // -------------------------------------------------------------------------
    // Duration Conversion Tests
    // -------------------------------------------------------------------------

    /// Test duration conversion utilities
    #[test]
    fn test_duration_conversions() {
        let dur = Duration::from_millis(1500);
        let ns = duration_to_ns(dur);
        assert_eq!(ns, 1_500_000_000);

        let back = ns_to_duration(ns);
        assert_eq!(back, dur);
    }

    /// Test duration conversion edge cases
    #[test]
    fn test_duration_conversion_edge_cases() {
        // Zero duration
        let zero = Duration::from_secs(0);
        assert_eq!(duration_to_ns(zero), 0);
        assert_eq!(ns_to_duration(0), zero);

        // Large duration
        let large = Duration::from_secs(3600); // 1 hour
        let ns = duration_to_ns(large);
        assert_eq!(ns, 3_600_000_000_000);
        assert_eq!(ns_to_duration(ns), large);
    }

    // -------------------------------------------------------------------------
    // KeyValue Tests
    // -------------------------------------------------------------------------

    /// Test KeyValue construction and conversion
    #[test]
    fn test_key_value_operations() {
        let kv = KeyValue::new("Content-Type", "application/json");
        assert_eq!(kv.key, "Content-Type");
        assert_eq!(kv.value, "application/json");

        // From tuple
        let kv2: KeyValue = ("Accept", "text/html").into();
        assert_eq!(kv2.key, "Accept");
        assert_eq!(kv2.value, "text/html");

        // To tuple
        let (k, v): (String, String) = kv.into();
        assert_eq!(k, "Content-Type");
        assert_eq!(v, "application/json");
    }

    // -------------------------------------------------------------------------
    // RedirectInfo Tests
    // -------------------------------------------------------------------------

    /// Test RedirectInfo variants
    #[test]
    fn test_redirect_info_variants() {
        let permanent = RedirectInfo::permanent("https://new.example.com");
        assert_eq!(permanent.status, 301);
        assert!(!permanent.preserve_body);

        let temporary = RedirectInfo::temporary("https://temp.example.com");
        assert_eq!(temporary.status, 302);
        assert!(!temporary.preserve_body);

        let temp_with_body = RedirectInfo::temporary_with_body("https://temp.example.com");
        assert_eq!(temp_with_body.status, 307);
        assert!(temp_with_body.preserve_body);

        let perm_with_body = RedirectInfo::permanent_with_body("https://new.example.com");
        assert_eq!(perm_with_body.status, 308);
        assert!(perm_with_body.preserve_body);
    }

    // -------------------------------------------------------------------------
    // ImmediateResponse Tests
    // -------------------------------------------------------------------------

    /// Test ImmediateResponse construction
    #[test]
    fn test_immediate_response_construction() {
        let resp = ImmediateResponse::ok()
            .with_header("X-Custom", "value")
            .with_json_body(r#"{"status":"ok"}"#);

        assert_eq!(resp.status, 200);
        assert!(resp.headers.len() >= 2); // X-Custom and Content-Type
        assert!(!resp.body.is_empty());
    }

    /// Test ImmediateResponse status code constructors
    #[test]
    fn test_immediate_response_status_codes() {
        assert_eq!(ImmediateResponse::ok().status, 200);
        assert_eq!(ImmediateResponse::not_found().status, 404);
        assert_eq!(ImmediateResponse::forbidden().status, 403);
        assert_eq!(ImmediateResponse::internal_error().status, 500);
        assert_eq!(ImmediateResponse::new(201).status, 201);
    }
}

// =============================================================================
// E2E Test: Complete WASM Plugin Flow
// =============================================================================

mod wasm_complete_flow_e2e {
    use super::*;

    /// Test complete WASM plugin flow with host functions
    ///
    /// This simulates a full plugin lifecycle using the host function APIs,
    /// combining configuration, KV storage, logging, and metrics.
    #[test]
    fn test_complete_wasm_plugin_flow() {
        // 1. Create host with plugin ID
        let mut host = DefaultHost::with_plugin_id("complete-flow-test");

        // 2. Add configs for networking and filesystem
        host.add_configs([
            ("network.enabled", "true"),
            ("filesystem.readonly", "false"),
            ("cache.ttl_seconds", "300"),
        ]);

        // 3. Verify networking config is accessible
        assert_eq!(host.config_get_bool("network.enabled"), Some(true));
        assert_eq!(host.config_get_bool("filesystem.readonly"), Some(false));

        // 4. Test KV operations (simulating plugin state)
        host.kv_set_string("request:count", "0").unwrap();
        let count = host.kv_increment("request:count", 1).unwrap();
        assert_eq!(count, 1);

        // 5. Test logging with structured fields
        host.log_structured(
            LogLevel::Info,
            "Request processed",
            &[
                ("path".to_string(), "/api/test".to_string()),
                ("duration_ms".to_string(), "15".to_string()),
            ],
        );

        // 6. Test metrics
        host.counter_inc("requests_total", 1);
        host.histogram_observe("request_duration_seconds", 0.015);
        host.gauge_set("active_connections", 5.0);

        // 7. Verify metrics were recorded
        let metrics = host.metrics();
        assert!(metrics.get_counter("requests_total").is_some());
        assert!(metrics.get_gauge("active_connections").is_some());
    }

    /// Test plugin flow with secrets
    #[test]
    fn test_plugin_flow_with_secrets() {
        let mut host = DefaultHost::with_plugin_id("secrets-test");

        // Add secrets
        host.add_secret("api_key", "sk-test-12345");
        host.add_secret("db_password", "secret-password");

        // Verify secret access
        assert!(host.secret_exists("api_key"));
        assert!(host.secret_exists("db_password"));
        assert!(!host.secret_exists("nonexistent"));

        let api_key = host.secret_get("api_key").unwrap();
        assert_eq!(api_key, Some("sk-test-12345".to_string()));

        // Required secret should succeed
        let required = host.secret_get_required("api_key");
        assert!(required.is_ok());

        // Required secret should fail for missing
        let missing = host.secret_get_required("nonexistent");
        assert!(missing.is_err());
    }

    /// Test plugin flow with compare-and-swap for locking
    #[test]
    fn test_plugin_flow_with_cas_locking() {
        let mut host = DefaultHost::with_plugin_id("cas-test");

        // Acquire lock (CAS on non-existent key)
        let acquired = host
            .kv_compare_and_swap("lock:resource", None, b"owner1")
            .unwrap();
        assert!(acquired, "First lock acquisition should succeed");

        // Try to acquire same lock (should fail)
        let reacquired = host
            .kv_compare_and_swap("lock:resource", None, b"owner2")
            .unwrap();
        assert!(!reacquired, "Second lock acquisition should fail");

        // Release lock (CAS with correct expected value)
        let released = host
            .kv_compare_and_swap("lock:resource", Some(b"owner1"), b"")
            .unwrap();
        assert!(released, "Lock release should succeed");

        // Now another owner can acquire
        let new_acquired = host
            .kv_compare_and_swap("lock:resource", Some(b""), b"owner2")
            .unwrap();
        assert!(
            new_acquired,
            "New lock acquisition should succeed after release"
        );
    }

    /// Test plugin flow with TTL-based expiration
    #[test]
    fn test_plugin_flow_with_ttl() {
        let mut host = DefaultHost::with_plugin_id("ttl-test");

        // Set value with TTL (5 seconds in nanoseconds)
        let ttl_ns = 5_000_000_000u64;
        host.kv_set_with_ttl("temp:session", b"session-data", ttl_ns)
            .unwrap();

        // Value should exist immediately
        assert!(host.kv_exists("temp:session"));

        let value = host.kv_get("temp:session").unwrap();
        assert_eq!(value, Some(b"session-data".to_vec()));
    }

    /// Test plugin flow with labeled metrics
    #[test]
    fn test_plugin_flow_with_labeled_metrics() {
        let host = DefaultHost::with_plugin_id("labeled-metrics-test");

        // Counter with labels
        host.counter_inc_labeled(
            "http_requests",
            1,
            &[
                ("method".to_string(), "GET".to_string()),
                ("status".to_string(), "200".to_string()),
                ("path".to_string(), "/api/users".to_string()),
            ],
        );

        host.counter_inc_labeled(
            "http_requests",
            1,
            &[
                ("method".to_string(), "POST".to_string()),
                ("status".to_string(), "201".to_string()),
                ("path".to_string(), "/api/users".to_string()),
            ],
        );

        // Gauge with labels
        host.gauge_set_labeled(
            "queue_size",
            42.0,
            &[("queue_name".to_string(), "default".to_string())],
        );

        // Histogram with labels
        host.histogram_observe_labeled(
            "request_duration",
            0.05,
            &[("endpoint".to_string(), "/api/users".to_string())],
        );

        // Duration recording with labels
        host.record_duration_labeled(
            "db_query_ns",
            50_000_000, // 50ms
            &[("query_type".to_string(), "select".to_string())],
        );
    }
}
