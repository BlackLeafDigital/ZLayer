# ZLayer WASM Plugin Examples

This directory contains example WASM plugins for ZLayer in multiple languages. Each example implements the `wasi:http/incoming-handler` interface to create serverless HTTP handlers.

## Overview

ZLayer supports WebAssembly plugins that can:
- Handle HTTP requests
- Make outgoing HTTP calls
- Access key-value storage
- Read configuration
- Emit structured logs and metrics

All examples implement the same HTTP endpoints:
- `GET /` - Returns a welcome message with request info
- `GET /health` - Health check endpoint
- `POST /echo` - Echoes back the request body as JSON

## Language Examples

| Language | Directory | Toolchain | Status |
|----------|-----------|-----------|--------|
| Rust | [rust/](rust/) | cargo-component | Production Ready |
| Go | [go/](go/) | TinyGo + wit-bindgen-go | Experimental |
| Python | [python/](python/) | componentize-py | Experimental |

## Rust Examples

Rust provides the most mature WASM component support via `cargo-component`.

### Prerequisites

```bash
# Install Rust WASM target
rustup target add wasm32-wasip2

# Install cargo-component
cargo install cargo-component
```

### Building

```bash
cd rust/hello-handler
cargo component build --release

# Output: target/wasm32-wasip2/release/hello_handler.wasm
```

### Available Rust Examples

- **hello-handler** - Simple HTTP request handler
- **kv-counter** - Counter using key-value storage
- **tcp-client** - TCP client demonstrating socket access

## Go Examples

Go support uses TinyGo with `wit-bindgen-go` for generating bindings.

### Prerequisites

```bash
# Install TinyGo (0.32+ recommended)
# See: https://tinygo.org/getting-started/install/

# Install wit-bindgen-go
go install go.bytecodealliance.org/cmd/wit-bindgen-go@latest
```

### Building

```bash
cd go/hello-handler

# Generate WIT bindings
wit-bindgen-go generate --out gen ../../../../wit

# Build the WASM component
tinygo build -target=wasip2 -o hello-handler.wasm .
```

### Notes

TinyGo's WASI Preview 2 support is still evolving. Check the [TinyGo documentation](https://tinygo.org/docs/guides/webassembly/) for the latest compatibility information.

## Python Examples

Python support uses `componentize-py` to compile Python to WASM components.

### Prerequisites

```bash
# Install componentize-py
pip install componentize-py
```

### Building

```bash
cd python/hello-handler

# Build the WASM component
componentize-py -d ../../../../wit -w wasi:http/proxy@0.2.0 componentize app -o hello-handler.wasm
```

### Notes

componentize-py bundles a Python interpreter into the WASM module, resulting in larger binary sizes (~10-20MB) compared to Rust or Go. This is acceptable for many use cases but may impact cold start times.

## Deployment

### Push to Registry

```bash
# Push the compiled WASM to a registry
zlayer wasm push ./hello-handler.wasm ghcr.io/myorg/hello-handler:v1
```

### Deployment Configuration

```yaml
# deployment.yaml
version: v1
deployment: my-wasm-app

services:
  hello-handler:
    rtype: service
    image:
      name: ghcr.io/myorg/hello-handler:v1
    endpoints:
      - name: http
        protocol: http
        port: 8080
        expose: public
    scale:
      mode: adaptive
      min: 1
      max: 10
    health:
      check:
        type: http
        url: /health
        expect_status: 200
```

### Deploy

```bash
zlayer deploy deployment.yaml
```

## Local Testing

### Validate WASM Binary

```bash
zlayer wasm validate ./hello-handler.wasm
```

### Run Locally

```bash
zlayer run --wasm ./hello-handler.wasm --port 8080
```

### Test Endpoints

```bash
# Welcome endpoint
curl http://localhost:8080/

# Health check
curl http://localhost:8080/health

# Echo endpoint
curl -X POST -d '{"test": "data"}' http://localhost:8080/echo
```

## WIT Interface

All examples implement the `wasi:http/incoming-handler` interface defined in the ZLayer WIT files:

```wit
// wasi:http/incoming-handler@0.2.0
interface incoming-handler {
    use types.{incoming-request, response-outparam};

    handle: func(request: incoming-request, response-out: response-outparam);
}
```

For full documentation on available interfaces and host capabilities, see [docs/WASM_PLUGINS.md](../../docs/WASM_PLUGINS.md).

## Directory Structure

```
examples/wasm-plugins/
├── README.md           # This file
├── rust/
│   ├── hello-handler/  # Simple HTTP handler
│   ├── kv-counter/     # Key-value storage example
│   └── tcp-client/     # TCP socket example
├── go/
│   └── hello-handler/  # TinyGo HTTP handler
└── python/
    └── hello-handler/  # componentize-py HTTP handler
```

## Troubleshooting

### Rust Build Errors

```
error: could not compile `hello-handler`
```

Ensure you have the correct target installed:
```bash
rustup target add wasm32-wasip2
```

### Go Binding Generation Errors

```
wit-bindgen-go: command not found
```

Install the binding generator:
```bash
go install go.bytecodealliance.org/cmd/wit-bindgen-go@latest
```

### Python Import Errors

```
ModuleNotFoundError: No module named 'proxy'
```

The `proxy` module is generated by componentize-py during build. Ensure you're running componentize-py from the correct directory with the WIT path pointing to the ZLayer wit/ directory.

### WASM Validation Failures

```
zlayer wasm validate: component does not export wasi:http/incoming-handler
```

Verify your plugin correctly implements and exports the incoming-handler interface. Check that:
1. The WIT world is correctly specified
2. The handler struct/class is properly exported
3. The build completed without errors

## Additional Resources

- [WASM Plugins Documentation](../../docs/WASM_PLUGINS.md) - Full plugin development guide
- [WIT Definitions](../../wit/) - ZLayer WIT interface definitions
- [Bytecode Alliance](https://bytecodealliance.org/) - WASI and Component Model specifications
- [TinyGo WASM Guide](https://tinygo.org/docs/guides/webassembly/)
- [componentize-py](https://github.com/bytecodealliance/componentize-py)
