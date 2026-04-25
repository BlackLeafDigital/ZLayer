# zlayer-client

Internal HTTP client for the ZLayer daemon's REST API.

## Overview

`zlayer-client` is the Rust library that every ZLayer command-line and SDK
front-end uses to talk to a running `zlayer serve` daemon. It hides the
transport differences between platforms — HTTP over a Unix-domain socket on
Linux/macOS and HTTP over a loopback TCP listener on Windows — behind one
typed `DaemonClient` and a small persistent session file. The crate was
extracted from `bin/zlayer` so out-of-tree consumers (the Docker shim, the
Python bindings, future language SDKs) can embed the same client without
pulling in the entire CLI binary.

`publish = false`: this is a workspace-internal crate. It is not published
to crates.io and its API is allowed to change.

## Public API

The crate re-exports its public surface from the root:

- [`DaemonClient`](src/daemon_client.rs) — the typed HTTP client. It owns
  the connection (`connect`, `connect_to`, `try_connect`, `try_connect_to`)
  and exposes one method per daemon endpoint: `health_check`,
  `health_ready`, `create_deployment`, `get_deployment`,
  `watch_deployment`, `delete_deployment`, `list_deployments`,
  `list_services`, `scale_service`, `get_logs` /
  `get_logs_with_instance` / `get_logs_streaming`, `list_containers`,
  `exec_command` / `exec_command_with_replica`, `list_images`,
  `remove_image`, `prune_images`, `push_image`, `inspect_image`,
  `pull_image_from_server`, `tag_image`, `get_token`, and more. Streaming
  endpoints (logs, builds, exec) return `tokio_stream::Stream` types.
- [`default_socket_path`](src/daemon_client.rs) — resolves the platform
  default daemon endpoint. Delegates to
  [`zlayer_paths::ZLayerDirs::default_socket_path`].
- [`BuildSpec`, `BuildHandle`](src/daemon_client.rs) — the request/handle
  pair returned by the `build` family of methods on `DaemonClient`.
- [`session::Session`](src/session.rs) — the on-disk session record
  (`token`, `email`, `expires_at`) plus `is_expired`.
- [`session::default_session_path`](src/session.rs),
  [`session::read_session`](src/session.rs),
  [`session::write_session`](src/session.rs),
  [`session::delete_session`](src/session.rs) — read/write
  `~/.zlayer/session.json` (mode `0600` on Unix). When a session is
  present, `DaemonClient` attaches `Authorization: Bearer <token>` to
  every outgoing request.

The wire types (`ImageInfoDto`, `PruneResultDto`, etc.) come from
`zlayer-api`, so request/response shapes stay in lockstep with the
server.

## Use from ZLayer

- `bin/zlayer` — every CLI subcommand under `bin/zlayer/src/commands/`
  that needs to reach the daemon (deploy, ps, logs, exec, image
  push/pull, build, etc.) goes through `DaemonClient`.
- `crates/zlayer-docker` — the Docker-API compatibility shim
  translates incoming Docker requests into `DaemonClient` calls so
  `docker` clients can drive a ZLayer daemon unchanged.
- `crates/zlayer-py` — the Python bindings expose `DaemonClient`
  methods to Python callers via PyO3.

## Platform notes

- **Linux / macOS**: the daemon listens on a Unix-domain socket
  (`/var/run/zlayer.sock` for system installs, otherwise under the
  per-user data dir). The local-admin bearer is auto-injected by the
  daemon's UDS middleware, so a local CLI typically authenticates
  without needing a session file.
- **Windows**: the daemon listens on `tcp://127.0.0.1:3669`. There is
  no socket-path-based local-admin bypass on this transport, so
  `DaemonClient` reads the persisted local-admin bearer
  (`zlayer_paths::ZLayerDirs::admin_bearer_path`) on connect and uses
  it for the loopback handshake.

The platform split is implemented with `cfg(unix)` / `cfg(windows)`
arms inside `daemon_client.rs`.

## When to edit this crate

- You added or changed a daemon REST endpoint in `zlayer-api` and
  every CLI/SDK consumer needs a typed wrapper for it.
- You changed the on-disk session format or the session file location.
- You changed how the local-admin bearer is discovered or injected
  for the Windows transport.
- You added a new transport (e.g. mTLS to a remote daemon) — wire it
  in here so every consumer picks it up at once.
