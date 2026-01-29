# ZLayer WASM Plugin Development Guide

This guide covers building WASM plugins for ZLayer using the WIT-based component model.

## Overview

ZLayer WASM plugins are WebAssembly Components that implement one or more WIT interfaces. Plugins can:

- Handle HTTP requests (incoming-handler)
- Make HTTP client calls (outgoing-handler)
- Route requests to backends (router)
- Intercept request/response (middleware)
- Handle WebSocket connections
- Control response caching
- Perform authentication
- Implement rate limiting

## Prerequisites

- Rust 1.75+ with `wasm32-wasip2` target
- `wit-bindgen` for generating bindings
- `wasm-tools` for component manipulation

```bash
# Install tools
rustup target add wasm32-wasip2
cargo install wit-bindgen-cli wasm-tools
```

## Project Structure

```
my-plugin/
├── Cargo.toml
├── src/
│   └── lib.rs
└── wit/
    └── deps/
        └── zlayer/     # Symlink or copy from ZLayer wit/zlayer/
```

## Available Plugin Worlds

ZLayer defines several plugin worlds for different use cases:

| World | Description | Host Interfaces |
|-------|-------------|-----------------|
| `zlayer-plugin` | Full-featured plugins | config, keyvalue, logging, secrets, metrics |
| `zlayer-http-handler` | HTTP request handlers | config, keyvalue, logging |
| `zlayer-transformer` | Simple transformations | logging |
| `zlayer-authenticator` | Authentication plugins | config, keyvalue, logging, secrets |
| `zlayer-rate-limiter` | Rate limiting plugins | config, keyvalue, logging, metrics |
| `zlayer-middleware` | Request/response middleware | config, logging |
| `zlayer-router` | Custom routing logic | config, keyvalue, logging |

## Example: HTTP Handler Plugin

The simplest plugin type - handles incoming HTTP requests.

### Cargo.toml

```toml
[package]
name = "my-handler"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
wit-bindgen = "0.36"

[package.metadata.component]
package = "myorg:my-handler"
```

### src/lib.rs

```rust
wit_bindgen::generate!({
    world: "zlayer-http-handler",
    path: "../wit",
});

use exports::wasi::http::incoming_handler::Guest;
use wasi::http::types::{
    Fields, IncomingRequest, OutgoingBody, OutgoingResponse, ResponseOutparam,
};

struct MyHandler;

impl Guest for MyHandler {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        // Read request info
        let method = request.method();
        let path = request.path_with_query().unwrap_or_default();

        // Create response headers
        let headers = Fields::new();
        headers
            .append(&"content-type".to_string(), &b"application/json".to_vec())
            .unwrap();

        // Create response
        let response = OutgoingResponse::new(headers);
        response.set_status_code(200).unwrap();

        // Write body
        let body = response.body().unwrap();
        let stream = body.write().unwrap();
        let json = format!(r#"{{"method":"{:?}","path":"{}"}}"#, method, path);
        stream.blocking_write_and_flush(json.as_bytes()).unwrap();
        drop(stream);
        OutgoingBody::finish(body, None).unwrap();

        // Send response
        ResponseOutparam::set(response_out, Ok(response));
    }
}

export!(MyHandler);
```

### Build and Push

```bash
cargo build --target wasm32-wasip2 --release
zlayer wasm push target/wasm32-wasip2/release/my_handler.wasm ghcr.io/myorg/handler:v1
```

## Using ZLayer Host Interfaces

### Configuration Access

The `config` interface provides typed access to configuration values:

```rust
use crate::config;

// Get optional config value
let api_key = config::get("api_key");

// Get required config (returns Result)
let db_url = config::get_required("database_url")?;

// Get all keys with prefix
let db_configs = config::get_prefix("database.");
// Returns: [("database.host", "localhost"), ("database.port", "5432"), ...]

// Typed getters
let debug_mode = config::get_bool("debug").unwrap_or(false);
let max_connections = config::get_int("max_connections").unwrap_or(100);
let timeout_secs = config::get_float("timeout_seconds").unwrap_or(30.0);

// Check if key exists
if config::exists("feature_flags.new_ui") {
    // Enable new UI
}
```

