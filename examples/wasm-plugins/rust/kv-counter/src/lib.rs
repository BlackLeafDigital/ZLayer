//! A key-value counter plugin demonstrating ZLayer host capabilities.
//!
//! This plugin shows how to use ZLayer's host interfaces:
//! - **keyvalue**: Persistent storage for counter state
//! - **logging**: Structured logging with levels and fields
//! - **metrics**: Counter and gauge metrics for observability
//!
//! # Building
//!
//! ```bash
//! cargo component build --release
//! ```
//!
//! # Endpoints
//!
//! - `GET /counter` - Get the current counter value
//! - `POST /counter/increment` - Increment the counter by 1 (or by body value)
//! - `POST /counter/decrement` - Decrement the counter by 1 (or by body value)
//! - `POST /counter/reset` - Reset the counter to 0
//! - `GET /stats` - Get request statistics

#![no_std]
extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use zlayer_sdk::bindings::{
    export_handler,
    exports::zlayer::plugin::handler::{
        Capabilities, Guest, HandleResult, InitError, PluginInfo, PluginRequest, PluginResponse,
        Version,
    },
    zlayer::plugin::{
        common::KeyValue,
        config,
        keyvalue,
        logging::{self, Level},
        metrics,
    },
};

/// The KV counter bucket name for namespacing our keys.
const BUCKET: &str = "kv-counter";

/// Counter plugin that tracks request counts using ZLayer's key-value storage.
struct KvCounterPlugin;

impl Guest for KvCounterPlugin {
    /// Initialize the plugin.
    ///
    /// Sets up initial counter values in the key-value store if they don't exist.
    fn init() -> Result<Capabilities, InitError> {
        logging::info("KV Counter plugin initializing...");

        // Log configuration if present
        if let Some(initial_value) = config::get("initial_value") {
            logging::log_structured(
                Level::Info,
                "Custom initial value configured",
                &[KeyValue {
                    key: String::from("initial_value"),
                    value: initial_value,
                }],
            );
        }

        // Initialize the counter if it doesn't exist
        let counter_key = format!("{}/counter", BUCKET);
        if !keyvalue::exists(&counter_key) {
            let initial = config::get_int("initial_value").unwrap_or(0);
            if let Err(e) = keyvalue::set_string(&counter_key, &format!("{}", initial)) {
                logging::error(&format!("Failed to initialize counter: {:?}", e));
                return Err(InitError::Failed(String::from(
                    "Failed to initialize counter storage",
                )));
            }
            logging::info(&format!("Counter initialized to {}", initial));
        }

        // Initialize request statistics
        let stats_key = format!("{}/total_requests", BUCKET);
        if !keyvalue::exists(&stats_key) {
            let _ = keyvalue::set_string(&stats_key, "0");
        }

        logging::info("KV Counter plugin initialized successfully");

        // Request capabilities we need
        Ok(Capabilities::CONFIG | Capabilities::KEYVALUE | Capabilities::LOGGING | Capabilities::METRICS)
    }

    /// Return plugin metadata.
    fn info() -> PluginInfo {
        PluginInfo {
            id: String::from("zlayer:kv-counter"),
            name: String::from("KV Counter Plugin"),
            version: Version {
                major: 0,
                minor: 1,
                patch: 0,
                pre_release: None,
            },
            description: String::from(
                "A counter plugin demonstrating ZLayer keyvalue, logging, and metrics interfaces",
            ),
            author: String::from("ZLayer Team"),
            license: Some(String::from("MIT")),
            homepage: Some(String::from("https://github.com/BlackLeafDigital/zlayer")),
            metadata: Vec::new(),
        }
    }

    /// Handle an incoming request.
    fn handle(request: PluginRequest) -> HandleResult {
        // Log the request
        logging::log_structured(
            Level::Debug,
            "Handling request",
            &[
                KeyValue {
                    key: String::from("request_id"),
                    value: request.request_id.clone(),
                },
                KeyValue {
                    key: String::from("path"),
                    value: request.path.clone(),
                },
                KeyValue {
                    key: String::from("method"),
                    value: format!("{:?}", request.method),
                },
            ],
        );

        // Record metrics
        metrics::counter_inc("kv_counter_requests_total", 1);

        // Increment total request count in storage
        let stats_key = format!("{}/total_requests", BUCKET);
        let _ = keyvalue::increment(&stats_key, 1);

        // Route based on path and method
        match (request.method, request.path.as_str()) {
            (zlayer_sdk::bindings::exports::zlayer::plugin::handler::HttpMethod::Get, "/counter") => {
                handle_get_counter()
            }
            (zlayer_sdk::bindings::exports::zlayer::plugin::handler::HttpMethod::Post, "/counter/increment") => {
                handle_increment(&request)
            }
            (zlayer_sdk::bindings::exports::zlayer::plugin::handler::HttpMethod::Post, "/counter/decrement") => {
                handle_decrement(&request)
            }
            (zlayer_sdk::bindings::exports::zlayer::plugin::handler::HttpMethod::Post, "/counter/reset") => {
                handle_reset()
            }
            (zlayer_sdk::bindings::exports::zlayer::plugin::handler::HttpMethod::Get, "/stats") => {
                handle_get_stats()
            }
            _ => {
                logging::debug(&format!("Unknown path: {}", request.path));
                HandleResult::PassThrough
            }
        }
    }

