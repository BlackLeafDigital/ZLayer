//! HTTP client library for the `ZLayer` daemon's REST API.
//!
//! This crate exposes [`DaemonClient`], a typed HTTP client that talks to a
//! running `zlayer serve` daemon, plus the on-disk [`session`] machinery used
//! to attach `Authorization: Bearer <token>` headers.
//!
//! On Unix platforms the transport is HTTP over a Unix-domain socket
//! (platform-dependent path; see `default_socket_path`). On Windows the
//! transport is HTTP over TCP on `127.0.0.1:3669`.
//!
//! It was extracted from `bin/zlayer` so library crates (`zlayer-docker`,
//! `zlayer-py`, future language SDKs) can embed the same daemon client
//! without depending on the CLI binary.

pub mod session;

mod daemon_client;

pub use daemon_client::{default_socket_path, DaemonClient, DaemonReachability};

// Re-export the streaming-endpoint wire DTOs so SDK consumers using
// `DaemonClient::stream_container_logs` / `stream_container_stats` /
// `stream_image_pull` (Tasks 3.2.3 / 3.3.2 / 3.5.2) don't need a direct
// dependency on the heavy `zlayer-agent` crate just to deserialize the
// daemon's NDJSON output. Mirror the shapes defined in
// `zlayer_agent::runtime` 1:1.
pub use daemon_client::{LogChannel, LogChunk, LogsStreamOptions, PullProgress, StatsSample};

// Re-export the exec-instance wire DTOs and PTY connection helpers so SDK
// consumers driving the Docker-style exec flow (`create_exec`,
// `start_exec_pty`, `inspect_exec`, `resize_exec`, `resize_container`) can
// build the `ExecOptions` body and decode the inspect response without
// pulling in `zlayer-agent` or `zlayer-api`.
pub use daemon_client::{
    CreateExecResponse, ExecContainerRef, ExecInstanceJson, ExecOptions, ExecPtyConnection,
    ExecPtyWriter,
};

// Re-export the registry-credential type used by `stream_image_pull`'s
// `auth` parameter, again so SDK consumers can build a request without
// pulling in `zlayer-agent` or the `zlayer-types::spec` module path.
pub use zlayer_types::spec::RegistryAuth;

// Re-export the long-tail image-management DTOs used by the Docker compat
// shim (`commit`, `import`, `history`, `search`) so callers don't need a
// direct dependency on `zlayer-types`.
pub use zlayer_types::api::images::{
    CommitContainerRequest, CommitContainerResponse, ImageHistoryEntryDto, ImageSearchResultDto,
    ImportImageResponse,
};

// Re-export the wire DTOs (now defined in `zlayer-types`) at the crate
// root so downstream callers using `zlayer_client::{Session, BuildSpec,
// BuildHandle}` keep working.
pub use zlayer_types::client::{BuildHandle, BuildSpec, Session};

// Re-export the daemon event-bus wire types so callers consuming
// `DaemonClient::events_stream` (notably the Docker-compat `/events`
// translator) don't need to depend on `zlayer-api` directly. The types
// live in `zlayer-api::event_bus` because the daemon's broadcast bus is
// defined there; `zlayer-client` already depends on `zlayer-api` for
// other request/response DTOs, so re-exporting here is dependency-free.
pub use zlayer_api::event_bus::{
    ContainerEvent, ContainerEventKind, DaemonEvent, ImageEvent, ImageEventKind, NetworkEvent,
    NetworkEventKind, VolumeEvent, VolumeEventKind,
};
