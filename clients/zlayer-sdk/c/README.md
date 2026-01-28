# ZLayer C SDK

A C library for building ZLayer plugins that compile to WebAssembly (WASIp2).

## Requirements

- CMake 3.20+
- WASI SDK (default path: `/opt/wasi-sdk`)
- C11 compatible compiler

## Installation

### Building the SDK

```bash
mkdir build && cd build
cmake .. -DWASI_SDK_PREFIX=/path/to/wasi-sdk
make
make install
```

### Building with Examples

```bash
cmake .. -DBUILD_EXAMPLES=ON
make
```

## Usage

### Basic Plugin Structure

```c
#include <zlayer.h>

// Your plugin entry point
int main(void) {
    zlayer_log_info("Plugin starting");

    // Get configuration
    char* value = zlayer_config_get("my_key");
    if (value) {
        zlayer_log_debug(value);
        zlayer_config_free(value);
    }

    return 0;
}
```

### Compiling Your Plugin

```bash
clang --target=wasm32-wasip2 \
    -I/path/to/zlayer-sdk/include \
    -L/path/to/zlayer-sdk/lib \
    -o plugin.wasm plugin.c -lzlayer_sdk
```

## API Reference

### Configuration

| Function | Description |
|----------|-------------|
| `zlayer_config_get(key)` | Get a configuration value by key. Returns `NULL` if not found. Caller must free with `zlayer_config_free()`. |
| `zlayer_config_get_all()` | Get all configuration as JSON string. Caller must free with `zlayer_config_free()`. |
| `zlayer_config_free(str)` | Free a string returned by config functions. |

### Key-Value Storage

| Function | Description |
|----------|-------------|
| `zlayer_kv_get(bucket, key, out_len)` | Get a value from KV storage. Returns `NULL` if not found. Sets `out_len` to data length. Caller must free with `zlayer_kv_free()`. |
| `zlayer_kv_set(bucket, key, value, len)` | Store a value. Returns `true` on success. |
| `zlayer_kv_delete(bucket, key)` | Delete a key. Returns `true` on success. |
| `zlayer_kv_keys(bucket, prefix, out_count)` | List keys with optional prefix. Sets `out_count` to number of keys. Caller must free with `zlayer_kv_free_keys()`. |
| `zlayer_kv_free(data)` | Free data returned by `zlayer_kv_get()`. |
| `zlayer_kv_free_keys(keys, count)` | Free key array returned by `zlayer_kv_keys()`. |

### Logging

| Function | Description |
|----------|-------------|
| `zlayer_log(level, message)` | Log a message at the specified level. |
| `zlayer_log_trace(message)` | Log at TRACE level. |
| `zlayer_log_debug(message)` | Log at DEBUG level. |
| `zlayer_log_info(message)` | Log at INFO level. |
| `zlayer_log_warn(message)` | Log at WARN level. |
| `zlayer_log_error(message)` | Log at ERROR level. |

**Log Levels:**
- `ZLAYER_LOG_TRACE` (0)
- `ZLAYER_LOG_DEBUG` (1)
- `ZLAYER_LOG_INFO` (2)
- `ZLAYER_LOG_WARN` (3)
- `ZLAYER_LOG_ERROR` (4)

### Secrets

| Function | Description |
|----------|-------------|
| `zlayer_secret_get(name)` | Get a secret value by name. Returns `NULL` if not found. Caller must free with `zlayer_secret_free()`. |
| `zlayer_secret_free(str)` | Free a string returned by secret functions. |

### Metrics

| Function | Description |
|----------|-------------|
| `zlayer_counter_inc(name, value)` | Increment a counter metric by the given value. |
| `zlayer_gauge_set(name, value)` | Set a gauge metric to the given value. |
| `zlayer_histogram_observe(name, value)` | Record an observation in a histogram metric. |

## Memory Management

The SDK follows a consistent memory management pattern:

1. **Functions returning `char*`**: Caller owns the memory and must free it using the appropriate `_free()` function.
2. **Functions returning `uint8_t*`**: Caller owns the memory and must free it using `zlayer_kv_free()`.
3. **Functions returning `char**`**: Caller owns the array and all strings; free with `zlayer_kv_free_keys()`.
4. **Input parameters**: The SDK does not take ownership of input pointers.

### Example: Proper Memory Management

```c
// Config - always free the result
char* config = zlayer_config_get("database_url");
if (config) {
    // Use config...
    zlayer_config_free(config);
}

// KV storage - always free the result
size_t len;
uint8_t* data = zlayer_kv_get("cache", "user:123", &len);
if (data) {
    // Use data...
    zlayer_kv_free(data);
}

// Key listing - free both array and contents
size_t count;
char** keys = zlayer_kv_keys("cache", "user:", &count);
if (keys) {
    for (size_t i = 0; i < count; i++) {
        // Use keys[i]...
    }
    zlayer_kv_free_keys(keys, count);
}

// Secrets - always free the result
char* secret = zlayer_secret_get("api_key");
if (secret) {
    // Use secret...
    zlayer_secret_free(secret);
}
```

## Version

Current SDK version: `0.1.0`

Access via the `ZLAYER_SDK_VERSION` macro:

```c
#include <zlayer.h>
printf("SDK Version: %s\n", ZLAYER_SDK_VERSION);
```

## License

See the main ZLayer repository for license information.