### Key-Value Storage

The `keyvalue` interface provides persistent storage with TTL support:

```rust
use crate::keyvalue;

// Store binary data
keyvalue::set("user:123", b"{\"name\":\"Alice\"}")?;

// Store string data
keyvalue::set_string("session:abc", "user_id=123")?;

// Retrieve data
if let Some(user_data) = keyvalue::get("user:123")? {
    // Process user data
}

// Get as string
if let Some(session) = keyvalue::get_string("session:abc")? {
    // Process session
}

// Set with TTL (1 hour = 3600 seconds = 3600_000_000_000 nanoseconds)
keyvalue::set_with_ttl("cache:key", data, 3600_000_000_000)?;

// Atomic increment (returns new value)
let count = keyvalue::increment("request_count", 1)?;

// Compare and swap for optimistic locking
let swapped = keyvalue::compare_and_swap(
    "lock:resource",
    None,                    // Expected: key doesn't exist
    b"locked".to_vec(),      // New value
)?;

// List keys with prefix
let user_keys = keyvalue::list_keys("user:")?;

// Delete a key (returns true if existed)
let existed = keyvalue::delete("user:123")?;
```

### Structured Logging

The `logging` interface provides leveled, structured logging:

```rust
use crate::logging::{self, Level};

// Simple logging
logging::trace("Entering function");
logging::debug("Processing request");
logging::info("Request completed");
logging::warn("Deprecated API used");
logging::error("Failed to connect to database");

// Generic log with level
logging::log(Level::Info, "Custom message");

// Structured logging with fields
logging::log_structured(
    Level::Info,
    "Request processed",
    &[
        KeyValue { key: "user_id".to_string(), value: "123".to_string() },
        KeyValue { key: "path".to_string(), value: "/api/users".to_string() },
        KeyValue { key: "duration_ms".to_string(), value: "45".to_string() },
    ],
);

// Check if level is enabled (for expensive log construction)
if logging::is_enabled(Level::Debug) {
    let expensive_debug_info = compute_debug_info();
    logging::debug(&expensive_debug_info);
}
```

### Secrets Management

The `secrets` interface provides secure access to sensitive values:

```rust
use crate::secrets;

// Get optional secret
if let Some(api_key) = secrets::get("api_key")? {
    // Use API key
}

// Get required secret
let db_password = secrets::get_required("database_password")?;

// Check if secret exists
if secrets::exists("stripe_key") {
    // Stripe integration is configured
}

// List available secret names (for diagnostics)
let available_secrets = secrets::list_names();
// Returns: ["api_key", "database_password", "stripe_key"]
```

### Metrics

The `metrics` interface provides observability primitives:

```rust
use crate::metrics;

// Counter - track occurrences
metrics::counter_inc("requests_total", 1);

// Counter with labels
metrics::counter_inc_labeled(
    "requests_by_method",
    1,
    &[
        KeyValue { key: "method".to_string(), value: "GET".to_string() },
        KeyValue { key: "status".to_string(), value: "200".to_string() },
    ],
);

// Gauge - track current values
metrics::gauge_set("active_connections", 42.0);
metrics::gauge_add("queue_size", 1.0);   // Add
metrics::gauge_add("queue_size", -1.0);  // Subtract

// Gauge with labels
metrics::gauge_set_labeled(
    "memory_usage_bytes",
    1024.0 * 1024.0 * 512.0,
    &[KeyValue { key: "type".to_string(), value: "heap".to_string() }],
);

// Histogram - track distributions
metrics::histogram_observe("request_duration_seconds", 0.025);

// Histogram with labels
metrics::histogram_observe_labeled(
    "http_request_duration_seconds",
    0.045,
    &[
        KeyValue { key: "method".to_string(), value: "POST".to_string() },
        KeyValue { key: "path".to_string(), value: "/api/users".to_string() },
    ],
);

// Convenience method for request duration (nanoseconds)
let start = get_monotonic_time();
// ... process request ...
let duration_ns = get_monotonic_time() - start;
metrics::record_duration("request_processing", duration_ns);

// With labels
metrics::record_duration_labeled(
    "request_processing",
    duration_ns,
    &[KeyValue { key: "handler".to_string(), value: "user_create".to_string() }],
);
```

