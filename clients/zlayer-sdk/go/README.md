# ZLayer Go SDK

Go SDK for building ZLayer WASM plugins using TinyGo and the WebAssembly Component Model.

## Prerequisites

- [Go 1.23+](https://golang.org/dl/)
- [TinyGo 0.34+](https://tinygo.org/getting-started/install/)
- [wit-bindgen-go](https://github.com/bytecodealliance/wit-bindgen)

## Installation

### Install wit-bindgen-go

```bash
go install go.bytecodealliance.org/cmd/wit-bindgen-go@latest
```

### Install TinyGo

Follow the official TinyGo installation guide for your platform:
https://tinygo.org/getting-started/install/

On Linux (Debian/Ubuntu):
```bash
wget https://github.com/tinygo-org/tinygo/releases/download/v0.34.0/tinygo_0.34.0_amd64.deb
sudo dpkg -i tinygo_0.34.0_amd64.deb
```

On macOS:
```bash
brew tap tinygo-org/tools
brew install tinygo
```

## Generating WIT Bindings

From this directory, generate the Go bindings from the WIT definitions:

```bash
wit-bindgen-go generate --world zlayer-plugin --out gen ../../../wit/zlayer
```

This will create Go packages in the `gen/` directory that provide type-safe access to ZLayer host functions.

## Building a Plugin

### Example Plugin

Create a new Go file for your plugin (e.g., `examples/hello/main.go`):

```go
package main

import (
    "github.com/zlayer/zlayer-sdk-go/gen/zlayer/plugin/types"
)

//go:generate wit-bindgen-go generate --world zlayer-plugin --out ../../gen ../../../wit/zlayer

func init() {
    // Register your plugin implementation
}

// Implement the required plugin interface
type HelloPlugin struct{}

func (p *HelloPlugin) Init(config types.PluginConfig) types.Result[struct{}, types.PluginError] {
    // Initialize your plugin
    return types.Ok[struct{}, types.PluginError](struct{}{})
}

func (p *HelloPlugin) HandleRequest(req types.HttpRequest) types.Result[types.HttpResponse, types.PluginError] {
    return types.Ok[types.HttpResponse, types.PluginError](types.HttpResponse{
        StatusCode: 200,
        Headers:    []types.HttpHeader{{Name: "Content-Type", Value: "text/plain"}},
        Body:       types.Some([]byte("Hello from Go!")),
    })
}

func main() {}
```

### Build with TinyGo

Build your plugin as a WASM component:

```bash
tinygo build -o plugin.wasm -target=wasip2 -scheduler=none -gc=leaking ./examples/hello
```

Build flags explained:
- `-target=wasip2`: Target WASI Preview 2 (Component Model)
- `-scheduler=none`: Disable the Go scheduler (not needed for request handlers)
- `-gc=leaking`: Use a simple allocator (smaller binary, suitable for short-lived requests)

For production builds with optimizations:

```bash
tinygo build -o plugin.wasm -target=wasip2 -scheduler=none -gc=leaking -opt=2 -no-debug ./examples/hello
```

### Alternative: Using wasm-tools for Component Model

If your TinyGo version doesn't support `-target=wasip2` directly, you can build a core module and convert it:

```bash
# Build core WASM module
tinygo build -o plugin.core.wasm -target=wasi -scheduler=none -gc=leaking ./examples/hello

# Convert to component using wasm-tools
wasm-tools component new plugin.core.wasm -o plugin.wasm --adapt wasi_snapshot_preview1.reactor.wasm
```

## Project Structure

```
go/
├── go.mod              # Go module definition
├── zlayer.go           # SDK helper functions
├── gen/                # Generated WIT bindings (git-ignored content)
│   └── .gitkeep
├── examples/           # Example plugins
│   └── .gitkeep
└── README.md           # This file
```

## Available Host Functions

Once bindings are generated, you'll have access to these ZLayer host capabilities:

### Configuration
```go
import "github.com/zlayer/zlayer-sdk-go/gen/zlayer/plugin/config"

value := config.Get("my-key")
```

### Key-Value Storage
```go
import "github.com/zlayer/zlayer-sdk-go/gen/zlayer/plugin/kv"

kv.Set("key", []byte("value"))
data := kv.Get("key")
```

### Logging
```go
import "github.com/zlayer/zlayer-sdk-go/gen/zlayer/plugin/logging"

logging.Log(logging.LevelInfo, "Processing request")
```

### Secrets
```go
import "github.com/zlayer/zlayer-sdk-go/gen/zlayer/plugin/secrets"

apiKey := secrets.Get("api-key")
```

## Best Practices

1. **Keep plugins small**: WASM modules should be focused and minimal
2. **Use `-gc=leaking`**: For request handlers, this reduces binary size significantly
3. **Avoid goroutines**: Use `-scheduler=none` since plugins handle one request at a time
4. **Handle errors explicitly**: Always check and handle error results from host functions
5. **Minimize allocations**: Pre-allocate buffers where possible for better performance

## Debugging

To include debug information in your WASM module:

```bash
tinygo build -o plugin.wasm -target=wasip2 -scheduler=none ./examples/hello
```

Use `wasm-tools` to inspect the module:

```bash
wasm-tools print plugin.wasm
wasm-tools component wit plugin.wasm
```

## License

See the main ZLayer repository for license information.
