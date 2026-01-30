# zlayer-registry

OCI image pulling, caching, and WASM artifact detection for ZLayer.

## Features

- **Image Pulling**: Pull OCI images from container registries
- **Blob Caching**: Local caching of image layers for faster subsequent pulls
- **WASM Detection**: Automatic detection of WASM artifacts vs container images
- **Authentication**: Support for registry authentication

### Feature Flags

- `persistent`: Enable persistent disk-based blob cache using redb
- `s3`: Enable S3-compatible storage backend for blob caching
- `local`: Enable local OCI registry for storing built images

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
zlayer-registry = { path = "../zlayer-registry" }

# With features
zlayer-registry = { path = "../zlayer-registry", features = ["persistent", "local"] }
```

## Usage

### Basic Image Pulling

```rust
use zlayer_registry::{ImagePuller, BlobCache, RegistryAuth};
use std::path::Path;

async fn pull_image() -> Result<(), Box<dyn std::error::Error>> {
    // Create blob cache
    let cache = BlobCache::open(Path::new("/var/lib/zlayer/cache.redb"))?;

    // Create puller
    let puller = ImagePuller::new(cache);

    // Pull an image
    let auth = RegistryAuth::Anonymous;
    let (manifest, layers) = puller.pull("ghcr.io/org/image:latest", &auth).await?;

    println!("Pulled image with {} layers", layers.len());
    Ok(())
}
```

### WASM Artifact Detection

The registry module can automatically detect whether an OCI artifact is a WebAssembly module or a traditional container image.

#### Types

```rust
use zlayer_registry::wasm::{ArtifactType, WasiVersion, WasmArtifactInfo};

// ArtifactType represents what kind of OCI artifact was detected
enum ArtifactType {
    Container,                           // Traditional container image
    Wasm { wasi_version: WasiVersion },  // WebAssembly artifact
}

// WasiVersion indicates the WASI compatibility level
enum WasiVersion {
    Preview1,  // WASI Preview 1 (wasip1) - core module
    Preview2,  // WASI Preview 2 (wasip2) - component model
    Unknown,   // Unable to determine WASI version
}

// WasmArtifactInfo contains detailed WASM artifact metadata
struct WasmArtifactInfo {
    wasi_version: WasiVersion,
    wasm_layer_digest: Option<String>,
    wasm_size: Option<i64>,
    module_name: Option<String>,
}
```

#### Detection

```rust
use zlayer_registry::wasm::{detect_artifact_type, extract_wasm_info, ArtifactType};
use oci_client::manifest::OciImageManifest;

fn check_artifact(manifest: &OciImageManifest) {
    let artifact_type = detect_artifact_type(manifest);

    match artifact_type {
        ArtifactType::Container => {
            println!("This is a container image");
        }
        ArtifactType::Wasm { wasi_version } => {
            println!("This is a WASM artifact with WASI {}", wasi_version);

            // Extract detailed info
            if let Some(info) = extract_wasm_info(manifest) {
                println!("WASM layer digest: {:?}", info.wasm_layer_digest);
                println!("WASM size: {:?} bytes", info.wasm_size);
                println!("Module name: {:?}", info.module_name);
            }
        }
    }
}
```

#### Detection Strategy

WASM artifacts are detected through multiple signals in order of specificity:

1. **`artifact_type` field** (OCI 1.1+): Most explicit indicator
   - `application/vnd.wasm.component.v1+wasm` -> wasip2
   - `application/vnd.wasm.module.v1+wasm` -> wasip1

2. **`config.media_type` field**: WASM-specific config types
   - `application/vnd.wasm.config.v1+json`
   - `application/vnd.wasm.config.v0+json`

3. **Layer media types**: WASM binary layers
   - `application/vnd.wasm.content.layer.v1+wasm`
   - `application/wasm`

### Pulling WASM Artifacts

```rust
use zlayer_registry::{ImagePuller, BlobCache, RegistryAuth};

async fn pull_wasm(image: &str) -> Result<(), Box<dyn std::error::Error>> {
    let cache = BlobCache::open("/var/lib/zlayer/cache.redb")?;
    let puller = ImagePuller::new(cache);
    let auth = RegistryAuth::Anonymous;

    // Pull WASM artifact
    let (wasm_bytes, wasm_info) = puller.pull_wasm(image, &auth).await?;

    println!("Pulled WASM module: {} bytes", wasm_bytes.len());
    println!("WASI version: {}", wasm_info.wasi_version);

    Ok(())
}
```

### Getting WASM Info Without Pulling

```rust
use zlayer_registry::{ImagePuller, BlobCache, RegistryAuth};

