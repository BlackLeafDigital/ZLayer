# zlayer-buildd

Buildah sidecar daemon. A small Go binary that wraps
`imagebuildah.BuildDockerfiles` behind a typed gRPC service so the Rust
`BuildahSidecarBackend` (under `crates/zlayer-builder/src/backend/buildah_sidecar/`)
can drive image builds without shelling out to the `buildah` CLI.

## Build / release

### Native build

```bash
cd bin/zlayer-buildd
make build         # produces ./zlayer-buildd
```

The binary is gitignored — it belongs in release artifacts, not in the
repository.

### Cross-arch release builds

The `release` target produces stripped binaries for `linux/amd64` and
`linux/arm64`. Both require `CGO_ENABLED=1` because `go.podman.io/buildah`
links dynamically to libgpgme, libassuan, libgpg-error, and libseccomp.

```bash
cd bin/zlayer-buildd
make release       # both arches
make release-amd64 # x86_64 only
make release-arm64 # aarch64 only
```

For arm64 from an amd64 host you'll need an aarch64 cross toolchain:

| Distro          | Package                              |
| --------------- | ------------------------------------ |
| Debian / Ubuntu | `apt install gcc-aarch64-linux-gnu`  |
| Arch (CachyOS)  | `pacman -S aarch64-linux-gnu-gcc`    |
| Fedora          | `dnf install gcc-aarch64-linux-gnu`  |

### System library requirements

`zlayer-buildd` dynamically links against:

- **libgpgme** — image signing / verification
- **libassuan**, **libgpg-error** — gpgme transitive deps
- **libseccomp** — runc/crun syscall filters

Devicemapper and btrfs graphdrivers are excluded via build tags
(`exclude_graphdriver_devicemapper`, `exclude_graphdriver_btrfs`), and
the gpgme-backed OpenPGP path is replaced by the pure-Go implementation
(`containers_image_openpgp`). We only support the `overlay` and `vfs`
graphdrivers from the sidecar.

Inspect build-time pkg-config availability with:

```bash
cd bin/zlayer-buildd
make check-deps
```

Install on Debian / Ubuntu:

```bash
apt install libgpgme-dev libassuan-dev libgpg-error-dev libseccomp-dev
```

Install on Arch / CachyOS:

```bash
pacman -S gpgme libassuan libgpg-error libseccomp
```

The release binary in `release/zlayer-buildd-linux-<arch>` carries the
same dynamic dependencies. The deploy host must have these libraries
available at the corresponding `.so` versions; verify with `ldd
release/zlayer-buildd-linux-amd64`.

## CLI flags

| Flag                | Default        | Purpose                                                                 |
| ------------------- | -------------- | ----------------------------------------------------------------------- |
| `--bind`            | `127.0.0.1:0`  | TCP listen address. `:0` lets the OS pick an ephemeral port.            |
| `--tls-ca`          | (required)     | PEM CA used to verify client certs. Enforced via `RequireAndVerifyClientCert`. |
| `--tls-cert`        | (required)     | PEM server cert chain.                                                  |
| `--tls-key`         | (required)     | PEM server private key.                                                 |
| `--idle-secs`       | `30`           | Shut down after this many seconds of RPC inactivity. `0` disables idle shutdown. |
| `--max-concurrent`  | `1`            | Cap on concurrent Build RPCs.                                           |
| `--storage-driver`  | (empty)        | Override containers/storage driver (`overlay`, `vfs`, `btrfs`, ...).    |
| `--storage-root`    | (empty)        | Override containers/storage `GraphRoot`. Setting any `--storage-*` flag triggers eager store init. |
| `--storage-runroot` | (empty)        | Override containers/storage `RunRoot`.                                  |
| `--log-level`       | `info`         | `error`, `warn`, `info`, `debug`. Plumbed to a stderr JSON `slog.Logger`. |
| `--version`         | (off)          | Print a single-line JSON with `sidecar_version`, `buildah_version`, `go_version`. Exits 0. |

## Handshake

The Rust lifecycle manager spawns this binary and reads **exactly one line**
from its stdout:

```
LISTENING 127.0.0.1:43871
```

All subsequent logging is structured JSON on stderr so stdout remains
pristine after the handshake.

## TLS

mTLS is mandatory. The daemon refuses to start without `--tls-ca`,
`--tls-cert`, and `--tls-key`. `MinVersion` is TLS 1.3. Client certs are
verified against the CA pool loaded from `--tls-ca`.

For local testing you can mint an ephemeral CA + server cert with
`openssl`:

```bash
cd /tmp
openssl req -x509 -newkey ed25519 -days 1 -nodes \
  -keyout zb-ca.key -out zb-ca.crt -subj "/CN=zlayer-buildd-ca"
openssl req -newkey ed25519 -nodes \
  -keyout zb-server.key -out zb-server.csr -subj "/CN=zlayer-buildd"
openssl x509 -req -in zb-server.csr -CA zb-ca.crt -CAkey zb-ca.key \
  -CAcreateserial -days 1 -out zb-server.crt
```

These certs are **smoke-test only** — never deploy them.

## Lazy storage initialisation

`containers/storage` ordinarily wants `containers.conf` plus a writable
`GraphRoot` and `RunRoot` reachable on disk. Smoke tests, CI hosts, and
unprivileged dev boxes frequently lack one or more of those. To keep the
boot path resilient, the daemon defers `storage.GetStore` until the first
Build RPC unless the operator supplies `--storage-root`, `--storage-runroot`,
or `--storage-driver`. Passing any of those three signals "I have prepared a
self-contained store" and the daemon initialises it eagerly during boot,
failing fast if the configuration is broken.

When run without any `--storage-*` flag the boot log records:

```
deferring storage.GetStore until first Build RPC; pass --storage-root to initialise eagerly
```

This is intentional, not a warning of misconfiguration. The first Build
handler is responsible for materialising the store on demand.

## Health

The standard `grpc.health.v1.Health` service is registered alongside
`zlayer.buildd.v1.BuildService`, so `grpc_health_probe` and Kubernetes
gRPC liveness checks work out of the box. Both `""` (overall) and
`"zlayer.buildd.v1.BuildService"` report `SERVING` once the daemon is
ready.

## Shutdown

Three triggers, one drain path:

1. **SIGINT / SIGTERM** → `GracefulStop`, with a 10-second hard-stop fallback
   via `grpcServer.Stop()` if drain takes longer.
2. **Idle timeout** → same drain path, logged with the observed gap.
3. **`Serve` returns** → process exits 0 unless the error is something other
   than `grpc.ErrServerStopped`.

## Smoke test

```bash
timeout 20 ./zlayer-buildd \
  --bind 127.0.0.1:0 \
  --tls-ca /tmp/zb-ca.crt \
  --tls-cert /tmp/zb-server.crt \
  --tls-key /tmp/zb-server.key \
  --idle-secs 1
```

Expected: first stdout line is `LISTENING 127.0.0.1:<port>`, structured
stderr logs follow, and the daemon exits 0 within ~6 seconds (5-second
idle-loop tick + 1-second idle window).

## Versioning

`--version` emits a single JSON line:

```json
{"buildah_version":"v1.44.0","go_version":"go1.26.3","sidecar_version":"(devel)"}
```

`sidecar_version` is overridable at link time via
`-ldflags "-X main.sidecarVersion=v0.1.0"`. Without the override it falls
back to the Go module's `BuildInfo.Main.Version`, which is `(devel)` for
plain `go build` invocations.
