//! A simple hello world ZLayer plugin.
//!
//! This example demonstrates the core ZLayer plugin interface:
//! - Plugin initialization with configuration access
//! - Returning plugin metadata
//! - Handling incoming requests
//! - Graceful shutdown
//!
//! # Building
//!
//! ```bash
//! cargo component build --release
//! ```
//!
//! # Configuration
//!
//! The plugin accepts these configuration options:
//! - `greeting`: Custom greeting message (default: "Hello")
//!
//! # Events
//!
//! The plugin responds to these request paths:
//! - `/greet` (POST): Returns a personalized greeting using the request body as the name
//! - `/echo` (POST): Echoes back the request body
//! - `/info` (GET): Returns plugin information as JSON

#![no_std]
extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// Import the ZLayer SDK bindings
// These are generated from the WIT definitions and provide type-safe
// access to host functions and the handler interface.
use zlayer_sdk::bindings::{
    export_handler,
    exports::zlayer::plugin::handler::{
        Capabilities, Guest, HandleResult, InitError, PluginInfo, PluginRequest,
    },
    zlayer::plugin::{
        common::KeyValue,
        config,
        logging::{self, Level},
        plugin_metadata::Version,
        request_types::PluginResponse,
    },
};

/// Hello world plugin implementation.
///
/// This plugin demonstrates the basic structure of a ZLayer plugin:
/// 1. Configuration access during initialization
/// 2. Metadata reporting
/// 3. Request handling with multiple endpoints
/// 4. Structured logging throughout
struct HelloPlugin;

impl Guest for HelloPlugin {
    /// Initialize the plugin.
    ///
    /// Called once when the plugin is loaded. This is where you should:
    /// - Read and validate configuration
    /// - Initialize any internal state
    /// - Request capabilities the plugin needs
    ///
    /// Returns the capabilities the plugin requires to function.
    fn init() -> Result<Capabilities, InitError> {
        logging::info("Hello plugin initializing...");

        // Read optional greeting configuration
        // This demonstrates accessing the config host interface
        if let Some(greeting) = config::get("greeting") {
            logging::info(&format!("Custom greeting configured: {}", greeting));
        } else {
            logging::debug("Using default greeting: Hello");
        }

        // Check for optional debug mode
        if config::get_bool("debug").unwrap_or(false) {
            logging::info("Debug mode enabled");
        }

        logging::info("Hello plugin initialized successfully");

        // Request only the capabilities we need:
        // - CONFIG: To read the greeting configuration
        // - LOGGING: To emit log messages
        Ok(Capabilities::CONFIG | Capabilities::LOGGING)
    }

    /// Return plugin metadata.
    ///
    /// Called after successful initialization. This information is used
    /// for logging, metrics, management UI, and plugin discovery.
    fn info() -> PluginInfo {
        PluginInfo {
            id: String::from("zlayer:hello"),
            name: String::from("Hello World Plugin"),
            version: Version {
                major: 0,
                minor: 1,
                patch: 0,
                pre_release: None,
            },
            description: String::from(
                "A hello world plugin demonstrating ZLayer SDK capabilities",
            ),
            author: String::from("ZLayer Team"),
            license: Some(String::from("MIT")),
            homepage: Some(String::from("https://github.com/BlackLeafDigital/zlayer")),
            metadata: Vec::new(),
        }
    }

    /// Handle an incoming request.
    ///
    /// This is the main entry point for request processing. The plugin receives
    /// a `PluginRequest` containing the HTTP method, path, headers, and body,
    /// and returns a `HandleResult` that can be:
    /// - `Response`: Plugin handled the request, return the response
    /// - `PassThrough`: Plugin declined to handle, continue to next handler
    /// - `Error`: Plugin encountered an error
    fn handle(request: PluginRequest) -> HandleResult {
        // Log the incoming request with structured data
        logging::log_structured(
            Level::Debug,
            "Received request",
            &[
                KeyValue {
                    key: String::from("request_id"),
                    value: request.request_id.clone(),
                },
                KeyValue {
                    key: String::from("path"),
                    value: request.path.clone(),
                },
            ],
        );

        // Route based on path
        match request.path.as_str() {
            "/greet" => handle_greet(&request),
            "/echo" => handle_echo(&request),
            "/info" => handle_info(),
            _ => {
                // Unknown path - pass through to next handler
                logging::debug(&format!("Unknown path: {}, passing through", request.path));
                HandleResult::PassThrough
            }
        }
    }

    /// Graceful shutdown hook.
    ///
    /// Called when the plugin is being unloaded. Use this to:
    /// - Flush any pending data
    /// - Close connections
    /// - Clean up resources
    ///
    /// Note: This is a best-effort call. Plugins may be terminated
    /// forcefully if they don't respond in time.
    fn shutdown() {
        logging::info("Hello plugin shutting down...");
        // In a real plugin, you would clean up resources here:
        // - Close database connections
        // - Flush caches
        // - Cancel pending operations
        logging::info("Hello plugin shutdown complete");
    }
}

/// Handle the /greet endpoint.
///
/// Expects a POST request with a name in the body.
/// Returns a personalized greeting using the configured greeting prefix.
fn handle_greet(request: &PluginRequest) -> HandleResult {
    // Parse the request body as a name
    let name = match core::str::from_utf8(&request.body) {
        Ok(s) if !s.is_empty() => s,
        Ok(_) => {
            logging::warn("Empty name in greet request");
            return HandleResult::Response(PluginResponse {
                status: 400,
                headers: Vec::new(),
                body: b"Name is required".to_vec(),
            });
        }
        Err(e) => {
            logging::error(&format!("Invalid UTF-8 in request body: {}", e));
            return HandleResult::Error(format!("Invalid UTF-8: {}", e));
        }
    };

    // Get the greeting from config or use default
    let greeting = config::get("greeting").unwrap_or_else(|| String::from("Hello"));

    // Build the response
    let response_text = format!("{}, {}!", greeting, name);
    logging::info(&response_text);

    HandleResult::Response(PluginResponse {
        status: 200,
        headers: vec![KeyValue {
            key: String::from("Content-Type"),
            value: String::from("text/plain; charset=utf-8"),
        }],
        body: response_text.into_bytes(),
    })
}

/// Handle the /echo endpoint.
///
/// Simply echoes back the request body. Useful for testing.
fn handle_echo(request: &PluginRequest) -> HandleResult {
    logging::trace("Echo request received");

    HandleResult::Response(PluginResponse {
        status: 200,
        headers: vec![KeyValue {
            key: String::from("Content-Type"),
            value: String::from("application/octet-stream"),
        }],
        body: request.body.clone(),
    })
}

/// Handle the /info endpoint.
///
/// Returns plugin information as JSON.
fn handle_info() -> HandleResult {
    logging::trace("Info request received");

    let info_json = r#"{"name":"Hello World Plugin","version":"0.1.0","description":"A hello world plugin demonstrating ZLayer SDK capabilities"}"#;

    HandleResult::Response(PluginResponse {
        status: 200,
        headers: vec![KeyValue {
            key: String::from("Content-Type"),
            value: String::from("application/json"),
        }],
        body: info_json.as_bytes().to_vec(),
    })
}

// Register the plugin implementation with the ZLayer runtime.
// This macro generates the necessary FFI exports that allow the
// WebAssembly runtime to call into our plugin.
// The `with_types_in` parameter tells wit-bindgen where to find the generated types.
export_handler!(HelloPlugin with_types_in zlayer_sdk::bindings);
