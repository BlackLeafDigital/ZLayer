# zlayer-scheduler

Raft-based distributed scheduler for container orchestration.

## Features

- **Distributed Consensus** - Raft-based leader election and log replication via OpenRaft
- **Adaptive Autoscaling** - Scale services based on CPU, memory, or RPS targets
- **Persistent Storage** - SQLite-backed Raft log and state machine (optional)
- **Metrics Integration** - Prometheus metrics for scaling decisions and cluster health

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
zlayer-scheduler = "0.8"

# With persistent storage
zlayer-scheduler = { version = "0.8", features = ["persistent"] }
```

## Feature Flags

- `persistent`: Enable SQLite-backed persistent Raft storage (via SQLx)
- `test-skip-http`: Skip HTTP calls in tests (for unit testing)

## Usage

### Standalone Mode

```rust
use zlayer_scheduler::{Scheduler, SchedulerConfig};

// Create standalone scheduler (no Raft)
let scheduler = Scheduler::new_standalone(
    SchedulerConfig::default(),
    "internal-token".to_string(),
    "http://localhost:8080".to_string(),
);

// Register a service for autoscaling
scheduler.register_service(
    "api",
    ScaleSpec::Adaptive {
        min: 1,
        max: 10,
        cooldown: Some(Duration::from_secs(60)),
        targets: ScaleTargets {
            cpu: Some(70),
            memory: None,
            rps: None,
        },
    },
    2, // initial replicas
).await?;
```

### Distributed Mode (Raft)

```rust
use zlayer_scheduler::{Scheduler, SchedulerConfig, RaftConfig};

let raft_config = RaftConfig {
    node_id: 1,
    cluster_nodes: vec!["10.0.0.1:8080", "10.0.0.2:8080", "10.0.0.3:8080"],
    data_dir: "/var/lib/zlayer/raft".into(),
};

let scheduler = Scheduler::new_distributed(
    SchedulerConfig::default(),
    raft_config,
    "internal-token".to_string(),
    "http://localhost:8080".to_string(),
).await?;
```

### Persistent Storage

With the `persistent` feature enabled, Raft state is stored in SQLite:

```rust
use zlayer_scheduler::PersistentRaftStorage;

// Create persistent storage
let storage = PersistentRaftStorage::new("/var/lib/zlayer/raft.sqlite").await?;

// Storage includes:
// - Log entries (Raft log)
// - Vote state (current term, voted_for)
// - Snapshot metadata and data
// - State machine applied state
```

**SQLite Configuration:**
- WAL mode for concurrent access
- `synchronous=FULL` for Raft ACID requirements
- 5 connection pool with 30s busy timeout

## Architecture

```
┌─────────────────────────────────────┐
│           Scheduler                  │
├─────────────────────────────────────┤
│  ┌─────────────┐  ┌──────────────┐ │
│  │ Autoscaler  │  │ Raft Coord   │ │
│  └─────────────┘  └──────────────┘ │
│         │                │          │
│  ┌──────▼────────────────▼───────┐ │
│  │      Metrics Collector        │ │
│  └───────────────────────────────┘ │
│                │                    │
│  ┌─────────────▼─────────────────┐ │
│  │   Persistent Raft Storage     │ │
│  │   (SQLite via SQLx)           │ │
│  └───────────────────────────────┘ │
└─────────────────────────────────────┘
```

## License

MIT - See [LICENSE](../../LICENSE) for details.
