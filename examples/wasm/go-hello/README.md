# Go Hello WASM Example

A minimal Go WebAssembly example for ZLayer using TinyGo.

## Overview

This example demonstrates how to create a Go WASM module that can be deployed to ZLayer. It uses TinyGo, which produces smaller WebAssembly binaries than the standard Go compiler and has better WASI support.

Key features:
- Minimal Go WASM project structure
- TinyGo configuration for WASIp2
- Exported functions for host runtime
- WASI I/O support

## Prerequisites

1. **Go** (1.21 or later):
   ```bash
   # Ubuntu/Debian
   sudo apt install golang-go

   # macOS
   brew install go

   # Or download from https://go.dev/dl/
   ```

2. **TinyGo** (0.32 or later):
   ```bash
   # Ubuntu/Debian
   wget https://github.com/tinygo-org/tinygo/releases/download/v0.32.0/tinygo_0.32.0_amd64.deb
   sudo dpkg -i tinygo_0.32.0_amd64.deb

   # macOS
   brew install tinygo

   # Or see https://tinygo.org/getting-started/install/
   ```

3. **Verify installation**:
   ```bash
   tinygo version
   # Should show: tinygo version 0.32.0 (or later)
   ```

## Building

### Basic Build

```bash
tinygo build -o hello.wasm -target=wasip2 main.go
```

### Optimized Build

```bash
tinygo build -o hello.wasm -target=wasip2 -opt=2 -no-debug main.go
```

Build flags:
- `-target=wasip2`: Target WASI Preview 2
- `-opt=2`: Maximum optimization level
- `-no-debug`: Strip debug information
- `-scheduler=none`: Disable goroutine scheduler (for simpler modules)

### Size-Optimized Build

```bash
tinygo build -o hello.wasm -target=wasip2 -opt=z -no-debug -scheduler=none main.go
```

## Running Locally

Test with a WASM runtime like Wasmtime:

```bash
# Install wasmtime
curl https://wasmtime.dev/install.sh -sSf | bash

# Run the module
wasmtime hello.wasm
# Output: Hello from Go WASM, ZLayer!
```

## Deploying to ZLayer

### Using zlayer CLI

```bash
# Deploy the WASM module
zlayer deploy --wasm hello.wasm

# Or with a deployment manifest
zlayer deploy -f ../deployment.yaml
```

### ZLayer Detection

ZLayer automatically detects WASM modules by:
1. File extension (`.wasm`)
2. Binary magic number (`\0asm`)
3. Component model markers

No special configuration is needed.

## Project Structure

```
go-hello/
├── main.go      # Module implementation
└── README.md    # This file
```

## Code Walkthrough

### Exported Functions

TinyGo uses `//export` comments to expose functions to the WASM host:

```go
//export greet
func greet(name *byte, nameLen int32) {
    // Function body
}
```

These functions can be called directly by the ZLayer runtime.

### WASI Integration

TinyGo automatically handles WASI setup. Standard Go I/O functions work:

```go
fmt.Println("Hello!")        // Writes to WASI stdout
fmt.Fprintln(os.Stderr, ...) // Writes to WASI stderr
```

## Testing

Run native Go tests:

```bash
go test -v
```

Note: Tests run with the standard Go compiler, not TinyGo.

## Extending This Example

### Add HTTP Handling

For web services, add WASI HTTP support:

```go
//go:build tinygo

package main

// Use WASI HTTP interfaces for request/response handling
// See: https://github.com/WebAssembly/wasi-http
```

### Add Configuration

Read environment or config:

```go
import "os"

func init() {
    if greeting := os.Getenv("GREETING"); greeting != "" {
        // Use custom greeting
    }
}
```

## Size Comparison

| Build Type      | Approximate Size |
|-----------------|------------------|
| Basic           | ~200 KB          |
| Optimized       | ~80 KB           |
| Size-optimized  | ~50 KB           |

## TinyGo vs Standard Go

| Feature          | TinyGo        | Standard Go    |
|------------------|---------------|----------------|
| Binary size      | Small (50-200KB) | Large (2MB+)   |
| WASI support     | Full WASIp2   | Experimental   |
| Goroutines       | Limited       | Full           |
| Reflection       | Partial       | Full           |
| Build speed      | Fast          | Fast           |

For WASM targets, TinyGo is strongly recommended.

## Troubleshooting

### "unknown target: wasip2"

Update TinyGo to version 0.32.0 or later:

```bash
tinygo version
# If older, reinstall from tinygo.org
```

### Large Binary Size

Ensure optimization flags are set:

```bash
tinygo build -o hello.wasm -target=wasip2 -opt=z -no-debug main.go
```

### Missing WASI Functions

Some Go standard library functions are not available in WASI. Check TinyGo compatibility:
https://tinygo.org/docs/reference/lang-support/

## License

MIT OR Apache-2.0
