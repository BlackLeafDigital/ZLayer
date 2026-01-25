# Registry Authentication Module

This module provides flexible authentication for OCI registries, supporting multiple authentication sources.

## Features

- **Docker Config Support**: Automatically reads credentials from `~/.docker/config.json`
- **Multiple Auth Sources**: Anonymous, Basic, Environment Variables, Docker Config
- **Per-Registry Configuration**: Different auth methods for different registries
- **Automatic Registry Detection**: Extracts registry from image references

## Usage

### Basic Usage with Docker Config (Default)

```rust
use zlayer_core::auth::{AuthConfig, AuthResolver};

// Create resolver with default config (uses Docker config)
let config = AuthConfig::default();
let resolver = AuthResolver::new(config);

// Resolve authentication for an image
let auth = resolver.resolve("ghcr.io/owner/repo:tag");
```

### Custom Configuration

```rust
use zlayer_core::auth::{AuthConfig, AuthSource, RegistryAuthConfig};

let config = AuthConfig {
    // Specific auth for GitHub Container Registry
    registries: vec![
        RegistryAuthConfig {
            registry: "ghcr.io".to_string(),
            source: AuthSource::Basic {
                username: "myuser".to_string(),
                password: "mytoken".to_string(),
            },
        },
    ],
    // Default to Docker config for other registries
    default: AuthSource::DockerConfig,
    docker_config_path: None,
};

let resolver = AuthResolver::new(config);
```

### Environment Variable Authentication

```rust
use zlayer_core::auth::{AuthConfig, AuthSource};

let config = AuthConfig {
    registries: vec![],
    default: AuthSource::EnvVar {
        username_var: "REGISTRY_USERNAME".to_string(),
        password_var: "REGISTRY_PASSWORD".to_string(),
    },
    docker_config_path: None,
};

let resolver = AuthResolver::new(config);
```

### Integration with AgentConfig

```rust
use zlayer_core::{AgentConfig, auth::{AuthConfig, AuthSource}};

let agent_config = AgentConfig {
    deployment_name: "my-deployment".to_string(),
    node_id: "node-1".to_string(),
    auth: AuthConfig {
        default: AuthSource::DockerConfig,
        ..Default::default()
    },
    ..Default::default()
};
```

## Docker Config Format

The module supports the standard Docker config.json format:

```json
{
  "auths": {
    "ghcr.io": {
      "auth": "base64(username:password)"
    },
    "docker.io": {
      "username": "myuser",
      "password": "mypass"
    }
  }
}
```

Both the `auth` field (base64-encoded) and plain `username`/`password` fields are supported.

## Registry Detection

The module automatically extracts the registry from image references:

- `ubuntu:latest` → `docker.io`
- `ghcr.io/owner/repo:tag` → `ghcr.io`
- `localhost:5000/image` → `localhost:5000`
- `myregistry.com/path/to/image:v1.0` → `myregistry.com`

## Testing

Run the test suite with:

```bash
cargo test -p zlayer-core
```

All authentication sources and registry detection logic are thoroughly tested.
