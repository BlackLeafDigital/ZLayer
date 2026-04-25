# zlayer-hns

Safe Rust wrapper for the Windows Host Compute Network Service (HCN v2).

## Overview

HCN is the networking half of the Windows container stack — companion to
the Host Compute Service (HCS) wrapped in `zlayer-hcs`. The underlying
surface lives in `computenetwork.dll`. This crate is the authoritative HCN
FFI boundary for ZLayer: it wraps the raw entry points behind RAII handle
types, JSON schema types matching `hcsshim/hcn` v2, and high-level helpers
for the common case of attaching a container to an overlay network through
an HCN namespace.

It backs Windows container networking in `zlayer-overlay` and
`zlayer-agent` — endpoint creation, network attach, and the HNS endpoint
DNS plumbing that makes service discovery work for Windows containers.

## Platform

Windows only. The crate root is gated with `#![cfg(windows)]`, so on every
other platform it compiles to an empty stub and callers can depend on it
unconditionally.

The crate carries `#![allow(unsafe_code)]` because it is the single allowed
location in the workspace for `computenetwork.dll` FFI plus
`GetAdaptersAddresses` traversal. Every `unsafe` site has a `SAFETY:`
comment.

## Public API

One module per HCN concern:

- `zlayer_hns::network::Network` — RAII handle for an HCN network:
  `create`, `open`, `delete`, `modify`, `query`, `create_transparent` (the
  helper that creates the L2-bridged network ZLayer's overlay attaches to),
  `id`, `handle`. Module-level `list(query_json)` returns all network
  GUIDs.
- `zlayer_hns::endpoint::Endpoint` — RAII handle for an HCN endpoint:
  `create`, `open`, `delete`, `modify`, `query_properties`, `query_stats`,
  `primary_ip`, `id`, `handle`. Module-level `list(query_json)` returns all
  endpoint GUIDs.
- `zlayer_hns::namespace::Namespace` — RAII handle for an HCN namespace
  (the HCS-side "netns" abstraction): `create_host_default`, `create`,
  `open`, `delete`, `add_endpoint`, `remove_endpoint`, `modify_json`,
  `query`, `list_endpoints`, `id`, `handle`. Module-level `list` returns
  namespace GUIDs.
- `zlayer_hns::attach::EndpointAttachment` — High-level helper that bundles
  endpoint + namespace creation for the overlay-attach path.
  `create_overlay(...)` wires up an endpoint on the chosen network with a
  fresh namespace; `teardown(self)` reverses both. `endpoint_id`,
  `namespace_id`, and `ip` accessors expose the ids needed by
  `HcsRuntime`. Module-level helpers `list_owned_endpoints(owner_tag)` and
  `delete_endpoint_and_namespace(endpoint_id, namespace_id)` cover crash-
  recovery cleanup.
- `zlayer_hns::adapter::find_primary_adapter` plus the `ZLAYER_UPLINK_ENV`
  constant (`"ZLAYER_HCN_UPLINK_ADAPTER"`) — Picks (or honors an override
  for) the host NIC the transparent network bridges through.
- `zlayer_hns::error::{HnsError, HnsResult}` — `thiserror`-based error type
  classifying HCN HRESULT codes.
- `zlayer_hns::handle` — Raw HCN handle wrappers with `Drop` impls
  (`OwnedNetwork`, `OwnedEndpoint`, `OwnedNamespace`).
- `zlayer_hns::schema` — `serde`-driven schema v2 types (`HostComputeNetwork`,
  `HostComputeEndpoint`, `HostComputeNamespace`, `EndpointStats`, ...).

## Use from ZLayer

Consumed primarily by:

- `zlayer-agent` — `HcsRuntime` invokes `EndpointAttachment::create_overlay`
  at container-create time so each Windows container lands on the ZLayer
  overlay with the right IP, DNS, and policy. The same path handles HNS
  endpoint DNS plumbing so service-discovery names resolve inside Windows
  containers.
- `zlayer-overlay` — Coordinates IP allocation against the HCN network's
  subnet so the agent's allocator and the HNS-side state agree.

This is an internal ZLayer crate. The workspace `version = "0.0.0-dev"` is
unpublished; consume it via the workspace path dependency, not crates.io.

## When to edit this crate

Edit `zlayer-hns` when you need to:

- Wrap a new `computenetwork.dll` entry point (always behind a safe API +
  a `SAFETY:` comment).
- Extend the v2 schema to expose a new HCN field.
- Change how transparent networks are constructed (`Network::create_transparent`).
- Adjust the overlay-attach contract — endpoint/namespace creation order,
  DNS records on the endpoint, IP plumbing — used by `HcsRuntime`.
- Fix uplink-adapter selection in `adapter::find_primary_adapter`.

Overlay routing, IP allocation, and WireGuard tunnels live in
`zlayer-overlay`, not here.

Repository: <https://github.com/BlackLeafDigital/ZLayer>
