//! # zlayer-consensus
//!
//! A shared Raft consensus library built on [openraft](https://docs.rs/openraft) 0.9.
//! Provides pluggable storage backends and network transports that can be
//! parameterized with any application's state machine types.
//!
//! Used by both **`ZLayer`** (container orchestration) and **Zatabase** (database replication).
//!
//! ## Quick Start
//!
//! ```ignore
//! use zlayer_consensus::{ConsensusNodeBuilder, ConsensusConfig};
//! use zlayer_consensus::storage::mem_store::{MemLogStore, MemStateMachine};
//! use zlayer_consensus::network::http_client::HttpNetwork;
//!
//! // 1. Define your TypeConfig
//! openraft::declare_raft_types!(
//!     pub MyTypeConfig:
//!         D = MyRequest,
//!         R = MyResponse,
//! );
//!
//! // 2. Create storage
//! let log_store = MemLogStore::<MyTypeConfig>::new();
//! let sm = MemStateMachine::<MyTypeConfig, MyState, _>::new(|state, req| {
//!     // apply request to state, return response
//! });
//!
//! // 3. Create network
//! let network = HttpNetwork::<MyTypeConfig>::new();
//!
//! // 4. Build the node
//! let node = ConsensusNodeBuilder::new(1, "127.0.0.1:9000".into())
//!     .with_config(ConsensusConfig::default())
//!     .build_with(log_store, sm, network)
//!     .await?;
//!
//! // 5. Bootstrap (first node only)
//! node.bootstrap().await?;
//!
//! // 6. Propose writes
//! let response = node.propose(MyRequest::Set("key".into(), "value".into())).await?;
//! ```
//!
//! ## Feature Flags
//!
//! | Flag | Default | Description |
//! |------|---------|-------------|
//! | `mem-store` | yes | In-memory BTreeMap-based storage (testing/dev) |
//! | `redb-store` | no | Crash-safe persistent storage via redb |

pub mod config;
pub mod error;
pub mod network;
pub mod node;
pub mod storage;
pub mod types;

// Re-export key items at the crate root for ergonomics.
pub use config::ConsensusConfig;
pub use error::{ConsensusError, Result};
pub use node::{ConsensusNode, ConsensusNodeBuilder};
pub use types::{BasicNode, NodeId};

// Re-export openraft types that consumers will commonly need.
pub use openraft;