    /// Graceful shutdown hook.
    fn shutdown() {
        logging::info("KV Counter plugin shutting down...");
        // Log final statistics
        if let Ok(Some(count)) = keyvalue::get_string(&format!("{}/total_requests", BUCKET)) {
            logging::info(&format!("Total requests handled: {}", count));
        }
        logging::info("KV Counter plugin shutdown complete");
    }
}

/// Handle GET /counter - returns current counter value.
fn handle_get_counter() -> HandleResult {
    let counter_key = format!("{}/counter", BUCKET);

    match keyvalue::get_string(&counter_key) {
        Ok(Some(value)) => {
            let json = format!(r#"{{"counter":{}}}"#, value);
            logging::trace(&format!("Counter value: {}", value));

            // Update gauge metric
            if let Ok(v) = value.parse::<f64>() {
                metrics::gauge_set("kv_counter_value", v);
            }

            json_response(200, &json)
        }
        Ok(None) => {
            logging::warn("Counter not found in storage");
            json_response(404, r#"{"error":"Counter not initialized"}"#)
        }
        Err(e) => {
            logging::error(&format!("Failed to get counter: {:?}", e));
            json_response(500, r#"{"error":"Storage error"}"#)
        }
    }
}

/// Handle POST /counter/increment - increments the counter.
fn handle_increment(request: &PluginRequest) -> HandleResult {
    // Parse increment amount from body, default to 1
    let delta = parse_body_as_i64(&request.body).unwrap_or(1);

    let counter_key = format!("{}/counter", BUCKET);
    match keyvalue::increment(&counter_key, delta) {
        Ok(new_value) => {
            logging::info(&format!("Counter incremented by {} to {}", delta, new_value));
            metrics::counter_inc("kv_counter_increments", 1);
            metrics::gauge_set("kv_counter_value", new_value as f64);

            let json = format!(r#"{{"counter":{},"delta":{}}}"#, new_value, delta);
            json_response(200, &json)
        }
        Err(e) => {
            logging::error(&format!("Failed to increment counter: {:?}", e));
            json_response(500, r#"{"error":"Failed to increment counter"}"#)
        }
    }
}

/// Handle POST /counter/decrement - decrements the counter.
fn handle_decrement(request: &PluginRequest) -> HandleResult {
    // Parse decrement amount from body, default to 1
    let delta = parse_body_as_i64(&request.body).unwrap_or(1);

    let counter_key = format!("{}/counter", BUCKET);
    match keyvalue::increment(&counter_key, -delta) {
        Ok(new_value) => {
            logging::info(&format!("Counter decremented by {} to {}", delta, new_value));
            metrics::counter_inc("kv_counter_decrements", 1);
            metrics::gauge_set("kv_counter_value", new_value as f64);

            let json = format!(r#"{{"counter":{},"delta":{}}}"#, new_value, -delta);
            json_response(200, &json)
        }
        Err(e) => {
            logging::error(&format!("Failed to decrement counter: {:?}", e));
            json_response(500, r#"{"error":"Failed to decrement counter"}"#)
        }
    }
}

/// Handle POST /counter/reset - resets the counter to 0.
fn handle_reset() -> HandleResult {
    let counter_key = format!("{}/counter", BUCKET);

    match keyvalue::set_string(&counter_key, "0") {
        Ok(()) => {
            logging::info("Counter reset to 0");
            metrics::counter_inc("kv_counter_resets", 1);
            metrics::gauge_set("kv_counter_value", 0.0);

            json_response(200, r#"{"counter":0,"reset":true}"#)
        }
        Err(e) => {
            logging::error(&format!("Failed to reset counter: {:?}", e));
            json_response(500, r#"{"error":"Failed to reset counter"}"#)
        }
    }
}

/// Handle GET /stats - returns request statistics.
fn handle_get_stats() -> HandleResult {
    let counter_key = format!("{}/counter", BUCKET);
    let stats_key = format!("{}/total_requests", BUCKET);

    let counter_value = keyvalue::get_string(&counter_key)
        .ok()
        .flatten()
        .unwrap_or_else(|| String::from("0"));
    let total_requests = keyvalue::get_string(&stats_key)
        .ok()
        .flatten()
        .unwrap_or_else(|| String::from("0"));

    let json = format!(
        r#"{{"counter":{},"total_requests":{},"plugin":"kv-counter"}}"#,
        counter_value, total_requests
    );

    json_response(200, &json)
}

/// Parse request body as an i64 integer.
fn parse_body_as_i64(body: &[u8]) -> Option<i64> {
    core::str::from_utf8(body)
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// Create a JSON response.
fn json_response(status: u16, body: &str) -> HandleResult {
    HandleResult::Response(PluginResponse {
        status,
        headers: alloc::vec![KeyValue {
            key: String::from("Content-Type"),
            value: String::from("application/json"),
        }],
        body: body.as_bytes().to_vec(),
    })
}

export_handler!(KvCounterPlugin);