async fn check_wasm(image: &str) -> Result<(), Box<dyn std::error::Error>> {
    let cache = BlobCache::open("/var/lib/zlayer/cache.redb")?;
    let puller = ImagePuller::new(cache);
    let auth = RegistryAuth::Anonymous;

    // Get WASM info without pulling the full artifact
    if let Some(info) = puller.get_wasm_info(image, &auth).await? {
        println!("Image is a WASM artifact:");
        println!("  WASI version: {}", info.wasi_version);
        println!("  Layer digest: {:?}", info.wasm_layer_digest);
        println!("  Size: {:?} bytes", info.wasm_size);
    } else {
        println!("Image is a container (not WASM)");
    }

    Ok(())
}
```

## Module Structure

- `client.rs`: OCI distribution client for pulling manifests and blobs
- `cache.rs`: In-memory blob cache implementation
- `persistent_cache.rs`: Persistent disk-based cache (feature: `persistent`)
- `s3_cache.rs`: S3-compatible storage backend (feature: `s3`)
- `unpack.rs`: OCI layer unpacking utilities
- `wasm.rs`: WASM artifact detection and types
- `local_registry.rs`: Local OCI registry (feature: `local`)
- `oci_export.rs`: OCI image export/import (feature: `local`)

## Media Types

### WASM Config Media Types

| Media Type | Description |
|------------|-------------|
| `application/vnd.wasm.config.v1+json` | Standard WASM config |
| `application/vnd.wasm.config.v0+json` | Legacy WASM config |

### WASM Layer Media Types

| Media Type | Description |
|------------|-------------|
| `application/vnd.wasm.content.layer.v1+wasm` | Standard WASM layer |
| `application/wasm` | Generic WASM binary |

### WASM Artifact Types (OCI 1.1+)

| Media Type | WASI Version |
|------------|--------------|
| `application/vnd.wasm.component.v1+wasm` | Preview 2 (wasip2) |
| `application/vnd.wasm.module.v1+wasm` | Preview 1 (wasip1) |

## Helper Methods

### ArtifactType

```rust
impl ArtifactType {
    /// Returns true if this is a WASM artifact
    fn is_wasm(&self) -> bool;

    /// Returns true if this is a container image
    fn is_container(&self) -> bool;

    /// Returns the WASI version if this is a WASM artifact
    fn wasi_version(&self) -> Option<&WasiVersion>;
}
```

### WasiVersion

```rust
impl WasiVersion {
    /// Returns true if this is wasip1
    fn is_preview1(&self) -> bool;

    /// Returns true if this is wasip2 (component model)
    fn is_preview2(&self) -> bool;

    /// Returns the target triple suffix for this WASI version
    fn target_triple_suffix(&self) -> &'static str;
    // Returns: "wasm32-wasip1", "wasm32-wasip2", or "wasm32-wasi"
}
```

## Examples

### Detecting WASM in a Pipeline

```rust
use zlayer_registry::wasm::{detect_artifact_type, ArtifactType};
use zlayer_registry::{ImagePuller, BlobCache, RegistryAuth};

async fn handle_image(image: &str) -> Result<(), Box<dyn std::error::Error>> {
    let cache = BlobCache::open("/var/lib/zlayer/cache.redb")?;
    let puller = ImagePuller::new(cache);
    let auth = RegistryAuth::Anonymous;

    // Fetch manifest
    let manifest = puller.fetch_manifest(image, &auth).await?;

    // Check artifact type
    match detect_artifact_type(&manifest) {
        ArtifactType::Container => {
            // Use container runtime (youki/Docker)
            pull_and_run_container(image).await?;
        }
        ArtifactType::Wasm { wasi_version } => {
            // Use WASM runtime (wasmtime)
            let (wasm_bytes, _) = puller.pull_wasm(image, &auth).await?;
            run_wasm_module(&wasm_bytes, wasi_version).await?;
        }
    }

    Ok(())
}
```

## License

Apache-2.0