## Advanced Interfaces

### Full Plugin Handler

For plugins that need lifecycle management and capability negotiation:

```rust
wit_bindgen::generate!({
    world: "zlayer-plugin",
    path: "../wit",
});

use crate::exports::handler::{Capabilities, Guest, InitError, PluginInfo, Version};
use crate::request_types::{HandleResult, PluginRequest, PluginResponse};

struct MyPlugin;

impl Guest for MyPlugin {
    fn init() -> Result<Capabilities, InitError> {
        // Check required configuration
        if crate::config::get("api_key").is_none() {
            return Err(InitError::ConfigMissing("api_key".to_string()));
        }

        // Return capabilities we need
        Ok(Capabilities::CONFIG | Capabilities::KEYVALUE | Capabilities::LOGGING)
    }

    fn info() -> PluginInfo {
        PluginInfo {
            id: "myorg:my-plugin".to_string(),
            name: "My Plugin".to_string(),
            version: Version {
                major: 1,
                minor: 0,
                patch: 0,
                pre_release: None,
            },
            description: "A sample ZLayer plugin".to_string(),
            author: "My Organization".to_string(),
            license: Some("MIT".to_string()),
            homepage: Some("https://github.com/myorg/my-plugin".to_string()),
            metadata: vec![],
        }
    }

    fn handle(request: PluginRequest) -> HandleResult {
        // Log the request
        crate::logging::info(&format!("Handling request: {} {}", request.method, request.path));

        // Check if we should handle this request
        if !request.path.starts_with("/api/") {
            return HandleResult::PassThrough;
        }

        // Process the request
        match process_request(&request) {
            Ok(body) => HandleResult::Response(PluginResponse {
                status: 200,
                headers: vec![
                    KeyValue {
                        key: "content-type".to_string(),
                        value: "application/json".to_string(),
                    },
                ],
                body,
            }),
            Err(e) => HandleResult::Error(e.to_string()),
        }
    }

    fn shutdown() {
        crate::logging::info("Plugin shutting down");
        // Cleanup resources
    }
}

export!(MyPlugin);
```

### Router Plugin

Implement custom request routing logic:

```rust
wit_bindgen::generate!({
    world: "zlayer-router",
    path: "../wit",
});

use crate::exports::router::Guest;
use crate::http_types::{RequestMetadata, RoutingDecision, Upstream};
use crate::request_types::PluginRequest;

struct MyRouter;

impl Guest for MyRouter {
    fn route(request: PluginRequest, metadata: RequestMetadata) -> RoutingDecision {
        // Route based on path
        if request.path.starts_with("/api/v2") {
            RoutingDecision::Forward(Upstream {
                host: "api-v2.internal".to_string(),
                port: 8080,
                tls: false,
                connect_timeout: 5_000_000_000,   // 5s in nanoseconds
                request_timeout: 30_000_000_000,  // 30s in nanoseconds
            })
        } else if request.path.starts_with("/api/v1") {
            RoutingDecision::Forward(Upstream {
                host: "api-v1.internal".to_string(),
                port: 8080,
                tls: false,
                connect_timeout: 5_000_000_000,
                request_timeout: 30_000_000_000,
            })
        } else if request.path == "/old-endpoint" {
            // Redirect to new location
            RoutingDecision::Redirect(RedirectInfo {
                location: "/api/v2/new-endpoint".to_string(),
                status: 308,  // Permanent redirect, preserve method
                preserve_body: true,
            })
        } else {
            // Let other handlers process
            RoutingDecision::ContinueProcessing
        }
    }
}

export!(MyRouter);
```

