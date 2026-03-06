//! Network implementations for Raft RPC communication.
//!
//! Provides an HTTP-based RPC client and server using **postcard2** serialization
//! for maximum throughput (70-90% smaller payloads vs JSON, 4x faster).

pub mod http_client;
pub mod http_service;
