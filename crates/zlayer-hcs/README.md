# zlayer-hcs

Safe Rust wrapper for the Windows Host Compute Service (HCS).

## Overview

HCS is the OS-level API powering Windows containers and Hyper-V-isolated
workloads. The underlying surface lives in `vmcompute.dll`. This crate is
the authoritative HCS FFI boundary for ZLayer: it wraps the raw entry points
behind RAII handle types, async helpers that translate HCS's native
completion-callback model into Rust futures, and `serde` JSON schema types
that match the current `hcsshim` schema v2.

It is the foundation that `zlayer-agent`'s `HcsRuntime` is built on — the
runtime ZLayer uses to start, stop, and inspect Windows containers natively
without Docker Desktop.

## Platform

Windows only. The crate root is gated with `#![cfg(windows)]`, so on every
other platform it compiles to an empty stub. Callers can depend on it
unconditionally.

The crate carries `#![allow(unsafe_code)]` because it is the single allowed
location in the workspace for `vmcompute.dll` FFI. Every `unsafe` site has a
`SAFETY:` comment.

## Public API

The crate exposes one module per HCS concern:

- `zlayer_hcs::system::ComputeSystem` — RAII handle for an HCS compute system
  (a Windows container or VM). Create / open and drive lifecycle: `create`,
  `open`, `start`, `shutdown`, `terminate`, `pause`, `resume`, `save`,
  `properties`, `modify`.
- `zlayer_hcs::process::ComputeProcess` — RAII handle for a process running
  inside an HCS system: `create`, `spawn`, `open`, `signal`, `terminate`,
  `modify`, `resize_console`, `properties`.
- `zlayer_hcs::operation::run_operation` — Bridges HCS's asynchronous
  `HcsCreateOperation` / completion-callback model into a Rust future
  resolving to the result JSON.
- `zlayer_hcs::events::{EventSubscription, HcsEvent, HcsEventKind, subscribe}`
  — Subscribe to system events (state changes, exits) as a stream.
- `zlayer_hcs::enumerate::{EnumeratedSystem, EnumerateQuery, list, list_by_owner}`
  — Enumerate live compute systems on the host.
- `zlayer_hcs::error::{HcsError, HcsResult}` — `thiserror`-based error type
  classifying HCS HRESULT codes (`NotFound`, `AccessDenied`, `InvalidSchema`,
  `Other`, ...).
- `zlayer_hcs::handle::{SendHandle, OwnedSystem, OwnedProcess, OwnedOperation}`
  — Raw HCS handle wrappers with `Drop` impls. `SendHandle<T>` is re-exported
  from the crate root for callers that need to move handles across threads.
- `zlayer_hcs::schema` — `serde`-driven schema v2 types: `ComputeSystem`,
  `Container`, `VirtualMachine`, `Topology`, `Statistics`, `ProcessParameters`,
  `ProcessStatus`, etc. PascalCase wire format matches HCS expectations.
- `zlayer_hcs::stats` — Helpers around the `Statistics` schema for
  observability scrapes.

## Use from ZLayer

Consumed by `zlayer-agent`:

- `crates/zlayer-agent/src/runtimes/hcs.rs` exposes `HcsRuntime`,
  `HcsConfig`, and `IsolationMode`. `HcsRuntime` is selected as the primary
  Windows runtime in `crates/zlayer-agent/src/lib.rs`, with
  `Wsl2DelegateRuntime` (from `zlayer-wsl`) handling Linux-image workloads.
- `crates/zlayer-agent/src/overlay_manager.rs` coordinates DNS / endpoint
  plumbing with the HCS runtime so overlay networks attach correctly at
  container start.

This is an internal ZLayer crate. The workspace `version = "0.0.0-dev"` is
unpublished; consume it via the workspace path dependency, not crates.io.

## Tests

Integration tests live in `tests/integration.rs` and are also
`#[cfg(windows)]`-gated. They include:

- `schema_round_trip` and `parse_statistics_fixture` — schema-only, no HCS.
- `empty_vm_create_and_terminate` — `#[ignore]` end-to-end lifecycle test
  that requires a host with the Hyper-V role enabled and an elevated token.
- `enumerate_self_empty` — calls `list_by_owner` against a synthetic owner
  string.
- `open_nonexistent_process_errors_cleanly` — ensures `ComputeProcess::open`
  on a null `HCS_SYSTEM` returns a classified `HcsError` instead of
  panicking. Skipped automatically when running in Windows services
  session 0 (e.g. as the Forgejo runner service account), where HCS's
  null-handle behavior diverges from interactive sessions.

The test helpers gate live HCS calls behind elevation and session-zero
probes so the suite stays green on unprivileged developer machines and
non-Hyper-V CI runners.

## When to edit this crate

Edit `zlayer-hcs` when you need to:

- Wrap a new `vmcompute.dll` entry point (always behind a safe API + a
  `SAFETY:` comment).
- Extend the schema v2 types to match a newer `hcsshim` schema.
- Adjust how an HCS HRESULT classifies into `HcsError`.
- Add a new event kind to `HcsEventKind` or change subscription wiring.

Container-runtime policy (image pull, mount layout, isolation defaults)
lives in `zlayer-agent/src/runtimes/hcs.rs`, not here.

Repository: <https://github.com/BlackLeafDigital/ZLayer>