### Middleware Plugin

Intercept and modify requests/responses:

```rust
wit_bindgen::generate!({
    world: "zlayer-middleware",
    path: "../wit",
});

use crate::exports::middleware::{Guest, MiddlewareAction};
use crate::common::KeyValue;
use crate::http_types::RequestMetadata;

struct MyMiddleware;

impl Guest for MyMiddleware {
    fn on_request(
        method: String,
        path: String,
        headers: Vec<KeyValue>,
        metadata: RequestMetadata,
    ) -> MiddlewareAction {
        // Add tracing header
        let mut new_headers = headers;
        new_headers.push(KeyValue {
            key: "x-trace-id".to_string(),
            value: generate_trace_id(),
        });

        // Add client IP header
        new_headers.push(KeyValue {
            key: "x-forwarded-for".to_string(),
            value: metadata.client_ip.clone(),
        });

        // Check for blocked paths
        if path.starts_with("/admin") {
            // Check authorization
            let has_auth = headers.iter().any(|h| h.key == "authorization");
            if !has_auth {
                return MiddlewareAction::Abort(401, "Unauthorized".to_string());
            }
        }

        MiddlewareAction::ContinueWith(new_headers)
    }

    fn on_response(status: u16, headers: Vec<KeyValue>) -> MiddlewareAction {
        let mut new_headers = headers;

        // Add security headers
        new_headers.push(KeyValue {
            key: "x-content-type-options".to_string(),
            value: "nosniff".to_string(),
        });
        new_headers.push(KeyValue {
            key: "x-frame-options".to_string(),
            value: "DENY".to_string(),
        });

        // Log errors
        if status >= 500 {
            crate::logging::error(&format!("Server error: {}", status));
        }

        MiddlewareAction::ContinueWith(new_headers)
    }
}

fn generate_trace_id() -> String {
    // Generate a unique trace ID
    format!("{:016x}", crate::random::get_random_u64())
}

export!(MyMiddleware);
```

### Authentication Plugin

Implement custom authentication logic:

```rust
wit_bindgen::generate!({
    world: "zlayer-authenticator",
    path: "../wit",
});

use crate::exports::authenticator::{AuthResult, Guest};
use crate::common::{Error, KeyValue};
use crate::request_types::PluginRequest;

struct JwtAuthenticator;

impl Guest for JwtAuthenticator {
    fn authenticate(request: PluginRequest) -> AuthResult {
        // Find Authorization header
        let auth_header = request.headers.iter().find(|h| h.key.to_lowercase() == "authorization");

        match auth_header {
            None => {
                // No credentials, challenge for them
                AuthResult::Challenge("Bearer realm=\"api\"".to_string())
            }
            Some(header) => {
                // Parse Bearer token
                if !header.value.starts_with("Bearer ") {
                    return AuthResult::Denied("Invalid authorization scheme".to_string());
                }

                let token = &header.value[7..];

                // Validate the token
                match validate_jwt(token) {
                    Ok(claims) => AuthResult::Authenticated(claims),
                    Err(e) => AuthResult::Denied(e),
                }
            }
        }
    }

    fn validate_token(token: String) -> Result<Vec<KeyValue>, Error> {
        validate_jwt(&token).map_err(|msg| Error {
            code: "INVALID_TOKEN".to_string(),
            message: msg,
        })
    }
}

fn validate_jwt(token: &str) -> Result<Vec<KeyValue>, String> {
    // Get the JWT secret from secrets
    let secret = crate::secrets::get_required("jwt_secret")
        .map_err(|_| "JWT secret not configured".to_string())?;

    // Validate token (simplified - use a proper JWT library)
    // In practice, use jsonwebtoken or similar
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err("Malformed token".to_string());
    }

    // Return extracted claims
    Ok(vec![
        KeyValue { key: "sub".to_string(), value: "user123".to_string() },
        KeyValue { key: "role".to_string(), value: "admin".to_string() },
    ])
}

export!(JwtAuthenticator);
```

