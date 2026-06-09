# Known issues: Linux `zlayer build`

This page collects host-side gotchas that surface during `zlayer build` on
Linux. None of these are zlayer bugs — they live in the buildah/netavark
toolchain we shell out to — but they can block local development and are
worth knowing.

## buildah 1.44.0 + netavark 1.17.2 — malformed network options

### Symptom

```
Build failed: error running container: did not get container start message from parent: EOF
Error: setup network: netavark: failed to load network options:
       IO error: invalid type: sequence, expected a map at line 1 column 101
```

The build fails at the very first `buildah run -- mkdir -p /build` (the
implicit run that materialises `workdir:` inside the planner stage). Every
subsequent attempt — `sudo -E`, `--host-network`, fresh containers — hits
the same error.

### Root cause

The build container itself never starts because netavark refuses the JSON
network-options blob buildah pipes to it. Empirically (via a logging shim
on `/usr/lib/podman/netavark`) the blob is well-formed JSON, ~560 bytes,
and the byte at column ~101 is the `[` opening the `networks` array.

- buildah 1.44.0 emits `networks: [{"name": "podman", ...}]` (a JSON array).
- netavark 1.17.2 expects `networks: {"podman": {...}}` (a JSON object / map).

Netavark `main` (untagged, post-1.17.2) added a custom serde adapter
(`deserialize_network_options_map_or_vec`) that accepts BOTH shapes — but
no tagged release ships that adapter yet. Stock Arch/CachyOS ships
netavark 1.17.2, so every fresh install with buildah 1.44.0 hits this.

Plain `sudo -E buildah run alpine-working-container -- echo hi` reproduces
the error with zero zlayer in the call chain — it is a buildah↔netavark
version skew, not a zlayer bug.

### Workarounds (pick one)

1. **`zlayer build --host-network` (recommended for local dev).**
   Forces `--net=host` on every `buildah run` zlayer issues, which makes
   the build container share the host's network namespace and bypasses
   netavark entirely. Matches `docker build --network=host` semantics.
   See `bin/zlayer/src/cli.rs` (the `--host-network` flag) and
   `crates/zlayer-builder/src/buildah/mod.rs` (the
   `DockerfileTranslator::with_host_network` path).

2. **Upgrade netavark from `main`.** Build the binary from the
   `containers/netavark` `main` branch and install it over
   `/usr/lib/podman/netavark` (or wherever your distro puts it). When a
   1.17.3 / 1.18.x release ships with the dual-deserializer, install that
   instead.

3. **Downgrade buildah.** Earlier buildah releases still serialise
   `networks` as a map; you can revert until upstream catches up. Check
   `gh release list -R containers/buildah` for tags older than 1.44.0.

4. **Build off-host.** Run the build on a Forgejo Actions runner, a Mac
   (HCS / Lima path), or any known-good Linux box. The image lands in
   the registry and your local zlayer pulls from there.

## Future work

A future branch may replace the buildah subprocess with an embedded
BuildKit client (`buildkit-rs` or shelling out to `buildctl`) — BuildKit
has its own network sandbox and would not be affected by buildah/netavark
version skew at all. That's tracked separately and is not in scope here.
