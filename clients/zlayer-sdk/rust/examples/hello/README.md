# Hello World Plugin

A simple example demonstrating the ZLayer Rust SDK.

## Overview

This plugin showcases the core concepts of ZLayer plugin development:

- **Initialization**: Reading configuration and requesting capabilities
- **Metadata**: Providing plugin information for discovery and management
- **Request handling**: Processing HTTP requests with routing
- **Logging**: Structured logging with different severity levels
- **Graceful shutdown**: Cleanup when the plugin is unloaded

## Prerequisites

1. **Rust toolchain** with the `wasm32-wasip2` target:
   ```bash
   rustup target add wasm32-wasip2
   ```

2. **cargo-component** for building WebAssembly components:
   ```bash
   cargo install cargo-component
   ```

## Building

Build the plugin as a WebAssembly component:

```bash
cd clients/zlayer-sdk/rust/examples/hello
cargo component build --release
```

The compiled component will be at:
```
target/wasm32-wasip2/release/hello_plugin.wasm
```

## Configuration

The plugin accepts these configuration options:

| Key       | Type   | Default   | Description                    |
|-----------|--------|-----------|--------------------------------|
| `greeting`| string | `"Hello"` | Custom greeting prefix         |
| `debug`   | bool   | `false`   | Enable debug logging           |

Example configuration in your ZLayer deployment:

```yaml
plugins:
  - name: hello
    path: ./hello_plugin.wasm
    config:
      greeting: "Welcome"
      debug: true
```

## Endpoints

### POST /greet

Returns a personalized greeting.

**Request:**
```bash
curl -X POST http://localhost:8080/greet -d "World"
```

**Response:**
```
Hello, World!
```

With custom greeting configured:
```
Welcome, World!
```

### POST /echo

Echoes back the request body.

**Request:**
```bash
curl -X POST http://localhost:8080/echo -d "test data"
```

**Response:**
```
test data
```

### GET /info

Returns plugin information as JSON.

**Request:**
```bash
curl http://localhost:8080/info
```

**Response:**
```json
{
  "name": "Hello World Plugin",
  "version": "0.1.0",
  "description": "A hello world plugin demonstrating ZLayer SDK capabilities"
}
```

## Code Structure

```
hello/
├── Cargo.toml        # Package manifest with WASMp2 configuration
├── README.md         # This file
└── src/
    └── lib.rs        # Plugin implementation
```

## Key Concepts

### The Handler Trait

Plugins implement the `Guest` trait from the generated bindings:

```rust
impl Guest for HelloPlugin {
    fn init() -> Result<Capabilities, InitError> { ... }
    fn info() -> PluginInfo { ... }
    fn handle(request: PluginRequest) -> HandleResult { ... }
    fn shutdown() { ... }
}
```

### Capabilities

Plugins declare which host capabilities they need during initialization:

```rust
Ok(Capabilities::CONFIG | Capabilities::LOGGING)
```

Available capabilities:
- `CONFIG` - Read configuration values
- `KEYVALUE` - Access key-value storage
- `LOGGING` - Emit log messages
- `SECRETS` - Access secrets management
- `METRICS` - Emit metrics
- `HTTP_CLIENT` - Make outbound HTTP requests

### Handle Results

The `handle` function returns one of:

- `HandleResult::Response(response)` - Return a response to the client
- `HandleResult::PassThrough` - Let the next handler process the request
- `HandleResult::Error(message)` - Report an error

### Logging

Use the logging module for structured output:

```rust
use zlayer_sdk::bindings::zlayer::plugin::logging::{self, Level};

logging::info("Simple message");
logging::log_structured(Level::Debug, "Message with fields", &[
    KeyValue { key: "key".into(), value: "value".into() },
]);
```

## Testing

Run the SDK tests to verify the example:

```bash
cd clients/zlayer-sdk/rust
cargo test
```

## License

MIT OR Apache-2.0
