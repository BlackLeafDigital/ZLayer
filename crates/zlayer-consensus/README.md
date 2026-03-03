# zlayer-consensus

Shared Raft consensus library built on [openraft 0.9](https://docs.rs/openraft) for ZLayer (container orchestration) and ZDB (database replication).

## Architecture

```
                     +------------------------------+
                     |       Application Layer       |
                     |  (ZLayer scheduler / ZDB)     |
                     +------+----------+------------+
                            |          |
                     +------v---+ +----v-----------+
                     |  propose | | read_state()   |
                     |  (write) | | (linearizable) |
                     +------+---+ +----+-----------+
                            |          |
              +-------------v----------v-------------+
              |         ConsensusNode<TC>             |
              |  - bootstrap / add_voter / shutdown   |
              |  - wraps openraft::Raft<TC>           |
              +---+-------------+--+-----------------+
                  |             |  |
        +---------v--+  +------v--v--------+
        | Log Store  |  | State Machine    |
        | (v2 API)   |  | (v2 API)         |
        +-----+------+  +--------+---------+
              |                   |
    +---------+-------+  +-------+--------+
    | MemLogStore     |  | MemStateMachine|   <-- mem-store (default)
    | RedbLogStore    |  | RedbStateMachine|  <-- redb-store (optional)
    +-----------------+  +----------------+
              |
    +---------v---------+
    |  HttpNetwork      |  <-- bincode over HTTP
    |  (RaftNetworkFactory)|
    +-------------------+
              |
    +---------v-----------+
    |  raft_service_router |  <-- Axum endpoints
    |  POST /raft/vote     |
    |  POST /raft/append   |
    |  POST /raft/snapshot |
    +---------------------+
```

## Quick Start (MemStore for testing)

```rust
use std::io::Cursor;
use zlayer_consensus::{ConsensusNodeBuilder, ConsensusConfig};
use zlayer_consensus::storage::mem_store::{MemLogStore, MemStateMachine};
use zlayer_consensus::network::http_client::HttpNetwork;
use serde::{Serialize, Deserialize};

// 1. Define your app types
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum MyRequest {
    #[default]
    Noop,
    Set { key: String, value: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MyResponse {
    pub ok: bool,
}

// 2. Declare the TypeConfig
openraft::declare_raft_types!(
    pub MyTypeConfig:
        D = MyRequest,
        R = MyResponse,
);

// 3. Create stores + network
let log_store = MemLogStore::<MyTypeConfig>::new();
let sm = MemStateMachine::<MyTypeConfig, HashMap<String, String>, _>::new(
    |state, req: &MyRequest| {
        match req {
            MyRequest::Set { key, value } => {
                state.insert(key.clone(), value.clone());
                MyResponse { ok: true }
            }
            _ => MyResponse { ok: false },
        }
    },
);
let network = HttpNetwork::<MyTypeConfig>::new();

// 4. Build node
let node = ConsensusNodeBuilder::new(1, "127.0.0.1:9000".into())
    .with_config(ConsensusConfig::default())
    .build_with(log_store, sm, network)
    .await?;

// 5. Bootstrap (first node only)
node.bootstrap().await?;

// 6. Propose writes
node.propose(MyRequest::Set {
    key: "hello".into(),
    value: "world".into(),
}).await?;
```

## Production Setup (RedbStore)

Enable the `redb-store` feature:

```toml
[dependencies]
zlayer-consensus = { path = "../zlayer-consensus", features = ["redb-store"] }
```

```rust
use zlayer_consensus::storage::redb_store::{RedbLogStore, RedbStateMachine};

let log_store = RedbLogStore::<MyTypeConfig>::new("/data/raft-log.redb")?;
let sm = RedbStateMachine::<MyTypeConfig, MyState, _>::new(
    "/data/raft-sm.redb",
    |state, req| { /* apply logic */ },
)?;
```

## Defining Your TypeConfig

Use `openraft::declare_raft_types!` to define the type configuration:

```rust
openraft::declare_raft_types!(
    pub MyTypeConfig:
        D = MyRequest,           // Log entry payload (application commands)
        R = MyResponse,          // Response type from state machine
        NodeId = u64,            // (default)
        Node = BasicNode,        // (default)
        Entry = Entry<Self>,     // (default)
        SnapshotData = Cursor<Vec<u8>>, // (default)
);
```

Requirements for `D` (request type):
- `Clone + Debug + Default + Serialize + Deserialize + Send + Sync + 'static`

Requirements for `R` (response type):
- `Clone + Debug + Default + Serialize + Deserialize + Send + Sync + 'static`

## Configuration Tuning

| Parameter | Default | Description |
|-----------|---------|-------------|
| `election_timeout_min_ms` | 1500 | Min election timeout (7.5x heartbeat) |
| `election_timeout_max_ms` | 3000 | Max election timeout (15x heartbeat) |
| `heartbeat_interval_ms` | 200 | Leader heartbeat interval |
| `snapshot_logs_since_last` | 10,000 | Entries before triggering snapshot |
| `max_payload_entries` | 300 | Max entries per AppendEntries RPC |
| `enable_prevote` | true | Prevents partitioned node disruption |
| `rpc_timeout` | 5s | Timeout for vote/append RPCs |
| `snapshot_timeout` | 60s | Timeout for snapshot transfers |

**WAN deployments**: Multiply all timeouts by 3-5x.

## Performance Characteristics

- **Serialization**: bincode (70-90% smaller than JSON, 4x faster)
- **Persistent storage**: redb (~15K durable writes/sec on SSD)
- **In-memory storage**: Limited only by memory bandwidth
- **Network**: HTTP/1.1 with connection pooling (reqwest)
- **PreVote**: Enabled by default to prevent term inflation from partitioned nodes

## Storage API

This crate uses the **openraft v2 storage API** (`RaftLogStorage` + `RaftStateMachine`)
which splits log and state machine operations into separate traits for better concurrency.
This is NOT the deprecated v1 `RaftStorage` + `Adaptor` pattern.

## Future: QUIC Upgrade

The HTTP transport is suitable for LAN and moderate-WAN deployments. For
high-throughput WAN scenarios, a QUIC-based transport will provide:
- Multiplexed streams (no head-of-line blocking)
- 0-RTT connection establishment
- Built-in encryption without TLS handshake overhead
- Better congestion control for lossy networks

This is planned as an additional network backend alongside HTTP.

## License

Apache-2.0