### Rate Limiter Plugin

Implement rate limiting:

```rust
wit_bindgen::generate!({
    world: "zlayer-rate-limiter",
    path: "../wit",
});

use crate::exports::rate_limiter::{Guest, RateLimitInfo, RateLimitResult};
use crate::request_types::PluginRequest;

struct SlidingWindowLimiter;

const WINDOW_SIZE_NS: u64 = 60_000_000_000; // 1 minute
const DEFAULT_LIMIT: u64 = 100;

impl Guest for SlidingWindowLimiter {
    fn check(request: PluginRequest) -> RateLimitResult {
        // Extract rate limit key (e.g., from API key or IP)
        let key = extract_key(&request);
        let limit = get_limit_for_key(&key);

        // Get current count
        let count_key = format!("ratelimit:{}:count", key);
        let current = crate::keyvalue::increment(&count_key, 1).unwrap_or(1) as u64;

        // Set TTL on first request in window
        if current == 1 {
            let _ = crate::keyvalue::set_with_ttl(
                &count_key,
                &1i64.to_le_bytes(),
                WINDOW_SIZE_NS,
            );
        }

        let remaining = limit.saturating_sub(current);

        // Record metrics
        crate::metrics::counter_inc("rate_limit_checks_total", 1);

        if current > limit {
            crate::metrics::counter_inc("rate_limit_exceeded_total", 1);
            RateLimitResult::Denied(RateLimitInfo {
                remaining: 0,
                limit,
                reset_after: WINDOW_SIZE_NS,
                retry_after: Some(WINDOW_SIZE_NS / 2),
            })
        } else {
            RateLimitResult::Allowed(RateLimitInfo {
                remaining,
                limit,
                reset_after: WINDOW_SIZE_NS,
                retry_after: None,
            })
        }
    }

    fn status(key: String) -> Option<RateLimitInfo> {
        let count_key = format!("ratelimit:{}:count", key);
        let current = crate::keyvalue::get(&count_key)
            .ok()
            .flatten()
            .map(|v| i64::from_le_bytes(v.try_into().unwrap_or_default()) as u64)
            .unwrap_or(0);

        let limit = get_limit_for_key(&key);
        let remaining = limit.saturating_sub(current);

        Some(RateLimitInfo {
            remaining,
            limit,
            reset_after: WINDOW_SIZE_NS,
            retry_after: if remaining == 0 { Some(WINDOW_SIZE_NS / 2) } else { None },
        })
    }
}

fn extract_key(request: &PluginRequest) -> String {
    // Try API key first
    request.headers.iter()
        .find(|h| h.key.to_lowercase() == "x-api-key")
        .map(|h| h.value.clone())
        // Fall back to client IP from context
        .or_else(|| {
            request.context.iter()
                .find(|h| h.key == "client_ip")
                .map(|h| h.value.clone())
        })
        .unwrap_or_else(|| "anonymous".to_string())
}

fn get_limit_for_key(key: &str) -> u64 {
    // Check for custom limit in config
    crate::config::get_int(&format!("rate_limits.{}", key))
        .map(|v| v as u64)
        .unwrap_or(DEFAULT_LIMIT)
}

export!(SlidingWindowLimiter);
```

### Caching Plugin

Control response caching:

