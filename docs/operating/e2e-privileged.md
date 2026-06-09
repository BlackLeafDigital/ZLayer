# Privileged-container e2e runner

ZLayer's overlay path (boringtun + TUN + namespace plumbing) cannot be
exercised meaningfully on loopback. `run-suite.py --overlay-mode
container` is the runner mode that boots each test node inside a
privileged container so signed-token joins, rotation, and revocation
flow over a real encrypted overlay.

This document is the operator runbook: host prereqs, env vars,
troubleshooting, and the iteration plan for finishing the
implementation.

## Status

Wave 10 ships the **scaffolding**:

- `images/ZImagefile.zlayer-e2e-node` is built into a privileged-ready
  test-node image, registered in `ZPipeline.yaml`.
- `run-suite.py --overlay-mode container` is wired as an argparse flag
  with a `ZLAYER_E2E_PRIVILEGED=1` env-var gate.
- The harness-side container launch (podman/docker run with
  `--privileged --cap-add NET_ADMIN --device /dev/net/tun`) is the
  remaining work — the flag currently bails with exit 2 and a clear
  message pointing here.

## Host prerequisites

- Linux host with kernel TUN support (`modprobe tun`; `/dev/net/tun`
  present). macOS works under Lima with the `linux` kernel.
- Podman 4.4+ OR Docker 24+ with `--privileged` allowed for the
  invoking user. Rootless podman works for most scenarios; if you see
  `RTNETLINK answers: Operation not permitted` errors when zlayer
  brings up `zlayer0`, fall back to rootful podman or sudo docker.
- `ZLAYER_E2E_PRIVILEGED=1` set in the runner's environment.
- The test image must be available locally:
  ```bash
  zlayer pipeline -f ZPipeline.yaml --set VERSION=dev
  ```
- Recommended: a dedicated bridge network for inter-container traffic
  so overlay packets traverse a real wire and not the docker0/podman0
  shared default:
  ```bash
  docker network create --driver bridge zlayer-e2e
  ```

## Env vars

| Variable | Required | Purpose |
|---|---|---|
| `ZLAYER_E2E_PRIVILEGED` | yes (for container mode) | Acknowledge the privileged-host requirement; runner refuses to start without it. |
| `ZLAYER_E2E_NETWORK` | no (default `zlayer-e2e`) | Name of the bridge network to attach all test containers to. |
| `ZLAYER_E2E_IMAGE` | no (default `zlayer/zlayer-e2e-node:latest`) | Image tag to launch. |
| `ZLAYER_E2E_KEEP_CONTAINERS` | no | If `1`, leaves containers running after the suite for postmortem inspection. |

## Troubleshooting

- **`open /dev/net/tun: no such device`** — host kernel lacks TUN.
  `modprobe tun` and retry. On macOS-under-Lima, ensure the Lima VM
  uses a Linux kernel image, not the Apple Hypervisor BSD variant.
- **`Operation not permitted` on `ip link add zlayer0`** — privileged
  flag missing or rootless podman is too restrictive. Try sudo docker
  or rootful podman.
- **Tokens generated in the host cluster don't validate in a
  containerised node** — verify the test container's clock isn't
  skewed; signed tokens carry `exp` and `iat` and refuse skew >30s.
- **`tcpdump` inside the container reports nothing on `zlayer0`** —
  the overlay interface didn't come up. Check `journalctl -u zlayer`
  inside the container; the daemon's `info!("overlay manager up")`
  log line should fire within ~2s of boot.

## Iteration plan

1. **Wave 10 (this wave)**: image + pipeline entry + run-suite flag +
   env-var gate + runbook (you are here).
2. **Wave 10a (next, not yet implemented)**:
   - Container-launch helper in `run-suite.py` that boots N nodes on
     `ZLAYER_E2E_NETWORK` and exec's `zlayer node init` then `serve`
     inside each.
   - Cluster spec `crates/zlayer-manager/tests/e2e/cluster-specs/overlay-3r-signed.yaml`:
     3-node signed-token join with overlay assertions.
   - Scenario `tests/e2e/scenarios/overlay/overlay-signed-token.test.yaml`:
     assert `tcpdump -i zlayer0 -c 1 -w /tmp/cap.pcap` inside a node
     sees the expected overlay traffic for a service-to-service ping.
3. **Wave 10b (further)**: rotation-mid-flight + revocation-mid-flight
   scenarios under the same harness.

## Why not just `kind`?

`kind` (Kubernetes In Docker) is the obvious analogue, but it
opinions away the overlay layer in favor of CNI plugins; we need to
exercise our OWN overlay (boringtun-backed WireGuard, no CNI). A
zlayer-specific privileged runner keeps the test surface honest.
