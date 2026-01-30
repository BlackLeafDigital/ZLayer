# ZLayer Rust SDK

Plugin Development Kit for building WASM plugins in Rust targeting ZLayer.

## Requirements

- Rust 1.85+ (edition 2024)
- [cargo-component](https://github.com/bytecodealliance/cargo-component) for WASM compilation

## Installation

Add to your `Cargo.toml`:

```toml
[package]
name = "my-plugin"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
zlayer-sdk = "0.1"

[package.metadata.component]
package = "zlayer:plugin"
```

## Development Setup

```bash
# Install cargo-component
cargo install cargo-component

# Clone and build the SDK
cd clients/zlayer-sdk/rust
cargo build
```

## Usage

### Basic Plugin Structure

```rust
use zlayer_sdk::prelude::*;

struct MyPlugin;

impl Handler for MyPlugin {
    fn handle(&self, request: Request) -> Response {
        // Access key-value storage
        let value = kv::get("my-key");

        // Log messages to host
        log::info("Processing request");

        // Return response
        Response::ok(b"Hello from ZLayer!")
    }
}

// Export the handler to ZLayer runtime
export_handler!(MyPlugin);
```

### Building WASM Component

```bash
# Build with cargo-component
cargo component build --release

# Output will be in target/wasm32-wasip2/release/my_plugin.wasm
```

### Available Host Capabilities

The SDK provides access to ZLayer host functions:

- **config** - Plugin configuration access
- **kv** - Key-value storage operations
- **log** - Structured logging
- **secrets** - Secure secret retrieval
- **metrics** - Emit custom metrics
- **http** - Outbound HTTP requests (WASI HTTP)

### Plugin Worlds

ZLayer supports multiple plugin worlds for different use cases:

| World | Description | Capabilities |
|-------|-------------|--------------|
| `zlayer-plugin` | Full-featured plugins | All host functions + WASI CLI/HTTP |
| `zlayer-http-handler` | HTTP request handlers | HTTP, config, KV, logging |
| `zlayer-transformer` | Simple transformations | Logging only |
| `zlayer-authenticator` | Authentication plugins | Full access + secrets |
| `zlayer-rate-limiter` | Rate limiting plugins | Config, KV, logging, metrics |
| `zlayer-middleware` | Request/response middleware | HTTP, config, logging |
| `zlayer-router` | Custom routing logic | HTTP, config, KV, logging |

### Selecting a World

Specify the world in your `Cargo.toml`:

```toml
[package.metadata.component]
package = "zlayer:plugin"
world = "zlayer-http-handler"
```

## Project Structure

```
src/
  lib.rs           # Main library with bindings
examples/
  hello.rs         # Basic hello world plugin
wit/               # Symlink to WIT definitions
```

## Testing

```bash
# Run unit tests
cargo test

# Test with the ZLayer runtime (requires zlayer CLI)
zlayer plugin test ./target/wasm32-wasip2/release/my_plugin.wasm
```

## Type Safety

All bindings are generated from WIT definitions at compile time, providing:

- Compile-time type checking
- IDE autocompletion
- Documentation from WIT comments

## License

MIT OR Apache-2.0