```rust
wit_bindgen::generate!({
    world: "zlayer-plugin",
    path: "../wit",
});

use crate::exports::caching::{CacheDecision, CacheEntry, Guest};
use crate::common::KeyValue;

struct SmartCache;

impl Guest for SmartCache {
    fn cache_policy(
        method: String,
        path: String,
        status: u16,
        headers: Vec<KeyValue>,
    ) -> CacheDecision {
        // Only cache successful GET requests
        if method != "GET" || status != 200 {
            return CacheDecision::NoCache;
        }

        // Check Cache-Control header
        let cache_control = headers.iter()
            .find(|h| h.key.to_lowercase() == "cache-control")
            .map(|h| h.value.as_str());

        if let Some(cc) = cache_control {
            if cc.contains("no-store") || cc.contains("private") {
                return CacheDecision::NoCache;
            }
        }

        // API responses - cache for 5 minutes with tags
        if path.starts_with("/api/") {
            let resource = extract_resource(&path);
            return CacheDecision::CacheWithTags(CacheEntry {
                ttl: 300_000_000_000, // 5 minutes
                tags: vec![format!("api:{}", resource)],
                vary: vec!["authorization".to_string(), "accept".to_string()],
                stale_while_revalidate: Some(60_000_000_000), // 1 minute
            });
        }

        // Static assets - cache for 1 hour
        if path.starts_with("/static/") {
            return CacheDecision::CacheFor(3600_000_000_000);
        }

        CacheDecision::NoCache
    }

    fn cache_key(method: String, path: String, headers: Vec<KeyValue>) -> String {
        // Include Vary headers in cache key
        let accept = headers.iter()
            .find(|h| h.key.to_lowercase() == "accept")
            .map(|h| h.value.as_str())
            .unwrap_or("*/*");

        format!("{}:{}:{}", method, path, accept)
    }
}

fn extract_resource(path: &str) -> String {
    path.split('/')
        .filter(|s| !s.is_empty())
        .take(2)
        .collect::<Vec<_>>()
        .join("/")
}

export!(SmartCache);
```

### WebSocket Plugin

Handle WebSocket connections:

```rust
wit_bindgen::generate!({
    world: "zlayer-plugin",
    path: "../wit",
});

use crate::exports::websocket::{Guest, Message, MessageType, UpgradeDecision};
use crate::common::KeyValue;

struct WebSocketHandler;

impl Guest for WebSocketHandler {
    fn on_upgrade(path: String, headers: Vec<KeyValue>) -> UpgradeDecision {
        // Validate upgrade request
        if !path.starts_with("/ws/") {
            return UpgradeDecision::Reject(404, "WebSocket endpoint not found".to_string());
        }

        // Check authentication
        let token = headers.iter()
            .find(|h| h.key == "sec-websocket-protocol")
            .and_then(|h| h.value.split(',').find(|p| p.starts_with("auth-")))
            .map(|p| p.trim_start_matches("auth-"));

        match token {
            Some(t) if validate_token(t) => {
                // Accept with selected subprotocol
                UpgradeDecision::AcceptWithHeaders(vec![
                    KeyValue {
                        key: "sec-websocket-protocol".to_string(),
                        value: format!("auth-{}", t),
                    },
                ])
            }
            _ => UpgradeDecision::Reject(401, "Invalid or missing authentication".to_string()),
        }
    }

    fn on_client_message(message: Message) -> Option<Message> {
        match message.msg_type {
            MessageType::Text => {
                // Log and forward text messages
                let text = String::from_utf8_lossy(&message.data);
                crate::logging::debug(&format!("Client message: {}", text));

                // Could transform message here
                Some(message)
            }
            MessageType::Binary => {
                // Forward binary messages as-is
                Some(message)
            }
            MessageType::Ping => {
                // Auto-respond with pong
                Some(Message {
                    msg_type: MessageType::Pong,
                    data: message.data,
                })
            }
            MessageType::Close => {
                crate::logging::info("Client initiated close");
                Some(message)
            }
            _ => Some(message),
        }
    }

    fn on_upstream_message(message: Message) -> Option<Message> {
        // Forward upstream messages to client
        Some(message)
    }
}

fn validate_token(token: &str) -> bool {
    // Validate token against secrets/keyvalue
    true // Simplified
}

export!(WebSocketHandler);
```

### Transformer Plugin

Simple request/response transformations:

```rust
wit_bindgen::generate!({
    world: "zlayer-transformer",
    path: "../wit",
});

use crate::exports::transformer::Guest;
use crate::common::KeyValue;

struct HeaderTransformer;

impl Guest for HeaderTransformer {
    fn transform_request_headers(headers: Vec<KeyValue>) -> Vec<KeyValue> {
        let mut result = headers;

        // Remove sensitive headers
        result.retain(|h| {
            let key = h.key.to_lowercase();
            key != "x-internal-secret" && key != "x-debug-token"
        });

        // Add standard headers
        result.push(KeyValue {
            key: "x-transformed-by".to_string(),
            value: "zlayer-transformer".to_string(),
        });

        result
    }

    fn transform_response_headers(headers: Vec<KeyValue>) -> Vec<KeyValue> {
        let mut result = headers;

        // Add CORS headers
        result.push(KeyValue {
            key: "access-control-allow-origin".to_string(),
            value: "*".to_string(),
        });

        result
    }

    fn transform_request_body(body: Vec<u8>) -> Vec<u8> {
        // Pass through unchanged
        body
    }

    fn transform_response_body(body: Vec<u8>) -> Vec<u8> {
        // Pass through unchanged
        body
    }
}

export!(HeaderTransformer);
```

## Filesystem Access

WASM plugins can access preopened directories configured in the service spec:

```yaml
# deployment.yaml
services:
  my-plugin:
    rtype: service
    image:
      name: ghcr.io/myorg/my-plugin:v1
    storage:
      - type: bind
        source: /data/models
        target: /models
        readonly: true
      - type: bind
        source: /data/uploads
        target: /uploads
        readonly: false
```

Then in your plugin:

```rust
use std::fs;
use std::io::{Read, Write};

// Read from preopened directory
let model_data = fs::read("/models/weights.bin")?;

// Write to writable directory
fs::write("/uploads/output.json", output_bytes)?;

// List directory contents
for entry in fs::read_dir("/models")? {
    let entry = entry?;
    println!("Found: {:?}", entry.path());
}
```

## Networking

WASM plugins have full `wasi:sockets` support for TCP/UDP networking:

```rust
// TCP client example
use wasi::sockets::tcp::*;
use wasi::sockets::network::*;

fn tcp_request(host: &str, port: u16, data: &[u8]) -> Result<Vec<u8>, String> {
    // Resolve address
    let network = instance_network();
    let addr = IpSocketAddress::Ipv4(Ipv4SocketAddress {
        port,
        address: (127, 0, 0, 1), // Simplified - use DNS resolution
    });

    // Create and connect socket
    let socket = tcp_create_socket(IpAddressFamily::Ipv4)
        .map_err(|e| format!("Failed to create socket: {:?}", e))?;

    socket.start_connect(&network, addr)
        .map_err(|e| format!("Failed to start connect: {:?}", e))?;

    // Wait for connection
    let pollable = socket.subscribe();
    pollable.block();

    let (rx, tx) = socket.finish_connect()
        .map_err(|e| format!("Failed to finish connect: {:?}", e))?;

    // Send data
    tx.blocking_write_and_flush(data)
        .map_err(|e| format!("Failed to write: {:?}", e))?;

    // Read response
    let mut response = Vec::new();
    loop {
        match rx.read(4096) {
            Ok(chunk) if chunk.is_empty() => break,
            Ok(chunk) => response.extend_from_slice(&chunk),
            Err(_) => break,
        }
    }

    Ok(response)
}
```

## HTTP Client Requests

Make outgoing HTTP requests using `wasi:http/outgoing-handler`:

```rust
use wasi::http::outgoing_handler;
use wasi::http::types::*;

fn make_http_request(url: &str, method: Method) -> Result<(u16, Vec<u8>), String> {
    // Create request
    let headers = Fields::new();
    headers.append(&"user-agent".to_string(), &b"zlayer-plugin/1.0".to_vec()).unwrap();

    let request = OutgoingRequest::new(headers);
    request.set_method(&method).unwrap();
    request.set_path_with_query(Some(url)).unwrap();

    // Send request
    let future_response = outgoing_handler::handle(request, None)
        .map_err(|e| format!("Request failed: {:?}", e))?;

    // Wait for response
    let pollable = future_response.subscribe();
    pollable.block();

    let response = future_response.get()
        .ok_or("No response")?
        .map_err(|e| format!("Response error: {:?}", e))?
        .map_err(|e| format!("HTTP error: {:?}", e))?;

    let status = response.status();

    // Read body
    let body = response.consume().map_err(|_| "Failed to consume body")?;
    let stream = body.stream().map_err(|_| "Failed to get stream")?;

    let mut data = Vec::new();
    loop {
        match stream.read(65536) {
            Ok(chunk) if chunk.is_empty() => break,
            Ok(chunk) => data.extend_from_slice(&chunk),
            Err(_) => break,
        }
    }

    Ok((status, data))
}
```

## Testing Plugins Locally

```bash
# Validate the WASM binary
zlayer wasm validate ./my_handler.wasm

# Show WASM binary info
zlayer wasm info ./my_handler.wasm

# Run locally for testing
zlayer run --wasm ./my_handler.wasm --port 8080

# Test with curl
curl http://localhost:8080/api/test
```

## Deployment Configuration

Configure WASM plugins in your deployment spec:

```yaml
version: v1
deployment: my-wasm-app

services:
  api-handler:
    rtype: service
    image:
      name: ghcr.io/myorg/api-handler:v1
    env:
      - name: LOG_LEVEL
        value: debug
      - name: API_KEY
        value: "${API_KEY}"
    endpoints:
      - name: http
        protocol: http
        port: 8080
        expose: public
    scale:
      mode: adaptive
      min: 2
      max: 10
    health:
      check:
        type: http
        url: /health
        expect_status: 200

  auth-plugin:
    rtype: service
    image:
      name: ghcr.io/myorg/auth-plugin:v1
    env:
      - name: jwt_secret
        value: "${JWT_SECRET}"

  rate-limiter:
    rtype: service
    image:
      name: ghcr.io/myorg/rate-limiter:v1
    env:
      - name: rate_limits.default
        value: "100"
      - name: rate_limits.premium
        value: "1000"
```

## Best Practices

1. **Keep plugins small** - WASM cold starts are fast (~1-5ms), but smaller binaries load faster
2. **Use structured logging** - Makes debugging in production easier; include request IDs
3. **Emit metrics** - Track plugin performance with counters and histograms
4. **Handle errors gracefully** - Return appropriate HTTP status codes; never panic
5. **Use TTLs for cached data** - Prevent unbounded storage growth
6. **Validate inputs** - Never trust user input; validate and sanitize
7. **Test with real requests** - Use `zlayer wasm validate` before deploying
8. **Use capabilities minimally** - Only request the capabilities you need
9. **Implement shutdown gracefully** - Clean up resources in the shutdown hook
10. **Version your plugins** - Use semantic versioning in plugin info

## Troubleshooting

### Common Issues

**"Capability unavailable" error during init:**
- Ensure your plugin world includes the required imports
- Check that the deployment configuration allows the capability

**"Config missing" error:**
- Verify environment variables are set in the deployment spec
- Check that config keys match exactly (case-sensitive)

**"Storage quota exceeded":**
- Use TTLs for temporary data
- Clean up old keys in list_keys
- Request a higher quota in deployment config

**WASM validation failures:**
- Ensure you're building for `wasm32-wasip2` target
- Check that wit-bindgen version matches ZLayer's WIT definitions
- Verify all exported interfaces are implemented

### Debug Logging

Enable debug logging to troubleshoot plugin issues:

```yaml
services:
  my-plugin:
    env:
      - name: RUST_LOG
        value: debug
```

Or in your plugin:

```rust
if crate::logging::is_enabled(Level::Debug) {
    crate::logging::debug(&format!("Request details: {:?}", request));
}
```
