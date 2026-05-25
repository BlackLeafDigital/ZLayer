# Overlay Architecture

## Overview

ZLayer's v0.51 overlay collapses what used to be a forest of per-service WireGuard TUN devices into a single per-cluster WireGuard interface (`zl-overlay0`) that carries every service's traffic via multi-CIDR `AllowedIPs`. Container attachment moves to a per-service Linux bridge (`br-svc-<hash>`) on each node, putting ZLayer in line with how CNI plugins (Cilium, Calico, Flannel) model their data path. Interface count drops from `O(services x nodes)` to `O(services + 1)` per node.

## Per-cluster WireGuard

Every node always brings up `zl-overlay0`, even when running standalone. On Linux and macOS the data plane is userspace boringtun; on Windows the underlying transport is HCN. The interface holds one cryptographic peer per remote node, and each peer's `AllowedIPs` is rewritten as services come and go: every subnet currently allocated to that remote node (across every service) appears as a CIDR entry. There is no per-service WG device anywhere in the stack.

## Per-service Linux bridge

On Linux, each service gets its own bridge per node: `br-svc-<hash>`, where the hash is derived from the service name. The bridge holds the L3 gateway address (the first usable IP in the per-(service, node) subnet); containers' veth-host ends are added as bridge members via `netlink::add_link_to_bridge`, and the container's default route points at the bridge gateway. The previous `/32` host-route-per-container scheme is gone.

## Subnet allocation

`ServiceSubnetRegistry` (in `zlayer-overlay::allocator`) carves the cluster CIDR (default `/16`) into `/28` slots, one per `(service, node)` pair. Slot selection uses FNV-1a hashing of the `(service, node)` tuple with linear probing on collision, so allocations are deterministic and stable across leader changes. The registry snapshots and restores through scheduler Raft state.

## Cross-node propagation

Two new Raft commands move allocations across the cluster:

- `AssignServiceSubnet { service, node, subnet }` — leader allocates, every node applies.
- `ReleaseServiceSubnet { service, node }` — symmetric teardown.

On apply, every node's handler calls `OverlayTransport::add_allowed_ip(remote_pubkey, subnet)` (or the release equivalent) against its local cluster transport, so cross-node routing converges without an extra control-plane round trip.

> **TODO (currently inert in production):** the apply handlers exist in `crates/zlayer-scheduler/src/raft.rs`, but `OverlayManager::setup_service_overlay` doesn't yet call `AssignServiceSubnet` — it uses a local-fallback `ServiceSubnetRegistry` that's correct for single-node and exercised by test simulations only. Production wiring also requires plumbing `OverlayManager` into `InternalState` in `bin/zlayer/src/commands/serve.rs`; until that one-line change lands, a latent `OverlayTransport::new()` fix in the internal peer-add path is "available but inert."

## Lifecycle and cleanup

- **Detach-on-exit:** when a container exits, its veth is removed from the service bridge.
- **Teardown-on-down:** when a service is torn down, the bridge and any remaining members are removed.
- **Daemon-shutdown drop:** all overlay state is released cleanly on graceful shutdown.
- **60s orphan sweep:** stale veths with no matching container are reaped.
- **Bridge sweep:** bridges with no remaining members are removed.

## OverlayMode reserved variants

`OverlayMode { Auto, Shared, Dedicated }` lives in `zlayer-types`. v0.51 always resolves to `Shared`:

- `Auto` (default) will eventually pick between modes based on bandwidth telemetry, NIC capabilities, and interface-count ceilings. Today it resolves to `Shared`.
- `Shared` is the model described above and is fully implemented.
- `Dedicated` is reserved for a future round. When the shared crypto context (single boringtun device) becomes the bandwidth bottleneck for a service, `Dedicated` will restore a per-service WG TUN as an opt-out. Today, setting `Dedicated` warns and falls back to `Shared`.

## ASCII diagram

```
3-node cluster, 2 services (svcA, svcB). Cluster CIDR 10.42.0.0/16.

                 node1 (pubkey K1)                node2 (pubkey K2)                node3 (pubkey K3)
                 ------------------               ------------------               ------------------
  zl-overlay0   [WG: K1]                         [WG: K2]                         [WG: K3]
                  peers:                           peers:                           peers:
                    K2 AllowedIPs=                   K1 AllowedIPs=                   K1 AllowedIPs=
                      10.42.10.16/28  (svcA@n2)        10.42.10.0/28  (svcA@n1)         10.42.10.0/28  (svcA@n1)
                      10.42.20.16/28  (svcB@n2)        10.42.20.0/28  (svcB@n1)         10.42.20.0/28  (svcB@n1)
                    K3 AllowedIPs=                   K3 AllowedIPs=                   K2 AllowedIPs=
                      10.42.10.32/28  (svcA@n3)        10.42.10.32/28  (svcA@n3)        10.42.10.16/28  (svcA@n2)
                      10.42.20.32/28  (svcB@n3)        10.42.20.32/28  (svcB@n3)        10.42.20.16/28  (svcB@n2)

  svcA bridge   br-svc-<A>  (gw 10.42.10.1/28)   br-svc-<A>  (gw 10.42.10.17/28)  br-svc-<A>  (gw 10.42.10.33/28)
                  veth-c1  -> ctr svcA-1            veth-c3  -> ctr svcA-3           veth-c5  -> ctr svcA-5
                  veth-c2  -> ctr svcA-2            veth-c4  -> ctr svcA-4

  svcB bridge   br-svc-<B>  (gw 10.42.20.1/28)   br-svc-<B>  (gw 10.42.20.17/28)  br-svc-<B>  (gw 10.42.20.33/28)
                  veth-d1  -> ctr svcB-1            veth-d2  -> ctr svcB-2           veth-d3  -> ctr svcB-3
```

Each node sees a single WG interface with two peers; each peer carries both services' subnets in `AllowedIPs`. Per-service bridges are local to each node and hold disjoint `/28` slices of the cluster CIDR.
