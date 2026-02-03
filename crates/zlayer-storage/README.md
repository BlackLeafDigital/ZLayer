# zlayer-storage

S3-backed container layer persistence with crash-tolerant uploads and SQLite replication.

## Features

- **Layer Persistence** - Persist container OverlayFS upper layers to S3
- **Crash-Tolerant Uploads** - Multipart S3 uploads with resume capability
- **SQLite State Tracking** - Track sync state in SQLite (via SQLx)
- **SQLite S3 Replication** - WAL-based SQLite replication to S3 with auto-restore
- **Compression** - Zstd compression for efficient storage

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
zlayer-storage = "0.8"
```

## Usage

### Layer Sync Manager

Persist container layers to S3:

```rust
use zlayer_storage::{LayerSyncManager, LayerStorageConfig};

let config = LayerStorageConfig {
    bucket: "my-bucket".to_string(),
    prefix: "layers/".to_string(),
    region: Some("us-west-2".to_string()),
    endpoint_url: None, // Or custom S3-compatible endpoint
    staging_dir: "/var/lib/zlayer/staging".into(),
    state_db_path: "/var/lib/zlayer/layer-state.sqlite".into(),
    ..Default::default()
};

let manager = LayerSyncManager::new(config).await?;

// Sync a container's upper layer to S3
let snapshot = manager.sync_layer(&container_id, "/path/to/upper").await?;

// Restore a layer from S3
let restored = manager.restore_layer(&container_id, "/path/to/restore").await?;
```

### SQLite S3 Replicator

Replicate any SQLite database to S3 with WAL-based incremental backups:

```rust
use zlayer_storage::{SqliteReplicator, SqliteReplicatorConfig, LayerStorageConfig};

let replicator_config = SqliteReplicatorConfig {
    db_path: "/var/lib/zlayer/state.sqlite".into(),
    s3_bucket: "my-bucket".to_string(),
    s3_prefix: "sqlite-backups/".to_string(),
    cache_dir: "/var/lib/zlayer/replicator-cache".into(),
    max_cache_size: 100 * 1024 * 1024, // 100MB
    auto_restore: true,
    snapshot_interval_secs: 3600, // 1 hour
};

let s3_config = LayerStorageConfig { /* ... */ };
let replicator = SqliteReplicator::new(replicator_config, &s3_config).await?;

// Start background replication
replicator.start().await?;

// Check status
let status = replicator.status();
println!("Last sync: {:?}", status.last_sync);

// Force sync before shutdown
replicator.flush().await?;
```

### Auto-Restore

If `auto_restore: true` and the local database is missing, it will be automatically restored from S3:

```rust
// Database will be restored from S3 if missing
let replicator = SqliteReplicator::new(config, &s3_config).await?;

// Or manually restore
replicator.restore().await?;
```

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      zlayer-storage                          │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌────────────────────┐    ┌────────────────────────────┐  │
│  │  LayerSyncManager  │    │    SqliteReplicator        │  │
│  │  - Multipart upload│    │    - WAL monitoring        │  │
│  │  - Resume support  │    │    - Local write cache     │  │
│  │  - Restore layers  │    │    - Async S3 shipping     │  │
│  └─────────┬──────────┘    └─────────────┬──────────────┘  │
│            │                              │                  │
│  ┌─────────▼──────────────────────────────▼──────────────┐ │
│  │                 SQLite (SQLx Pool)                     │ │
│  │                 - WAL mode                             │ │
│  │                 - State persistence                    │ │
│  └────────────────────────────────────────────────────────┘ │
│                              │                               │
└──────────────────────────────┼───────────────────────────────┘
                               │ async
               ┌───────────────▼───────────────┐
               │           S3 Bucket           │
               │  - Layer tarballs (.tar.zst)  │
               │  - SQLite snapshots           │
               │  - WAL segments               │
               │  - Metadata                   │
               └───────────────────────────────┘
```

## S3 Key Structure

### Layer Storage
```
s3://{bucket}/{prefix}/
├── {digest}.tar.zst      # Compressed layer tarball
└── {digest}.meta.json    # Layer metadata
```

### SQLite Replication
```
s3://{bucket}/{prefix}/
├── snapshots/
│   └── {timestamp}.sqlite.zst   # Full database snapshots
├── wal/
│   └── {sequence:020}.wal.zst   # WAL segments
└── metadata.json                 # Replication state
```

## Configuration

### LayerStorageConfig

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `bucket` | String | required | S3 bucket name |
| `prefix` | String | `"layers/"` | S3 key prefix |
| `region` | Option<String> | None | AWS region |
| `endpoint_url` | Option<String> | None | Custom S3 endpoint |
| `staging_dir` | PathBuf | `/var/lib/zlayer/staging` | Local staging directory |
| `state_db_path` | PathBuf | `/var/lib/zlayer/layer-state.sqlite` | SQLite state DB |
| `part_size_bytes` | u64 | 64MB | Multipart part size |
| `max_concurrent_uploads` | usize | 4 | Concurrent uploads |
| `compression_level` | i32 | 3 | Zstd compression (1-22) |

### SqliteReplicatorConfig

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `db_path` | PathBuf | required | SQLite database path |
| `s3_bucket` | String | required | S3 bucket for backups |
| `s3_prefix` | String | required | S3 key prefix |
| `cache_dir` | PathBuf | required | Local cache directory |
| `max_cache_size` | u64 | 100MB | Max cache before blocking |
| `auto_restore` | bool | true | Auto-restore if DB missing |
| `snapshot_interval_secs` | u64 | 3600 | Snapshot interval |

## S3-Compatible Services

Works with any S3-compatible storage:

```rust
// MinIO
let config = LayerStorageConfig {
    endpoint_url: Some("http://minio:9000".to_string()),
    ..Default::default()
};

// Cloudflare R2
let config = LayerStorageConfig {
    endpoint_url: Some("https://{account_id}.r2.cloudflarestorage.com".to_string()),
    ..Default::default()
};

// Backblaze B2
let config = LayerStorageConfig {
    endpoint_url: Some("https://s3.{region}.backblazeb2.com".to_string()),
    ..Default::default()
};
```

## License

MIT - See [LICENSE](../../LICENSE) for details.
