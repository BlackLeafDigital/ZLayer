# ZLayer Python SDK

Plugin Development Kit for building WASM plugins in Python targeting ZLayer.

## Requirements

- Python 3.11+
- [componentize-py](https://github.com/bytecodealliance/componentize-py) for WASM compilation

## Installation

```bash
# Using uv (recommended)
uv add zlayer-sdk

# Or with pip
pip install zlayer-sdk
```

## Development Setup

```bash
# Clone and install in development mode
cd clients/zlayer-sdk/python
uv sync --all-extras

# Install componentize-py for WASM compilation
uv add --dev componentize-py
```

## Usage

### Basic Plugin Structure

```python
from zlayer import kv, log

def handle_request(request: bytes) -> bytes:
    """Plugin entry point called by ZLayer runtime."""
    # Access key-value storage
    value = kv.get("my-key")

    # Log messages to host
    log.info("Processing request")

    # Return response
    return b"OK"
```

### Building WASM Component

```bash
# Generate WIT bindings and compile to WASM
componentize-py -d ../../wit -w zlayer-plugin componentize my_plugin -o my_plugin.wasm
```

### Available Host Capabilities

The SDK provides access to ZLayer host functions:

- **kv** - Key-value storage operations
- **log** - Structured logging
- **http** - Outbound HTTP requests
- **config** - Plugin configuration access

## Project Structure

```
zlayer/
  __init__.py      # Package root with version
  py.typed         # PEP 561 type marker
examples/
  .gitkeep         # Example plugins (coming soon)
```

## Type Checking

This package is fully typed and includes a `py.typed` marker for PEP 561 compliance.

```bash
# Run type checks
uv run mypy zlayer/
```

## License

MIT
