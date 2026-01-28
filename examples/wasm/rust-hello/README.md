# Rust Hello WASM Example

A minimal Rust WebAssembly example for ZLayer demonstrating the basics of WASM module development.

## Overview

This example shows how to create a simple Rust WASM module that can be deployed to ZLayer. It demonstrates:

- Basic Rust WASM project structure
- Cargo configuration for WASIp2 targets
- Simple module entry point
- No external dependencies (pure Rust)

## Prerequisites

1. **Rust toolchain** (1.82 or later recommended):
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

2. **WASM target**:
   ```bash
   rustup target add wasm32-wasip2
   ```

3. **cargo-component** for building WebAssembly components:
   ```bash
   cargo install cargo-component
   ```

## Building

### Development Build

```bash
cargo component build
```

Output: `target/wasm32-wasip2/debug/rust_hello_wasm.wasm`

### Release Build (Optimized)

```bash
cargo component build --release
```

Output: `target/wasm32-wasip2/release/rust_hello_wasm.wasm`

The release build includes:
- Size optimization (`opt-level = "s"`)
- Link-time optimization (LTO)
- Debug symbol stripping
- Single codegen unit for better optimization

## Deploying to ZLayer

### Using zlayer CLI

```bash
# Deploy the WASM module
zlayer deploy --wasm target/wasm32-wasip2/release/rust_hello_wasm.wasm

# Or with a deployment manifest
zlayer deploy -f deployment.yaml
```

### Using deployment.yaml

See the parent directory's `deployment.yaml` for a complete example.

## Project Structure

```
rust-hello/
├── Cargo.toml       # Package manifest with WASM configuration
├── README.md        # This file
└── src/
    └── lib.rs       # Module implementation
```

## Code Walkthrough

### Cargo.toml

Key configuration for WASM:

```toml
[lib]
crate-type = ["cdylib"]  # Compile as dynamic library (WASM)

[package.metadata.component]
package = "example:rust-hello"  # Component package name
```

### src/lib.rs

The module exports:
- `Greeter` struct with `greet()` and `farewell()` methods
- `greet_many()` function for batch processing
- `_start()` entry point for command execution

## Testing

Run native tests:

```bash
cargo test
```

## Extending This Example

To build a more complete ZLayer application:

1. **Add the ZLayer SDK**:
   ```toml
   [dependencies]
   zlayer-sdk = "0.1"
   ```

2. **Implement the plugin interface** (see `clients/zlayer-sdk/rust/examples/hello/`)

3. **Add WASI capabilities** for I/O, networking, etc.

## Size Comparison

| Build Type | Approximate Size |
|------------|------------------|
| Debug      | ~50 KB           |
| Release    | ~15 KB           |

## License

MIT OR Apache-2.0
