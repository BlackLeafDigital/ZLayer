# Changelog

All notable changes to this project will be documented in this file.

## [0.50.1] - 2026-05-15

### Fixed
- `zlayer node join` now accepts the Wave-3+ signed envelope (`SignedClusterJoinToken` v=1 and v=2) that `zlayer node generate-join-token` has been emitting by default. Previously `parse_cluster_join_token` only knew the legacy plaintext flat-JSON shape and bailed with `Invalid join token: not valid JSON: missing field 'api_endpoint'` when handed a freshly-minted signed token — the CLI was rejecting its own output. The parser now tries the signed envelope first (lifting `api_endpoint`/`raft_endpoint`/`leader_wg_pubkey`/`overlay_cidr` out of `claims`, and using `claims.iat` as the legacy `created_at` field) and falls back to the legacy shape on parse failure. Signature verification stays on the leader — the joiner does not hold the verifying key. Regression-tested by `test_parse_signed_envelope_join_token`. Unblocks the `cluster_3node` / `cluster_failover` / `cluster_scaling` / `cluster_upgrade` / `cluster_node_upgrade` e2e suites.
- E2E `login.test.yaml` final `waitForSelector` now names the specific Dashboard heading (`{ role: heading, name: "Dashboard" }`) instead of the ambiguous `{ role: heading }` that violated Playwright strict mode (Dashboard renders 3 `<h*>` tags).
- `raft-e2e-tests` workflow step in `.forgejo/workflows/e2e.yml` no longer silently swallows per-suite failures. The cluster-suite loop previously used `if ! cmd; then rc=$?; …` which always captured `$?` as `0` (the negated test) so `worst` never accumulated a non-zero exit code; rewritten as `if cmd; then …; else rc=$?; worst=$rc; fi` and explicit `set -eo pipefail` at the top of the script.

### Changed
- Every Linux job in `.forgejo/workflows/e2e.yml` (`e2e-tests`, `wasm-e2e-tests`, `docker-e2e-tests`, `manager-tests`, `manager-intellitester-tests`, `raft-e2e-tests`, `scheduler-unit-tests`) now runs inside a privileged `ubuntu:24.04` container with `--privileged --cap-add=NET_ADMIN --device=/dev/net/tun`. The throwaway daemon's overlay/TUN init no longer degrades to `Global overlay failed (cross-node networking disabled)`. `docker-e2e-tests` additionally mounts `/var/run/docker.sock` and installs `docker.io`. The `macos-sandbox-e2e` and `dispatch-build` jobs are unchanged. Sudo prefixes were dropped from steps that run as root in the container, and the now-obsolete "Fix permissions after sudo test" step in `e2e-tests` was removed. Per-test-node container mode (`run-suite.py --overlay-mode container`) remains scaffolded — see `docs/operating/e2e-privileged.md`.

## [0.50.0] - 2026-05-15

### Fixed
- `WipeJoinSecret` Raft op now triggers a local filesystem delete of `{data_dir}/join_secret` on every node when it applies (resolves the Known Limitation called out in v0.16.0). The Raft apply wrapper in `zlayer_scheduler::raft::ClusterState` fires `zlayer_secrets::NodeSideEffects::fire_wipe_join_secret` on a successful apply; the daemon spawns a watcher in `zlayer serve` that drains the notify and runs the wipe (sub-second on the leader, propagates to followers as the op replicates). A boot-time reconcile in `zlayer serve` additionally consults `SecretsState::join_secret_wiped_at` at startup and forces the delete if `Some(_)`, covering the snapshot-restore path on followers that joined after the original apply. End-to-end idempotent; replaying the op preserves the first-applied timestamp for audit. `zlayer serve --vacuum-secrets` remains as an out-of-band escape hatch for operator-driven cleanup.

### Added
- `zlayer_secrets::NodeSideEffects` — first cross-cutting channel for Raft ops that need per-node filesystem/network effects. The handle is plumbed through the new `RaftCoordinator::with_auth_secrets_and_effects` constructor; existing constructors (`new`, `with_auth`, `with_auth_and_secrets`) delegate with `None` for backwards compatibility. Future ops needing similar behavior should add a matching `tokio::sync::Notify` field plus `fire_…` / `wait_…` accessors here rather than fanning out into per-op types.

## [0.16.0] - 2026-05-14

### Added
- HS256 → EdDSA-JWT migration path. New cluster-wide `jwt_algorithm` policy (`hs256` | `both` | `eddsa`) replicated via Raft, defaulting to `both` for safe in-place migration. EdDSA-JWT validator joins the existing HS256 validator and the Wave-3 signed-envelope validator in `cluster_join`'s dispatch, gated by the active policy. Operators run `zlayer cluster migrate-jwt-to-eddsa` to flip to `both`, then `zlayer cluster decommission-hs256 [--vacuum-secret]` to flip to `eddsa`-only (and optionally schedule a cluster-wide wipe of `{data_dir}/join_secret`). New endpoints `POST/GET /api/v1/cluster/{jwt-algorithm,jwt-status,wipe-join-secret}` (admin auth) + matching CLI commands + `zlayer cluster jwt-status`.

### Known limitations
- The `WipeJoinSecret` Raft op records the wipe instant in the state machine but does NOT yet trigger a local filesystem delete on each node — operators can still use `zlayer serve --vacuum-secrets` (Wave 6) for the actual file cleanup. Wiring the filesystem effect to the apply path is a follow-up.

## [0.15.0] - 2026-05-14

### Added
- Privileged-container e2e runner scaffolding. New image `images/ZImagefile.zlayer-e2e-node` (registered in `ZPipeline.yaml`) for tests that exercise the real boringtun overlay. `run-suite.py --overlay-mode container` is the entry-point flag (gated by `ZLAYER_E2E_PRIVILEGED=1`); the container-launch driver itself is scoped for a follow-up iteration — today the flag exits cleanly with an actionable message. See `docs/operating/e2e-privileged.md` for the operator runbook.
- Cluster CA + SPIFFE-style federation (Wave 9). Each cluster generates a long-lived `cluster_ca.key` at first daemon start (never rotated). Tokens may be minted as `v=2` carrying a `ca_chain` (`CaCert`) signed by the cluster CA, binding the per-rotation `kid` to a `cluster_domain`. Operators export this cluster's `TrustBundle` (CA pubkey + domain) via `GET /api/v1/cluster/trust-bundle` (unauthed by design), transport it out-of-band, and `zlayer cluster trust-bundle import <FILE-OR-URL>` it into a peer cluster. Imports replicate via Raft (`SecretsRaftOp::ImportTrustBundle` / `RemoveTrustBundle`). Validators now accept v=2 tokens whose `kid` is unknown locally if `ca_chain.cluster_domain` is in the imported trusted-bundles set and the CaCert verifies under that bundle's CA pubkey. New endpoints: `POST /api/v1/cluster/trust-imports`, `GET /api/v1/cluster/trust-bundles`, `DELETE /api/v1/cluster/trust-imports/{cluster_domain}` (all admin auth). New CLI: `zlayer cluster trust-bundle {export, import, list, remove}`. HTTPS-only fetch when importing from URLs.

## [0.14.0] - 2026-05-14

### Added
- Cluster-wide token revocation list (Wave 7). New `POST /api/v1/cluster/revoke-token` and `GET /api/v1/cluster/revocations` endpoints (both admin auth) plus matching CLI commands `zlayer cluster revoke-token <token-or-hash> [--reason ...]` and `zlayer cluster list-revocations`. Revocations replicate through Raft via the new `SecretsRaftOp::RevokeToken`; entries auto-prune at apply time so the table stays bounded by the un-expired token horizon. Token hashes (lowercase hex SHA-256 of the b64 envelope) are the replicated identifier — raw token bytes never enter cluster state. Applies to both Ed25519-signed and HS256-JWT tokens — the check runs in `cluster_join` before format-specific validators.
- `SigningBackend` trait + `FileBackend` adapter in `zlayer-secrets` (Wave 8, minimal). Abstracts the cluster signing keystore behind a trait so future TPM/YubiHSM/KMS impls can swap in without touching call sites. Current shipping implementation is the existing JSON keystore on disk (`FileBackend`); the trait reports `is_hardware_backed() = false`. Hardware-backed impls (TPM 2.0, YubiHSM 2) are deliberately deferred — the trait is the extension point only.

## [0.13.0] - 2026-05-14

### Removed
- Plaintext cluster join token format. Servers no longer accept tokens with `auth_secret` baked into the body; CLI no longer emits them. Re-issue any plaintext tokens by running `zlayer node generate-join-token`. Ed25519-signed and HS256 formats are unaffected.
- `--legacy-plaintext` CLI flag on `zlayer node generate-join-token` (was an emergency rollback escape hatch in v0.12.0).

### Added
- `--vacuum-secrets` flag on `zlayer serve`. When set at startup, wipes `{data_dir}/join_secret` so the HS256 HMAC key regenerates. Use after suspected symmetric-secret leak. Default off.

### Migration
- Before upgrading: have all clients re-issue join tokens using `zlayer node generate-join-token` from a v0.12.x daemon. The output's signed and HS256 tokens both validate against v0.13.0 servers. Plaintext tokens issued under v0.11.x or earlier MUST be re-issued.

## [0.12.0] - 2026-05-14

### Added
- Begin signed-join-token work — added `ed25519-dalek = "2"` to workspace deps (no behavior change yet; Wave 1 will introduce the signing keypair). See docs/adr/0001-signed-join-tokens.md.
- `zlayer node generate-join-token --ttl <DURATION>` (default `24h`,
  humantime syntax accepted). Drives the new Wave-3 signed cluster join
  token's `exp` claim (`now() + ttl`). The handler now emits a signed
  token (Ed25519, recommended) alongside the existing legacy plaintext
  token (deprecated; removed in Wave 6). Legacy token unchanged. The
  signed-token issuer (`iss`) is the local node UUID from
  `node_config.json`; the signer is loaded from
  `{data_dir}/cluster_signing.key` (same path as the daemon).
- Ed25519-signed cluster join tokens (Wave 3). New `zlayer node generate-join-token --ttl <duration>` mints a signed token with explicit expiration; default ttl is 24h. The signed format coexists with the existing HS256-JWT and plaintext formats; servers try Ed25519 → HS256 → plaintext.
- TLS for the daemon API listener (Wave 2). New `--api-tls-cert`/`--api-tls-key` flags load static PEM certs; `--api-tls-acme` delegates to the proxy's ACME-capable `CertManager`. Default off.
- `GET /api/v1/cluster/signing-pubkey` endpoint (Wave 1, unauthenticated by design). Returns the cluster's active Ed25519 verifying key + kid for joining nodes to fetch.

### Changed
- `zlayer node generate-join-token` no longer emits the legacy plaintext token by default (Wave 4). Pass `--legacy-plaintext` if you specifically need it for backward compat with un-upgraded peers; the flag is deprecated and removed in v0.13.0.

### Deprecated
- Plaintext join tokens. Servers still accept them but now emit a warning both server-side (`tracing::warn`) and in the API response body. Removal scheduled for v0.13.0 (Wave 6).

## [0.11.24] - 2026-05-12

### Added
- `cluster_node_upgrade` e2e suite in
  `crates/zlayer-manager/tests/e2e/scripts/run-suite.py`. Boots a real
  3-node loopback cluster, posts `/api/v1/cluster/upgrade` with a
  nonexistent target version, and verifies: (a) a follower-targeted
  POST returns 421 + `X-Leader-Addr`, (b) the leader orchestrator
  walks every follower and records an error for each, (c) the
  cluster survives the failed upgrade with all 3 nodes still ready.
  Surfaced via the new `raft-e2e:cluster_node_upgrade` dropdown
  option on both `.forgejo/workflows/e2e.yml` and
  `.github/workflows/e2e.yml`.
- `zlayer node upgrade` — leader-driven rolling daemon-binary upgrade
  across the cluster. The CLI POSTs `/api/v1/cluster/upgrade` to the
  local daemon; if it isn't the leader the daemon returns
  `421 Misdirected Request` with an `X-Leader-Addr` response header
  and the CLI re-targets the leader automatically. The leader
  iterates followers in stable `node_id` order: it POSTs
  `/api/v1/internal/upgrade/start` against each follower (using the
  internal-cluster `X-ZLayer-Internal-Token` header), waits for
  `/health/ready` to flap (drop, come back), then sleeps
  `--cooldown-secs` (default 30s) before the next node. Followers
  shell out to `zlayer self-update --yes [--version <v>]` from the
  endpoint handler, write a `{data_dir}/run/zlayer.restart` sentinel,
  and exit code 75 (EX_TEMPFAIL) so a supervisor respawns them on the
  new binary. After every follower has come back healthy, the CLI
  POSTs `/api/v1/cluster/upgrade-self` on the leader to schedule its
  own self-upgrade and polls `/health/ready` to confirm the leader
  came back on the new binary. Flags: `--version vX.Y.Z`,
  `--cooldown-secs N`, `--strict` (abort on the first follower
  failure), `--skip-leader` (don't auto-upgrade the leader — useful
  when timing a deliberate failover), `-y/--yes`.
- `POST /api/v1/cluster/upgrade-self` endpoint (admin auth). Used by
  `zlayer node upgrade` after followers report healthy to schedule
  the leader's own self-upgrade. Two-step orchestration: (1) the
  handler picks a healthy follower (`status == "ready"`, fresh
  heartbeat) and POSTs `/api/v1/internal/raft/trigger-elect` to it
  so that follower campaigns immediately via `Raft::trigger().elect()`
  instead of the cluster waiting for heartbeat-loss after the leader
  drops; we poll `raft.metrics().current_leader` for up to 5s to
  confirm leadership flipped, then (2) re-enter
  `internal_upgrade_start` over a loopback request with the configured
  `X-ZLayer-Internal-Token` so authn/authz semantics stay identical
  to the follower path. If the trigger-elect handoff fails or times
  out we still proceed — the worst case is one heartbeat-loss window
  of leaderlessness, which is what the previous behavior had anyway.
- `POST /api/v1/internal/raft/trigger-elect` (internal-token auth).
  Calls `RaftCoordinator::trigger_elect()` which wraps
  `Raft::trigger().elect()` from openraft 0.9. Only available on
  clustered daemons; returns 503 otherwise. Plumbing: `InternalState`
  gained an `Option<Arc<RaftCoordinator>>` field wired in
  `bin/zlayer/src/commands/serve.rs` when `_raft` is `Some`.
- Supervisor-respawn coverage matrix for `zlayer serve
  --restart-on-exit` (exit code 75) — the path used by self-update
  and `zlayer node upgrade`:
  - **Linux systemd**: unit generated by `zlayer daemon install`
    gains `SuccessExitStatus=75` and `RestartForceExitStatus=75`.
    Belt-and-suspenders: even if the operator switches to
    `Restart=on-failure`, exit 75 still triggers a restart instead
    of being treated as a clean exit. Existing `Restart=always`
    semantics unchanged.
  - **macOS launchd**: `KeepAlive=true` already respawns on any
    exit. `zlayer daemon install` and `zlayer self-update --restart`
    additionally fire `launchctl kickstart -k <label>` (resolved via
    the same `system/...` vs `gui/<uid>/...` scope logic the install
    path uses) so the running daemon is forcibly replaced with the
    freshly-installed binary instead of racing on whatever
    `ThrottleInterval` happens to be configured.
  - **Windows SCM**: `zlayer daemon install` now follows
    `CreateServiceW` with `sc.exe failure <name> reset= 86400
    actions= restart/5000/restart/5000/restart/5000` so SCM respawns
    the daemon up to three times after non-zero exit. Previously SCM
    ignored the exit and left the service stopped.
- `ApiError::NotLeader { leader_addr: Option<String> }` variant
  mapping to HTTP `421 Misdirected Request`. Includes a
  `LEADER_ADDR_HEADER` constant (`X-Leader-Addr`) so the response
  carries the leader's API endpoint when known. First use is the new
  `cluster_upgrade` handler; eventually every leader-only endpoint
  should return this instead of generic 503s.
- `POST /api/v1/cluster/upgrade` and `POST /api/v1/internal/upgrade/start`
  + `GET /api/v1/internal/upgrade/{id}` endpoints. Internal endpoints
  reuse the existing `InternalAuth` extractor + `X-ZLayer-Internal-Token`
  header pattern. Status polling follows the 202-Accepted + UUID-id
  convention from `start_build`.
- `zlayer serve --restart-on-exit` (also `ZLAYER_RESTART_ON_EXIT`)
  flag. After clean shutdown, exits with code 75 (`EX_TEMPFAIL`)
  instead of 0 so a supervisor (`systemd Restart=on-failure`, runit,
  etc.) respawns the daemon. Used by `zlayer node upgrade`'s
  follower-self-update path.
- `zlayer self-update` subcommand. Downloads a newer zlayer release
  tarball from GitHub (`BlackLeafDigital/ZLayer`), optionally verifies
  a SHA-256 sidecar when present (skips silently when absent so
  current releases keep working), extracts the binary, and
  atomic-renames it onto the running executable. Linux/macOS support
  the in-place rename because the kernel keeps the old text segment
  alive via the open fd. Windows falls back to writing
  `zlayer.exe.new` adjacent to the current exe with a clear restart
  message. Flags: `--version vX.Y.Z` to target a specific tag,
  `--yes` to skip the prompt, `--restart` to re-exec the new binary
  after install, `--repo owner/name` (hidden) for test/mirror
  overrides.
- `cluster_scaling` and `cluster_upgrade` suites in
  `crates/zlayer-manager/tests/e2e/scripts/run-suite.py`. Both suites
  stand up a real 3-node loopback cluster via
  `_bootstrap_3node_cluster` and exercise
  `zlayer deploy`/`zlayer ps --containers --format json` against the
  leader. `cluster_scaling` deploys a single nginx replica, scales to
  3 (asserting replicas land on at least 2 distinct nodes), then
  scales back to 1. `cluster_upgrade` deploys
  `nginx:1.28-alpine` × 3, redeploys with `nginx:1.29-alpine`, and
  asserts every running replica's image field transitions to the v2
  tag within 180s.
- Three deployment fixtures under
  `crates/zlayer-manager/tests/e2e/cluster-specs/`
  (`nginx-v1-1r.yaml`, `nginx-v1-3r.yaml`, `nginx-v2-3r.yaml`) used
  by the two new suites. All three share `deployment: e2e-cluster-app`
  so re-applying triggers in-place updates rather than fresh
  deployments.
- `raft-e2e` / `raft-e2e:<suite>` and `scheduler-unit` dropdown
  options on both `.forgejo/workflows/e2e.yml` and
  `.github/workflows/e2e.yml`. New `raft-e2e-tests` job dispatches
  the four cluster suites (`cluster_3node`, `cluster_failover`,
  `cluster_scaling`, `cluster_upgrade`) via the existing
  `run-suite.py` harness. New `scheduler-unit-tests` job runs
  `cargo nextest run -p zlayer-scheduler --features test-skip-http`
  — gives GitHub parity with Forgejo's `ci.yaml` raft coverage.
- Raft RPC transport upgraded to HTTP/2 prior-knowledge (h2c). Both
  `RaftHttpClient` reqwest clients now call `.http2_prior_knowledge()`
  with a 10s keepalive ping interval, and the server-side bind in
  `crates/zlayer-scheduler/src/raft_service.rs` switched from
  `axum::serve` to `hyper-util`'s `auto::Builder` so the same socket
  accepts both HTTP/1.1 and HTTP/2. Multiplexed streams replace the
  per-RPC connection churn that the previous HTTP/1.1
  `pool_max_idle_per_host(10)` ceiling caused.
- `X-ZLayer-Raft-Protocol: 1` header on every Raft RPC, validated by a
  new middleware in
  `crates/zlayer-consensus/src/network/http_service.rs`. Mismatched
  versions return `426 Upgrade Required` with an
  `X-ZLayer-Raft-Protocol-Supported` response header. A missing header
  forwards unchanged so peers that don't yet emit it stay compatible.
- `DELETE /api/v1/cluster/nodes/{id}`, `PUT /api/v1/cluster/nodes/{id}/mode`,
  `PUT /api/v1/cluster/nodes/{id}/drain`, and
  `PUT /api/v1/cluster/nodes/{id}/undrain` endpoints. Previously the
  CLI commands `zlayer node remove`, `zlayer node set-mode`, and
  `zlayer node drain` 404'd silently; they now reach real handlers
  that propose the corresponding Raft state changes.
- `Request::UpdateNodeMode { node_id, mode }` Raft state-machine
  variant. Appended last to preserve postcard2 discriminant stability
  for the pre-existing variants.
- `--api-port`, `--raft-port`, and `--overlay-port` flags on
  `zlayer node join` for parity with `zlayer node init`. Lets multiple
  nodes coexist on the same machine, enabling loopback clustering for
  tests and dev.
- In-process multi-node Raft test harness under
  `crates/zlayer-scheduler/tests/` with `common/mod.rs` exposing
  `spawn_node`, `wait_for_leader`, `wait_for_apply`, and
  `shutdown_node`. New integration tests: `single_node_bootstrap`,
  `two_node_cluster` (validates the 1-voter + 1-learner 2-node
  guarantee from the README), `three_node_replication` (replicates
  100 entries plus installs a snapshot to a lagging follower),
  `leader_failover` (kills the leader and asserts a new one wins
  election), and `allocation_modes` (validates the shared / dedicated
  / exclusive node-allocation modes from README "Node Allocation
  Modes").
- `cluster_3node` and `cluster_failover` suites in
  `crates/zlayer-manager/tests/e2e/scripts/run-suite.py`. They spawn
  three real `zlayer serve` processes on loopback, validate the
  cluster forms, and confirm the leader recovers from a worker
  kill+restart.
- `docs/rolling-upgrade.md` operator runbook covering drain → swap
  binary → restart, the protocol-version handshake's role in
  preventing cross-version corruption, and rollback semantics.

### Changed
- `RedbStateMachine::apply`, `RedbStateMachine::install_snapshot`,
  `RedbLogStore::save_vote`, `save_committed`, `append`, `truncate`,
  and `purge` now run their synchronous redb transactions inside
  `tokio::task::spawn_blocking` so fsync no longer pins a tokio worker
  thread. In-memory state-machine cache updates stay in the async
  context and only run after the redb commit succeeds.
- `RaftHttpClient` precomputes per-connection URL strings as
  `Arc<str>` and the `Authorization` header as a
  `reqwest::header::HeaderValue` (built once, marked sensitive).
  Eliminates two `String` allocations and one header-value rebuild
  per Raft RPC.
- `zlayer node list` now displays the server-computed `role` field
  (`Leader` / `Voter` / `Worker`) instead of deriving the column from
  the local `node_config.is_leader` bool. Status is shown next to the
  role (e.g. `Worker (draining)` or `Worker [DEAD]`).
- Lowered `pool_max_idle_per_host` from 10/5 (RPC/snapshot) to 2/2.
  HTTP/2 multiplexes, so idle TCP connections aren't useful for raft.

### Fixed
- `ZLayerDirs::default_data_dir` now honors `$ZLAYER_DATA_DIR` as the
  highest-precedence source. Previously the env var was only read by
  `zlayer serve`'s `--data-dir` clap arg, so CLI subcommands like
  `zlayer user create` that dispatch through `DaemonClient::connect()`
  without threading `cli.effective_data_dir()` silently dialed
  `$HOME/.zlayer/run/zlayer.sock` instead of the throwaway daemon's
  socket. Surfaced when the manager-intellitester e2e harness ran
  `user create` against a `--data-dir`-scoped daemon in CI.
- Manager e2e harness (`crates/zlayer-manager/tests/e2e/scripts/run-suite.py`)
  now echoes captured stdout/stderr to `sys.stderr` when a
  `capture_output=True, check=True` subprocess fails. Previously a CLI
  failure showed only `CalledProcessError: returned non-zero exit
  status 1` in the CI log with no underlying message.
- Manager e2e harness polls `/health/ready` (the actual handler in
  `crates/zlayer-api/src/handlers/health.rs`) instead of the
  nonexistent `/healthz`. The `/` fallback in `_wait_http_ok` was
  masking the misnamed endpoint.
- Dead `peers: RwLock<HashMap<NodeId, String>>` field and its three
  unused accessor methods removed from `HttpNetwork` -- the field was
  never read by any caller in the workspace.

### Internal
- `crates/zlayer-scheduler/src/lib.rs::build_node_states` extracted to
  a free helper `cluster_nodes_to_node_states` so the
  `status == "ready"` filter behind README "Node Status" is now
  guarded by a unit test (`build_node_states_excludes_non_ready`).

## [0.11.23] - 2026-05-12

### Changed
- Scratch storage (Docker build contexts, OCI image tarballs, S3 layer staging,
  SQLite replicator caches, git clones, in-memory store fixtures) now lives
  under `{data_dir}/tmp` instead of the OS temp dir. Avoids putting large
  scratch data on `tmpfs` (RAM-backed `/tmp` on most Linux distros), preventing
  OOM on memory-tight nodes during big builds or layer uploads. New types
  `zlayer_types::Scratch` and `zlayer_types::ScratchFile` (RAII guards), plus
  helpers `ZLayerDirs::scratch_dir(prefix)` / `ZLayerDirs::scratch_file(prefix)`
  in `zlayer-paths`. Project git clones moved from `${TMPDIR}/zlayer-projects`
  to `{data_dir}/projects` (new `ZLayerDirs::projects()` accessor), so they
  survive daemon restarts. Shell scripts (`build-macos-images.sh`,
  `run_dev.sh`, `Makefile`, `docker-compat-local.sh`) now anchor scratch dirs
  at `${ZLAYER_DATA_DIR:-$HOME/.zlayer}/tmp/...` for the same reason.

## [0.11.22] - 2026-05-11

### Added
- `zlayer serve --wg-port <port>` (also `ZLAYER_WG_PORT`) and
  `zlayer serve --dns-port <port>` (also `ZLAYER_DNS_PORT`) let an
  operator bind the WireGuard overlay UDP port and the overlay DNS
  server on caller-chosen ports. Defaults remain
  `zlayer_core::DEFAULT_WG_PORT` (51420) and
  `zlayer_overlay::DEFAULT_DNS_PORT` (15353). Precedence for `wg-port`
  is `CLI flag > node_config.json#overlay_port > constant default`; for
  `dns-port` it is `CLI flag > constant default`. Pair with
  `--deployment-name` to run a hermetic test daemon alongside a system
  install without UDP/DNS port collisions. The launchd installer
  forwards both overrides into the generated plist.
- `zlayer serve --deployment-name <name>` (also `ZLAYER_DEPLOYMENT_NAME`)
  scopes overlay network interfaces, the WireGuard UAPI socket name, and
  per-deployment runtime state to a caller-chosen prefix. The default
  remains `"zlayer"` (so `zl-zlayer-*` link names and stock socket paths
  are unchanged), but a second daemon can now run alongside a system
  install by passing `--deployment-name zlayer-e2e`, which produces
  `zl-zlayer-e2e-g` instead of colliding on `zl-zlayer-g`. The launchd
  install path forwards the override into the generated plist.
- End-to-end test harness for the `zlayer-manager` UI under
  `crates/zlayer-manager/tests/e2e/`, driven by Intellitester +
  Playwright. Includes a bootstrap → logout → login workflow, a
  navigation smoke pass across every protected route, and a
  stale-session regression covering the ghost-session fix below. The
  stale-session test bootstraps an admin, persists the resulting
  Playwright storage state, wipes the user row from `users.db` via
  `sqlite3`, then loads the cookies back with intellitester's
  `--storage-state` flag and asserts (network layer via
  `expectResponse`, browser-jar layer via `assertCookies`) that
  `manager_me` cleared both `zlayer_session` and `zlayer_csrf`. The
  hermetic runner (`scripts/run-suite.sh`) spins up a daemon and the
  manager against a throwaway data directory, runs the suite, and
  tears everything down on exit. Browsers and node deps are not
  bundled — see `tests/e2e/README.md` for the one-time install step.
  Requires `intellitester@^0.4.5`.
- `manager-intellitester-tests` job added to both
  `.github/workflows/e2e.yml` and `.forgejo/workflows/e2e.yml`. The job
  installs cargo-leptos + Playwright Chromium, then drives the
  hermetic runner under `sudo -E` (the daemon needs CAP_NET_ADMIN for
  the overlay manager). It is wired into the release-dispatch gate
  (`trigger-release` on GitHub, `dispatch-build` on Forgejo) so a
  failing browser test blocks tag creation just like the existing
  Youki / WASM / Docker / macOS suites. A new `test`
  `workflow_dispatch` choice input lets an operator run a single
  intellitester file (`bootstrap.test.yaml`, `nav-smoke.test.yaml`,
  `auth.workflow.yaml`, etc.) without running the full suite — useful
  for fast iteration on a single regression. The Forgejo job uses the
  internal `setup-rust@main`, `s3-cache@main`, and `ci-step-status@main`
  actions to match the existing e2e jobs, including S3-backed caching
  of the Playwright browser tarball.
- `crates/zlayer-manager/tests/e2e/scripts/run-suite.sh` now accepts
  `--only <test.yaml>` (run a single intellitester file and skip the
  stale-session sqlite step) and `--no-build` (skip `cargo build` /
  `cargo leptos build` when `target/release` is already current). The
  test daemon is now launched with `--deployment-name zlayer-e2e
  --wg-port 51421 --dns-port 15354` so it can run alongside a stock
  zlayer install without colliding on `zl-zlayer-*` interface names or
  the default UDP+DNS ports. All four ports
  (`ZLAYER_E2E_API_PORT`, `ZLAYER_E2E_MANAGER_PORT`,
  `ZLAYER_E2E_WG_PORT`, `ZLAYER_E2E_DNS_PORT`) are env-overridable.

### Changed
- The manager e2e harness (`crates/zlayer-manager/tests/e2e/scripts/
  run-suite.sh`) now defaults to running against the user's locally-
  installed zlayer daemon: it resolves the daemon's Unix socket
  (`/var/run/zlayer.sock` or `~/.zlayer/run/zlayer.sock`), creates a
  scoped fixture admin via `zlayer user create` (using the auto-
  injected `local-admin` bearer that the daemon mints for any UDS
  connection), drives the login UI through intellitester, and deletes
  the fixture user on exit. The stale-session regression replaces its
  `sqlite3 DELETE FROM users` step with `zlayer user delete <id>
  --yes` and now asserts a redirect to `/login` (other users still
  exist in the host daemon's user table). A new `--throwaway` flag
  (also `ZLAYER_E2E_THROWAWAY=1`) keeps the previous behaviour of
  spinning up an isolated daemon for clean CI runners. Removes the
  `sqlite3` PATH dependency from the harness. The obsolete
  `bootstrap.test.yaml`, `logout.test.yaml`,  `auth.workflow.yaml`,
  and `scripts/reset-data-dir.sh` files are deleted — the suite now
  exercises the login flow (which is the bug regression we actually
  care about) and signup is exercised every time someone installs
  zlayer.
- The `BlackLeafDigital/ZLayer` composite action (`action.yml`) is now
  CI-safe and supports both portable and system installs. Three new
  inputs: `install-dir` (empty = portable temp install to
  `$RUNNER_TEMP/zlayer-bin`; non-empty = system install via `install.sh`
  with per-OS dep setup — libseccomp, cgroups v2 check, SELinux relabel,
  PATH wiring, shell completions); `data-dir` (defaults to
  `$RUNNER_TEMP/zlayer-data` in portable mode, unset in system mode);
  and `install-daemon` (defaults `false`, sets `ZLAYER_NO_SERVICE=1`
  when invoking `install.sh`). `ZLAYER_DATA_DIR` is now propagated via
  `$GITHUB_ENV` so every subsequent step in the job inherits the
  per-runner sandbox automatically. A new `data-dir` output mirrors
  the resolved value for callers that want to read it explicitly. The
  `Run ZLayer command` sub-step's broken YAML-level
  `if: inputs.command != ''` (which Forgejo's `act_runner` ignores on
  composite sub-steps) has been replaced with a bash-internal guard, so
  empty-command invocations install-only instead of hanging on the
  implicit TUI.
- The manager e2e harness has been rewritten from bash to Python
  (`run-suite.py`, stdlib only, invoked via `uv run`). The bash version
  (`run-suite.sh`) is deleted in the same commit. Same flags
  (`--throwaway`, `--only`, `--no-build`), same yamls, same login-based
  flow — none of the bash-shaped failure modes (subshell-scope loss of
  `DAEMON_PID`, env-prefix parameter expansion, `trap`-on-`$()` cleanup
  drops, tmpfs orphans, CLI silently hitting the wrong daemon). The
  throwaway daemon's data dir moved from `mktemp /tmp/zlayer-e2e-*` to
  `target/zlayer-e2e/<suite-id>/` (gitignored, swept by `cargo clean`,
  user-owned). A new `--sudo-daemon` flag is available as an escape
  hatch in case the daemon's overlay init ever starts demanding
  CAP_NET_ADMIN at startup — today the overlay manager logs a
  permission warning and degrades to "cross-node networking disabled",
  which is harmless for the auth/manager flow the suite exercises, so
  no part of the local or CI invocation runs under sudo. CI workflows
  (`.github/workflows/e2e.yml` and `.forgejo/workflows/e2e.yml`)
  install uv via `astral-sh/setup-uv@v6` and invoke
  `uv run …/run-suite.py --throwaway`.
- The `test` `workflow_dispatch` dropdown in both e2e workflows now
  gates every e2e job, not just the intellitester sub-suite. Options
  are `youki`, `wasm`, `docker`, `manager-unit`,
  `manager-intellitester`, `manager-intellitester:<yaml>`, and
  `macos-sandbox` (plus `macos-unit` on GitHub). Empty value runs all
  jobs as before. The release dispatch gate
  (`dispatch-build` on Forgejo, `trigger-release` on GitHub) now
  additionally requires `inputs.test == ''` so a partial e2e run can
  never short-circuit into a release with the other suites skipped.
  Dead `bootstrap.test.yaml`, `logout.test.yaml`, and
  `auth.workflow.yaml` dropdown options (the underlying files were
  deleted earlier in 0.11.22) are removed.

### Fixed
- The daemon's `AuthActor` extractor now prefers a session cookie
  over a Bearer token when both are present on the same request.
  Previously the Bearer always won, which meant any user-facing call
  the manager proxied to the daemon **over the Unix socket** would
  be auth'd as the auto-injected `local-admin` instead of the real
  logged-in user. `/auth/me` would then look up `local-admin` in
  the user store, find nothing, and return `401 "User no longer
  exists"` — so the manager's `AuthGuard` bounced every logged-in
  request back to `/login`. The bug was latent in TCP-transported
  manager → daemon setups (the auto-bearer middleware is
  Unix-socket-only) but blocked any deployment that pointed
  `ZLAYER_SOCKET` at the daemon — including the e2e harness. CLI /
  service auth (Bearer-only, no cookie) is unchanged: cookie
  extraction fails, the extractor falls through to Bearer, and
  `local-admin` continues to drive the request. Mirrors to
  zlayer-zql.
- `DELETE /api/v1/users/{id}` is now properly idempotent: deleting an
  id that no longer exists returns `204 No Content` instead of
  `404 Not Found`, matching REST DELETE semantics. The underlying
  `IdentityManager::delete_user` signature changed from
  `Result<StoredUser, IdentityError>` to
  `Result<Option<StoredUser>, IdentityError>` (`Ok(None)` = already
  gone, `Ok(Some(u))` = deleted). Removes the need for `zlayer user
  delete <id> --yes || true` wrappers in cleanup scripts and trap
  handlers.
- `zlayer` (no subcommand) and `zlayer tui` now refuse to launch the
  Ratatui TUI when stdin or stdout is not a terminal, exiting 1 with a
  clear error pointing at `zlayer --help`. Previously a non-TTY caller
  (CI, piped shells, `</dev/null`) would block forever on crossterm
  trying to read stdin events — the failure mode that hung the
  ZArcRunner release workflow for 2h+ until the step timeout fired.
  `--version`, `--help`, and every explicit subcommand are unaffected
  (clap short-circuits the flags before the guard, and the guard only
  fires on the implicit/explicit TUI dispatch paths).
- Fresh `--data-dir` boots no longer crash with
  `Failed to create node key directory {data_dir}/secrets: File exists
  (os error 17)`. `PersistentSecretsStore::open` previously branched on
  `path.is_dir()`, and when the data dir did not yet exist the branch
  fell through to treat `{data_dir}/secrets` as a *file* path, after
  which SQLite created a regular file at that location. The subsequent
  `fs::create_dir_all({data_dir}/secrets)` call in
  `load_or_generate_node_keypair` then tripped EEXIST against the file.
  `open` now `mkdir -p`s the path up front when it doesn't exist and the
  extension doesn't look like a `SQLite` filename, so the directory
  branch always wins on fresh installs. The legacy upgrade path in
  `bin/zlayer/src/migrations.rs` also swaps `create_dir` for
  `create_dir_all` defensively so a partially-completed migration is
  idempotent on the next boot.
- The `zlayer serve` boot sweep no longer indiscriminately deletes
  network interfaces, WireGuard UAPI sockets, or `utun` devices that
  belong to another live daemon. The Linux/macOS link sweep now only
  matches names starting with `zl-<deployment-name>-` (derived from the
  new `--deployment-name` flag), and the WireGuard socket sweep only
  removes `*.sock` files whose stem starts with that same prefix. On top
  of that, if a `daemon.json` from another data directory (the system
  `/var/lib/zlayer` location or the user `~/.zlayer` location) names a
  PID that is still alive, the entire sweep is skipped with a warning —
  two daemons sharing the default `"zlayer"` deployment name would
  otherwise see their prefix matches overlap. Stale links survive a
  hostile run-over of a live overlay every time.
- `--data-dir` now actually isolates daemon logs, runtime state, and the
  control socket. Previously the CLI promised "Other directories (logs,
  run, containers) are derived from this unless individually overridden"
  but `default_log_dir` / `default_run_dir` / `default_socket_path`
  ignored the resolved data dir and always returned `/var/log/zlayer`,
  `/var/run/zlayer`, and `/var/run/zlayer.sock` on Linux. Passing
  `--data-dir /tmp/foo` would still collide with a system-installed
  daemon. The path resolver now exposes data-dir-aware variants
  (`default_log_dir_for`, `default_run_dir_for`,
  `default_socket_path_for`) and the CLI wrappers use them: stock
  installs are unaffected (FHS layout preserved) but custom data-dirs
  derive `{data_dir}/logs`, `{data_dir}/run`, and
  `{data_dir}/run/zlayer.sock`. Fixes the e2e harness collision with a
  running root-owned daemon.
- The WireGuard UAPI socket directory is now data-dir-aware. Stock
  installs still write boringtun UAPI sockets to `/var/run/wireguard`
  (so `wg(8)` can talk to them), but a daemon launched with
  `--data-dir /tmp/foo` now writes them to `/tmp/foo/run/wireguard`,
  matching the rest of the data-dir-aware path resolver. The boot sweep
  scoped to `zl-<deployment-name>-*.sock` walks the same data-dir-aware
  directory, so an isolated test daemon no longer touches the
  host-global socket dir even by accident. New
  `ZLayerDirs::wireguard()` helper centralises the path; new
  `OverlayConfig::uapi_sock_dir` field threads it through the overlay
  transport (defaulting to the FHS path for source-compat).
- The Manager UI's compiled CSS no longer contains malformed
  `.\[api\:8080\]{api:8080}` / `.\[cache\:6379\]{cache:6379}` /
  `.\[database\:5432\]{database:5432}` / `.\[web\:443\]{web:443}`
  rules that triggered "Unknown property" console warnings in
  Firefox. Tailwind v4's auto-detect pass was scanning the whole
  workspace and treating ASCII-art port-mapping diagrams in
  `examples/containers/multi-service/deployment.yaml` as arbitrary-
  property class names. `crates/zlayer-manager/tailwind.css` now
  explicitly excludes `examples/`, `docs/`, `images/`, and `tests/`
  from the scan.
- The Manager UI no longer gets stuck in a "ghost session" when the
  upstream API rejects a forwarded `zlayer_session` cookie at
  `GET /auth/me` (e.g. the user row was deleted, the data directory was
  swapped, or the daemon restarted with an empty `users.db`). The
  `manager_me` server function now fires a best-effort
  `POST /auth/logout` when it sees a 401 against a forwarded session
  cookie, which returns expired `Set-Cookie` headers for both
  `zlayer_session` and `zlayer_csrf`. The browser drops the stale
  cookies and the next request lands cleanly on `/login` (or
  `/bootstrap` if the user table is empty) instead of re-replaying the
  dead session forever.

## [0.11.21] - 2026-05-10

### Fixed
- Upgrades from versions prior to 0.11.20 no longer crash-loop on first
  boot under the new `secrets/` directory layout. The daemon now
  migrates the legacy top-level `secrets` SQLite file into the new
  layout in place on every startup, and the same migration is exposed
  as a manual subcommand (see `zlayer daemon migrate` below). Linux,
  macOS, and Windows all run the same migration path.
- Early-init failures (corrupt redb, missing keys, layout mismatches,
  unparseable JWT secret) now surface in stderr — and therefore in
  journald on Linux, the unified log on macOS, and the Event Log on
  Windows — instead of the daemon exiting silently with status 1. The
  stderr-to-tracing redirect that swallows these messages is now
  installed only after the daemon has fully initialised, so the final
  error print on startup failure reaches the system log.
- The daemon no longer double-forks when launched under systemd.
  Presence of `NOTIFY_SOCKET` or `INVOCATION_ID` in the environment is
  treated as a signal to stay in the foreground so systemd's
  `Type=notify` readiness handshake works as designed; previously the
  daemon's own `daemon()` call detached from systemd and masked every
  startup error as a "service exited cleanly" event.
- A minimal stderr `tracing_subscriber` is now installed as the very
  first action in `main()`, so panics, argument-parse failures, and any
  error that fires before the full observability stack is up still
  reach the system log instead of vanishing.

### Added
- `zlayer daemon migrate [--data-dir <path>] [--dry-run]` — manual,
  idempotent data-directory migration. Safe to re-run; used by the
  install scripts as a belt-and-suspenders step after `daemon install`,
  and available as an escape hatch for operators recovering a node by
  hand.
- The systemd unit now sets `StandardOutput=journal`,
  `StandardError=journal`, and `SyslogIdentifier=zlayer`, so daemon
  output is captured in journald even when the binary writes to stderr
  directly, and `journalctl SYSLOG_IDENTIFIER=zlayer` works without
  having to know the unit name.
- `install.sh` and `install.ps1` now pre-create the `secrets/`
  directory with tight permissions and run `zlayer daemon migrate`
  after `daemon install`, so the very first boot after an upgrade is
  already on the new layout.

## [0.11.19] - 2026-05-10

### Fixed
- `cargo publish` failure on `zlayer-secrets`. The crate's
  `zlayer-types` dep was declared path-only (`{ path = "../zlayer-types" }`)
  with no `version` field, which `cargo publish` rejects. Switched to
  `zlayer-types.workspace = true` to match every other crate in the
  workspace; the workspace entry already carries the placeholder version
  that the release sed step rewrites at publish time. Earlier
  `0.11.17`/`0.11.18` retries hit the same wall partway through the
  publish chain (early crates landed on crates.io, the rest never made
  it); the action's idempotency check skips already-live crates so
  re-running `0.11.19` resumes at `zlayer-secrets`.

## [0.11.16] - 2026-05-08

### Fixed
- Manager mutating server_fns no longer 403 with
  `"CSRF token missing or does not match session cookie"`. The Leptos
  hydrate side now installs a `window.fetch` wrapper on startup
  (`crates/zlayer-manager/src/csrf_client.rs`) that reads the
  JS-readable `zlayer_csrf` cookie and echoes it as `x-csrf-token` on
  every fetch. The daemon already enforces the double-submit check
  (`crates/zlayer-api/src/middleware/csrf.rs`); the browser half was
  unimplemented and any POST/PATCH/DELETE from the Manager UI would
  have detonated the moment a user got past `/auth/me`. GET requests
  are CSRF-exempt so this was invisible until a mutating call was made.
- Manager `manager_me`, `manager_list_*`, and every other read-side
  server_fn that calls the daemon now propagates upstream `Set-Cookie`
  headers back to the browser. Previously only `manager_login`,
  `manager_bootstrap`, and `manager_logout` did, so daemon-side cookie
  rotations / session refreshes / 401-clear-cookie hints disappeared
  silently between fetch and browser. Centralised through a new
  `forward_raw` helper in `crates/zlayer-manager/src/app/server_fns.rs`
  that wraps `client.raw_request(...)` with `propagate_set_cookies`;
  ~60 call sites across users, variables, secrets, tasks, notifiers,
  workflows, groups, permissions, audit, and projects converted in one
  pass. The bootstrap-state probe inside `manager_me` deliberately
  bypasses the helper since it is an unauthenticated state probe, not
  part of the user's auth flow.

### Changed
- `extract_forwarded_headers` (manager SSR) now logs `cookie_names`
  (no values) plus `has_session` / `has_csrf_cookie` booleans, so
  diagnosing whether the `zlayer_session` cookie reached the manager
  no longer requires inferring from a 32-char preview. The redundant
  `manager_me: about to forward cookie` block (which only logged the
  first 32 chars and effectively only ever showed the CSRF cookie
  name) was removed; the new fields cover every server_fn entry, not
  just `/auth/me`.
- `manager_me` now logs the upstream response body verbatim on
  non-success status, so the daemon's own
  `"Session cookie missing"` / `"Session expired"` JSON message is
  visible from the manager log alone without having to splice in
  daemon journal entries.

## [0.11.15] - 2026-05-08

### Added
- `ProtectedShell` component (`crates/zlayer-manager/src/app/components/protected_shell.rs`)
  combining `AuthGuard` with the drawer / Navbar / Sidebar chrome.
  Authenticated routes render inside it; `/login` and `/bootstrap` render
  bare so the sidebar and navbar are no longer visible to logged-out users.
- `zlayer_registry::manifest_cache_key(image)` helper alongside the
  existing `manifest_digest_cache_key`. Both registry-side writers
  (`client.rs`, `oci_export.rs`) and agent-side readers
  (`runtimes/youki.rs`) now compose the manifest body cache key through
  one function instead of raw `format!("manifest:{image}")` strings,
  closing the same drift hazard that broke image-recreate detection on
  the digest sidecar (fixed in 0.11.14). `crates/zlayer-registry/src/cache.rs`
  documents the construction rule at the top of the file.

### Fixed
- `zlayer up -b` / `zlayer up -d` no longer fails with `401 Unauthorized`
  on the daemon host. The CLI's `apply_session_auth` is now a no-op on
  Unix; the daemon's UDS middleware injects the local-admin Bearer for
  any request lacking `Authorization`, so a stale `~/.zlayer/session.json`
  signed under a previous JWT secret no longer shadows that injection.
  Root on the daemon host never has to "log in." Windows behaviour
  unchanged: TCP loopback still uses the file-backed admin bearer +
  optional session.
- Daemon Unix socket permissions reverted from `0o666` to `0o660` (in
  `bind_dual_with_local_auth` and `ApiServer::run_dual`). The earlier
  flip rationalised "non-root users can connect via systemd," but
  combined with the UDS middleware's auto-injection of the admin Bearer
  it meant any local user could claim full daemon-API admin by
  `connect()`-ing the socket. Tight perms are the access-control;
  non-root operators needing CLI access should be added to the daemon's
  primary group. Test assertion updated to match.

### Changed
- Manager `App` shell no longer renders the drawer / Navbar / Sidebar
  unconditionally. Those components are now reachable only via the new
  `ProtectedShell` wrapper applied to authenticated routes (see Added).

## [0.11.14] - 2026-05-06

### Added
- Structured tracing across the `/auth/me` path so failed manager-login flows
  can be diagnosed without speculation. New fields: `auth_state_present`,
  `session_cookie_present`, `cookie_len`, `token_verify_result`, `sub_prefix`
  on `SessionAuthUser`; named JWT failure `variant` (Expired /
  InvalidSignature / InvalidToken / Crypto / …) on `verify_token`;
  `actor_path` (bearer / cookie / none) on `AuthActor`; `store_result`
  (found / missing / error) on `me`; one-time `jwt_secret_fp` (8-hex
  SHA-256 prefix) at router build; and `cookie_preview` (32-char prefix)
  in the manager's `manager_me` server function. Daemon-side lines emit
  at `warn!` (visible in default daemon logs at `/var/log/zlayer/daemon.log.*`
  and journald without setting `RUST_LOG`); manager-side lines emit at
  `info!` (visible via `zlayer logs zlayer-manager` since the manager
  image bakes `RUST_LOG=info`). None log secrets, full tokens, or full
  session subjects. The elevated daemon-side levels are temporary while
  diagnosing the post-login auth bug; they revert to `debug!` once the
  root cause is fixed.
- `zlayer-secrets` now exposes `JwtSecretManager`, mirroring `KeyManager`
  for the API daemon's JWT signing secret. The daemon resolves it as:
  `--jwt-secret` / `ZLAYER_JWT_SECRET` env, then a persisted file at
  `{data_dir}/jwt_secret_zlayer.key` (mode 0600), then auto-generate +
  persist 64 random bytes. Previously, omitting `--jwt-secret` silently
  used the literal `"CHANGE_ME_IN_PRODUCTION"`, and any operator who set
  the env var once and forgot it would invalidate every previously issued
  session cookie on the next restart.

### Fixed
- `zlayer up` now actually rolls running replicas when the upstream image
  digest moves on a floating tag (e.g. `:latest`). The youki runtime's
  `list_images` was reading the manifest digest sidecar under
  `manifest-digest:{ref}` while `zlayer-registry` writes it under
  `manifest:digest-{ref}`, so the reader always saw `digest: None` and the
  `should_recreate` branch in `upsert_service` short-circuited to "no
  recreate." Both crates now share `zlayer_registry::manifest_digest_cache_key`
  to keep the format in sync. Users no longer have to run `zlayer pull`
  before `zlayer up -b` / `zlayer up -d`.

## [0.11.13] - 2026-05-06

### Added
- `zlayer manager init` now reuses an existing `bootstrap/ZLAYER_BOOTSTRAP_PASSWORD`
  secret instead of re-prompting on every run. Pass `--force` (or any of
  `--password` / `--password-file` / `--random`) to rotate the stored value.
- Diagnostic tracing in the manager's `extract_forwarded_headers` and
  `manager_me` server fns to log cookie-forwarding state and upstream
  `/auth/me` status — surfaces which layer is dropping cookies on
  post-login probes.

### Changed
- Manager sidebar version footer now reads from `env!("CARGO_PKG_VERSION")`
  instead of the hard-coded `v0.1.0`, so dev builds show `0.0.0-dev` and
  release builds reflect the workspace version automatically.

### Fixed
- Manager favicon and sidebar logo (`/assets/zlayer_logo.png`) no longer 404.
  cargo-leptos preserves the `assets-dir` subdirectory layout when copying to
  `site-root`; the file lives under `assets/assets/` so it lands at
  `target/site/assets/zlayer_logo.png` to match the `nest_service("/assets",
  …)` mount.

## [0.11.12] - 2026-05-04

### Added
- Docker compat: Swarm endpoints (`/swarm`, `/services`, `/tasks`, `/nodes`, `/secrets`, `/configs`) now bridge to ZLayer's native cluster primitives (deployments, cluster nodes, replicas, secrets, variables) instead of returning empty arrays. Tools like `docker stack ps`, Portainer, and `docker service ls` now see ZLayer's real cluster state.
- Docker compat: `/info` Swarm field now reports ZLayer cluster topology (NodeID, NodeAddr, LocalNodeState, ControlAvailable, RemoteManagers, Cluster.ID).
- Docker compat: CLI redirects added for `docker swarm|node|service|secret|config` — these print a hint pointing users at the native `zlayer cluster|deploy|secret|variable` commands.

### Changed
- Docker compat: `crates/zlayer-docker/src/socket/swarm.rs` refactored into a `swarm/` submodule with one file per resource family (shape, swarm, nodes, services, tasks, secrets, configs).

## [0.11.11] - 2026-05-03

### Added
- `ServiceSpec` gained `tty`, `stdin_open`, `userns_mode`, `cgroup_parent`,
  and `expose` fields so the Docker Compose converter can plumb them through
  end-to-end.
- Compose-to-deployment conversion now honours every previously silent-dropped
  service field: `labels`, `user`, `read_only`, `init`, `tty`, `stdin_open`,
  `stop_signal`, `stop_grace_period`, `sysctls`, `ulimits`, `security_opt`,
  `pid`, `ipc`, `cgroup_parent`, `devices` (parsed into `DeviceSpec`),
  `deploy.resources.reservations.devices` GPU requests (folded into
  `resources.gpu`), `cap_drop`, top-level `restart` and Swarm-style
  `deploy.restart_policy`, `network_mode`, `expose:` port lists,
  `pull_policy`, `platform`, healthcheck `CMD-SHELL` arrays,
  healthcheck `retries: 0` and `start_interval`, and
  `deploy.resources.reservations` (cpu/memory).

### Changed
- `convert_resources` now folds `deploy.resources.reservations.cpus` into
  `ResourcesSpec.cpu_shares` (using the standard 1024-per-core weight) and
  `reservations.memory` into `ResourcesSpec.memory_reservation`.

## [0.11.10] - 2026-05-02

### Fixed
- macOS sandbox and VM runtimes (`pull_image_with_policy`) no longer fail to
  compile after the `PullPolicy::Newer` variant was added; `Newer` is now
  handled at the runtime layer like `Always` (always re-pull), with drift
  detection remaining at the service layer.

## [0.11.9] - 2026-05-01

### Added
- `zlayer up --pull` and `--no-pull` flags (mutually exclusive) to override
  per-service `pull_policy` for a single deploy.
- `-d` short alias for `--detach` global flag.
- `zlayer pull` with no arguments now auto-discovers the spec and pulls every
  distinct image referenced by services.
- Spec auto-discovery now matches `zlayer.<name>.yml` / `zlayer.<name>.yaml`
  (prefix form) in addition to the existing `<name>.zlayer.yml` (dot form).
- `PullPolicy::Newer` variant — resolves remote digest and recreates
  containers on drift; auto-applied as the default for `:latest` / untagged
  images via `effective_pull_policy()`.
- Fine-grained deploy progress events for the TUI:
  `ServiceRegistrationStarted`, `OverlaySetupStarted`, `ProxySetupStarted`,
  `StabilizationProgress`, `ImagePullStarted`, `ImagePullComplete`,
  `ServiceUpToDate`, `ServiceRecreating`. The deploy TUI now renders
  per-service phase progress instead of long silent gaps.
- `scripts/check-all.sh` cross-target workspace check (native, wasm32,
  x86_64-pc-windows-msvc, aarch64-apple-darwin) with optional `--with-tests`
  flag.
- ZImagefile/Dockerfile `COPY --from=<external-image-ref>` now works for
  arbitrary registry images (e.g.
  `COPY --from=ghcr.io/astral-sh/uv:0.5.0 /uv /usr/local/bin/uv`). The
  buildah backend pulls the external image once into the build's storage
  and forwards the reference to buildah's native COPY.

### Changed
- `zlayer up` re-deploy reconciles image-digest drift on existing services.
  With `pull_policy=Newer` (the default for `:latest`), the daemon resolves
  the remote digest, compares to the running container's digest, and
  rolling-recreates when they differ. Previously, re-running `zlayer up` on
  the same spec was a silent no-op even when `:latest` had been re-pushed.
- Daemon emits per-service "started" events around each orchestration phase
  (registration, overlay, proxy, image pull) instead of only on completion,
  eliminating the multi-second TUI freezes the user observed during deploys.

### Fixed
- Registry pull no longer logs `failed to delete stale manifest digest cache
  entry` warning. The cache key prefix was renamed from `manifest-digest:`
  to `manifest:digest-` so it satisfies the `manifest:` validator branch.
- Package map verifier (`verify-reposources.yml`) no longer pins
  `openssl@3` literally — now wildcards `openssl@*` so the assertion
  survives upstream major bumps (OpenSSL 4 already shipped).
- `zlayer-manager` SSR panicked on the first request with
  `js-sys-0.3.91/src/lib.rs:13087: cannot access imported statics on
  non-wasm targets`. Root cause was the locked transitive
  `wasm-bindgen 0.2.114 / js-sys 0.3.91` pair, which leptos→tachys exercises
  on SSR codepaths with extern statics that panic-on-call on non-wasm
  targets in that release line. Bumped via
  `cargo update -p wasm-bindgen-futures -p web-sys -p wasm-bindgen -p js-sys`
  onto the `0.2.120 / 0.3.97` line where the extern-statics no longer
  panic. Reverted the over-aggressive `optional = true` /
  `default-features = false` changes on
  `leptos`/`leptos_meta`/`leptos_router`/`leptos-chartistry` in
  `crates/zlayer-manager/Cargo.toml` — those were a guess that broke the
  `#[component]` macro's doc-comment-on-params handling on
  `cargo test --workspace --lib --bins` (CI builds the manager with no
  features, so `dep:leptos` was inactive and the proc-macro never ran).
- `zlayer-manager` SSR panicked on the first request with `js-sys` "cannot
  access imported statics on non-wasm targets". The Navbar component called
  `web_sys::window()` from inside `apply_theme`, `read_saved_theme`, and the
  `on_logout` closure with no cfg gate, so the bodies were codegen'd into the
  native server binary even though the surrounding `Effect::new` /
  `spawn_local` only fire on the client. Gated all three sites behind
  `#[cfg(target_arch = "wasm32")]` to match the pattern already used in
  `settings.rs` and `secrets.rs`; SSR `read_saved_theme` now returns `"dark"`
  and the browser re-applies the persisted theme during hydration.
- `test_map_linux_packages_falls_back_to_common` was non-hermetic: when a
  per-distro shard wasn't seeded, `fetch_or_load_shard` reached out to live
  RepoSources, and the upstream's stale `centos_8/l.json`
  (`libssl-dev → openssl`) clobbered the seeded `common/l.json`
  (`libssl-dev → openssl@3`) because per-distro wins on conflict in the
  merge. Test now seeds explicit empty shards for every probed
  `(label, shard)` pair so the resolver hits the local cache only.
- `scripts/generate-package-maps.py` dump load failed with `cannot drop
  schema repology because other objects depend on it` (pg_trgm, libversion).
  The Repology dump now starts with a non-CASCADE `DROP SCHEMA repology`,
  and our pre-installed extensions had been created in that schema by
  default (first writable entry in `search_path = repology, public`).
  Extensions now created with explicit `SCHEMA public`, so they survive
  the dump's schema reset.
- `scripts/generate-package-maps.py` Repology dump load silently produced
  an empty database. Three concrete causes addressed:
  - The dump references `repology.<table>` but ships no
    `CREATE SCHEMA repology`, so every CREATE/INSERT failed against the
    missing schema; `setup_postgres` now pre-creates it.
  - `ON_ERROR_STOP=0` masked those errors and let psql exit 0 on a
    half-loaded database; bumped to `ON_ERROR_STOP=1`.
  - The shell pipeline didn't use pipefail, so a truncated curl/zstd
    download surfaced as a successful psql run; loader now runs under
    `sh -o pipefail` with `curl -fL --show-error` and per-stage stderr
    capture, and `diagnose()` raises `SystemExit` if `repology.packages`
    is missing or empty instead of continuing to "Found 0 repos".

## [0.11.8] - 2026-04-29

### Added
- `scripts/generate-package-maps.py` now publishes a cross-distro consensus
  shard set at `public/maps/common/<shard>.json` alongside the existing
  per-distro shards. The `common/` set is the union of all per-distro
  mappings with most-common-wins consensus (versioned-highest tiebreak),
  and is what the verify workflow asserts against. The macOS resolver
  performs cascading lookup: per-distro shard first, `common/<shard>`
  fallback, distro winning on conflict — so `apt install libssl-dev`
  resolves correctly even when the source distro's image is centos
  (`libssl-dev` doesn't exist on centos but `common/` carries it).
- `scripts/generate-package-maps.py --test` self-test mode covering
  `pick_winner` and `build_consensus` against synthetic alias/formula
  data; runs in seconds without postgres or the Repology dump.

### Changed
- `pick_winner` rewritten to a deterministic 4-step algorithm: canonicalize
  candidates via brew's alias index + dedupe (collapses
  `[openssl, openssl@3]` to `[openssl@3]` because brew aliases `openssl`
  to `openssl@3`); resolve the Linux name through brew aliases with no
  major-match veto; versioned-preference tiebreak when the canonical is
  unversioned but `<canonical>@<MAJOR>` variants exist (`nodejs` → `node@24`);
  versioned-highest fallback. Drops `parse_brew_major` and the per-package
  `linux_version` plumbing — they were dead weight after the redesign.
  No version-pin overrides; `OVERRIDES` stays scoped to the structural
  cases brew/Repology genuinely cannot derive (debian's `default-jdk`/
  `default-jre` metapackages).
- `write_sharded_map_files` no longer merges with existing on-disk shards —
  each generator run is authoritative for its own data. The cross-distro
  consensus union preserves names that disappear from one distro's
  Repology, so the protective intent of "packages only grow" is satisfied
  in `common/` instead. Eliminates zombie entries from prior runs (e.g.
  centos_8 retaining `libssl-dev: openssl` from an earlier OVERRIDES
  shape).
- `.forgejo/workflows/verify-reposources.yml` retargeted: cross-distro
  consensus assertions now run once against `common/<shard>.json` instead
  of four times against per-distro shards. Per-distro freshness checks
  remain, plus a new freshness check for `common/index.json`.
- `crates/zlayer-builder/src/macos_image_resolver.rs::PACKAGE_MAP_CACHE_SUBDIR`
  bumped from `package-maps-v2` to `package-maps-v3` to invalidate caches
  that pre-date the cross-distro layout.

### Fixed
- `verify-reposources.yml` daily failure: published shards returned
  unversioned brew formulas (`openssl`, `python`, `node`) where the
  resolver expects versioned (`openssl@3`, `python@3.*`, `node@*`). Root
  cause: per-distro `pick_winner` couldn't satisfy a cross-distro
  consistency invariant (EOL distros, distro-only names, Repology
  candidate-list gaps); merge-with-existing kept stale entries forever.
  The deterministic `pick_winner` + cross-distro `common/` union + clean
  per-distro overwrite together cure all three.

## [0.11.5] - 2026-04-27

### Changed
- macOS package resolution now derives brew formula names dynamically from
  Repology's Linux version field plus Homebrew's formula list. The generator
  emits `<base>@<MAJOR>` real-formula pins where the Linux name implies a
  major version (e.g. `libssl-dev → openssl@3`, `nodejs → node@22`), and
  alias-canonicals where brew uses an alias (e.g. `python3 → python@3.14`).
  `OVERRIDES` in `scripts/generate-package-maps.py` is drained to
  `{default-jdk, default-jre}` only — entries Repology genuinely cannot
  resolve.
- `.forgejo/workflows/verify-reposources.yml` assertions switched from exact
  string equality to POSIX glob matching (`python@3.*`, `node@*`) so brew
  rotating its default doesn't break the workflow.
- The package-map cache directory bumped from `package-maps/` to
  `package-maps-v2/`; pre-Phase-2 caches are abandoned (would otherwise
  return alias-only names like `python@3` that 404 against the brew API).

### Fixed
- `.forgejo/workflows/verify-reposources.yml` BusyBox `date -d` could not
  parse the ISO-8601 `YYYY-MM-DDTHH:MM:SSZ` form the generator emits;
  every nightly run false-failed the freshness check. Added `coreutils`
  for GNU `date`, dropped the silent `|| echo 0` fallback that hid the
  parse failure, and reordered the null-check before the date parse.
- `macos_image_resolver.rs::resolve_package` Python `UvPython` special-case
  now matches `python@3.X` patterns (was `python` / `python3` / `python@3`
  only). The new generator emits `python@3.14`-style names for the
  `python3` Linux package, which would otherwise have routed to a brew
  bottle install instead of uv-managed Python.
- `map_single_package_hardcoded` pruned to a pass-through. Its prior
  match arms hardcoded names like `python3 → python@3` (alias-only, 404s
  on `formulae.brew.sh/api/formula/python@3.json`) and contradicted the
  dynamic generator output for `libssl-dev`, `openjdk-17-jdk`, etc.
- `test_map_linux_packages_live_reposources` correctly gated `#[ignore]`
  (CHANGELOG previously claimed this had landed; the attribute was
  missing from source).

### Added
- `crates/zlayer-builder/src/bottle_lockfile.rs`: per-spec
  `zlayer-bottles.lock` schema for reproducible macOS bottle installs.
  Stand-alone module with TOML load/save and unit tests.
- `macos_image_resolver::resolve_package` and `install_with_deps` now take
  an optional `&BottleLockfile` and a `&mut Vec<LockedBottle>`: a hit on
  the lockfile short-circuits the live brew API entirely; every freshly
  resolved `HomebrewBottle` (root + transitive deps) is captured for
  rewrite at end of build.
- `SandboxImageBuilder` loads `<spec_dir>/zlayer-bottles.lock` once at
  the start of `build()`, threads it through every `RUN` interception,
  and rewrites the file post-build when at least one bottle was resolved.
  Unrelated pins from a prior lockfile are carried forward unless the
  user requested a full refresh.
- `zlayer build --update-bottles` flag: ignores any existing lockfile,
  forces live resolution for every formula, and rewrites the file from
  scratch. Mirrors `cargo update`. Threaded through `BuildOptions`,
  `ImageBuilder::update_bottles`, and `SandboxImageBuilder::with_update_bottles`.
  No-op on Linux/Windows builds (flag still parses cleanly).
- `BlackLeafDigital/functions/RepoSourceSyncer/src/modules/refresh.ts`:
  daily Appwrite CRON pulls the full Homebrew formula list, expands
  every formula to canonical + alias + oldname rows, and bulk-upserts
  via `tablesDB.upsertRows` (batches of 1000). Mirrors GitHub Pages so
  the resolver's first-step cache hits 100% for any brew-known name —
  including alias-only names like `python@3`. New `last_seen_in_brew_at`
  column lets us soft-keep formulas brew has removed.

## [0.11.4]

### Fixed
- **`zlayer-types` is publishable to crates.io again.** Proprietary-branch (`zlayer-zql`) config had leaked into the public manifest: `crates/zlayer-types/Cargo.toml` carried a hardcoded `version = "0.1.0"`, an `authors` line, and `publish = ["forgejo"]`, while the workspace dep alias in the root `Cargo.toml` (`zlayer-types = { path = ... }`) was missing a version requirement. As a result, `cargo publish -p zlayer-spec` failed with `dependency \`zlayer-types\` does not specify a version` mid-release. Switched `zlayer-types` to workspace inheritance (`version.workspace = true`, etc.), added `version = "0.0.0-dev"` to the workspace alias, and converted every consumer (`zlayer-agent`, `zlayer-api`, `zlayer-builder`, `zlayer-client`, `zlayer-core`) from `path = "../zlayer-types"` to `zlayer-types.workspace = true` so `sed -i "s/0\.0\.0-dev/${VERSION}/g" Cargo.toml` in `.forgejo/workflows/release.yml` substitutes the release version uniformly. Files: `Cargo.toml`, `crates/zlayer-types/Cargo.toml`, `crates/zlayer-{agent,api,builder,client,core}/Cargo.toml`.

## [0.11.0]

### Added
- **Proxy honors `CF-Connecting-IP` / `X-Forwarded-For` from trusted upstream proxies.** When a ZLayer-proxy origin sits behind Cloudflare or any other reverse proxy, the backend now sees the real visitor IP instead of the CF edge IP. `ZLayerProxyConfig` gained `trusted_proxy_cidrs: Vec<ipnet::IpNet>` (default: `[127.0.0.0/8, ::1/128]` — localhost only, safe on a public node that accidentally receives direct requests) and `cloudflare_trust: CloudflareTrust { Off | Static | AutoRefresh { interval } }` (default `Off`). New `crates/zlayer-proxy/src/cf_ip_list.rs` maintains a Cloudflare edge IP cache: baked-in IPv4/IPv6 fallback list (current as of 2026) plus optional auto-refresh from `https://www.cloudflare.com/ips-v4` and `ips-v6` on a configurable interval, lock-free reads via `RwLock<Vec<IpNet>>`. New `crates/zlayer-proxy/src/trust.rs` exposes `TrustedProxyList` combining user CIDRs + optional CF cache into a single `is_trusted(peer: IpAddr) -> bool` predicate. `ReverseProxyService` gained `trusted_proxies: Arc<TrustedProxyList>` (default `localhost_only`), a `with_trusted_proxies` builder, and rewritten `add_forwarding_headers` logic: when the TCP peer is trusted, `CF-Connecting-IP` wins, then leftmost `X-Forwarded-For`, and the real client IP is prepended to any existing XFF chain; `X-Real-IP` is set to the effective IP. Untrusted peers get the original append-peer behavior — spoofed headers from direct attackers are discarded. New workspace dep `ipnet = "2"`. Files: `crates/zlayer-proxy/src/{lib,cf_ip_list,trust,service}.rs`, `Cargo.toml`, `crates/zlayer-proxy/Cargo.toml`.
- **`zlayer import` now accepts an `http(s)://` URL with optional HTTP Basic auth.** `zlayer import <url> [--username U] [--password P] [--tag T]` fetches an OCI tar archive over HTTP and imports it in one step — no intermediate `curl` + local file. Primary motivation: ZLayer releases publish `zlayer-*-<ver>-oci.tar` to Forgejo's generic package registry (not a container registry), and operators on target nodes previously had to `curl` the tar before `zlayer import`. Now a single command handles it, with Basic auth for private Forgejo/Gitea/Nexus registries. New `zlayer_registry::{fetch_from_url, fetch_archive_from_url}` public helpers in `crates/zlayer-registry/src/client.rs` — `fetch_from_url` is the low-level HTTP primitive that the existing `fetch_blob_from_url` (foreign-layer redirects, with SHA-256 + size verification) now shares. `crates/zlayer-registry/src/oci_export.rs::import_image` was refactored to split file-read from tar-parsing: new `import_image_from_bytes` entry point lets callers feed an in-memory archive straight to the importer. Compression detection in the bytes path now relies solely on the gzip magic number (no filename needed). Local-path input on `zlayer import` still works identically; `--username` / `--password` are rejected for non-URL inputs with a warning. Six new unit tests in `client.rs::tests` cover invalid URL / unreachable host / basic-auth / context-label error paths. Files: `crates/zlayer-registry/src/client.rs`, `crates/zlayer-registry/src/oci_export.rs`, `crates/zlayer-registry/src/lib.rs`, `bin/zlayer/src/cli.rs`, `bin/zlayer/src/main.rs`, `bin/zlayer/src/commands/registry.rs`.
- **`zlayer docker install` / `zlayer docker uninstall` — one-shot Docker compatibility setup.** New CLI commands that make the `docker` and `docker compose` CLIs transparently route through ZLayer. The install flow reconfigures the daemon service (systemd / launchd / SCM) to expose the Docker Engine API socket, drops `docker` and `docker-compose` shims on PATH, writes `DOCKER_HOST` + `DOCKER_BUILDKIT=0` to the user's shell profile (Unix) or user environment (Windows via registry), and optionally offers to symlink `/var/run/docker.sock` (Unix) to the ZLayer Docker socket with a prompt for existing installations. Windows is first-class: the Docker Engine API socket server (`crates/zlayer-docker/src/socket/`) gains a named-pipe transport (`\\.\pipe\zlayer-docker`) alongside the existing Unix-domain-socket path, lifting its `#[cfg(unix)]` gate. A new `zlayer_paths::ZLayerDirs::default_docker_socket_path()` centralizes the per-platform default. The prior inline Linux shim-install in `zlayer daemon install --docker-socket` is refactored to call the shared `zlayer_docker::shim::{install_shim, uninstall_shim}` helper so Linux uninstall also removes the shim, and the `--docker-socket` flag on macOS and Windows `daemon install` now actually takes effect (previously silently ignored).
- **Phase G — WSL2 delegate runtime for Linux workloads on Windows nodes.** Completes the `Wsl2DelegateRuntime` (`crates/zlayer-agent/src/runtimes/wsl2_delegate.rs`) so Linux containers scheduled onto a Windows host run under `youki` inside a WSL2 distro. G-1 refactored `crates/zlayer-agent/src/bundle.rs` to split OCI spec generation from Unix-specific device probes, letting the delegate reuse the spec builder cross-platform. G-2 wired a real bundle-write path in `create_container` — writes `config.json` + rootfs into the distro-visible UNC path, replacing the previous `Unsupported` stub. G-3 replaced the exec stub with real streaming via `wsl.exe -d <distro> -- youki exec`, forwarding stdin/stdout/stderr. G-4 plumbed the youki log file path into the WSL2 invocation so container logs are retrievable from the Windows side. G-5 made the distro name and youki binary path config-driven (surfaced through `AgentConfig::wsl_distro` / `wsl_youki_path`) instead of hardcoded. G-6 added the auto-install consent flow: `ensure_wsl_backend_ready_with_consent` runs an elevated `wsl --install --no-distribution` via `ShellExecuteExW "runas"` when the user opts in, with `WslError::{InstallRefused, RebootRequired}` variants distinguishing opt-out from exit-code-3010 reboot cases.
- **Phase H — Windows cluster bootstrap (`node init`, `node join`, `zlayer join <url>`).** Full Windows body for the cluster-bringup CLI. H-1 implemented `handle_node_init` on Windows: detects platform capabilities, provisions the overlay (incl. Windows port-53 DNS listener from J-1), installs the WSL2 delegate if consent given, and writes the node identity. H-2 implemented `handle_node_join` (same bootstrap plus peer handshake). H-3 implemented the convenience `zlayer join <url>` flow that pulls join config from a control-plane URL and invokes `handle_node_join` under the hood. H-4 added the Windows branch of the GPU detector — enumerates DXGI adapters via `IDXGIFactory1::EnumAdapters1` and reports `GpuVendor::{Nvidia, Amd, Intel}` based on PCI vendor IDs. H-5 implemented `zlayer_paths::is_root()` on Windows using `CheckTokenMembership` against the Administrators SID (Unix body unchanged, fallback returns `false`). H-6 shipped a new `zlayer_overlay::firewall` module with a `WindowsFirewall` wrapper over `INetFwPolicy2` that opens overlay + daemon ports via COM, plus `iptables`-based Linux and `pfctl`-based macOS bodies behind the same trait.
- **Phase I — Windows Service integration for the daemon.** `zlayer serve --service` now runs as a proper Windows Service via the `windows-service` crate (I-1): registers a SCM control handler, accepts `Stop`/`Shutdown` control codes, and signals a `CancellationToken` so the tokio runtime shuts down cleanly before SCM marks the service Stopped. Service name `ZLayerDaemon`; `--service` is hidden from `--help` because it is only meaningful when SCM spawns the binary; foreground `zlayer serve` is unchanged. I-2 replaced the legacy detached-child-process spawn in `daemon install` with `ServiceManager::create_service` (AutoStart, `LocalSystem` account, binary `<current_exe> serve --service --bind <bind> --data-dir <data_dir>`, display name `ZLayer Daemon`). I-3 made `daemon stop` send the SCM `Stop` control code and poll `query_status` until `ServiceState::Stopped` or a 30s timeout — no more Task Manager instructions. I-4 made `daemon uninstall` best-effort stop then `service.delete()`, leaving foreground daemons unaffected. I-5 made `daemon status` query SCM and map `ServiceState` to user-facing strings, falling back to a TCP probe when the service is not registered (foreground daemon).
- **Phase J — Container network attachment on Windows.** J-1 threaded overlay DNS through the HNS endpoint: `OverlayManager::attach_container_hcn` now accepts `dns_server: Option<IpAddr>` + `dns_domain: Option<String>` and plumbs them into the `HostComputeEndpoint.Dns` schema via `EndpointAttachment::create_overlay`, so Windows containers inherit the overlay hickory resolver at namespace-attach time. New `OverlayManager::{set_dns_config, with_dns_config, dns_server_addr, dns_domain}` accessors expose the configured resolver + zone (service.rs wires them through); `HcsRuntime::set_next_container_dns` is the matching per-container stash consumed on `create_container`. Because HNS will not let us override the endpoint DNS port, added `zlayer_overlay::DnsServer::bind_windows_fallback(bind_ip)` which binds a second UDP+TCP listener on port 53 of the overlay IP, sharing the same authority as the primary 15353 listener — Linux workloads continue using 15353 untouched. J-3 replaced the push-only `record_container_ip` stub in `Wsl2DelegateRuntime::start_container` with a real per-container netns: creates the netns inside the WSL2 distro, veth-pairs to the host-end, assigns the overlay IP, and wires a default route before invoking `youki start`; `remove_container` tears the netns down; setup failure falls back to host networking with a clear warning log.
- **Phase K — Windows CI and end-to-end validation.** K-1 + J-2 added `docs/windows-ci-runner.md` documenting the existing MiniWindows self-hosted forgejo runner (label `windows-latest`), CI integration snippets using `uv run --python 3.12` for the pyo3 build-time Python dependency, the replication runbook for additional Windows runners, and the manual DNS-resolution validation procedure for Windows containers attached via HNS endpoints. K-2 added a `test-windows` job in `.forgejo/workflows/ci.yaml` that runs workspace unit tests under both the default and the `hcs-runtime,wsl` composite feature combos, plus `composite_dispatch_e2e` via `-- --ignored`, on the MiniWindows runner (same `runs-on: windows-latest` label as `check-windows` / `build-windows-amd64`; `uv run --python 3.12 --` prefix handles the pyo3 build-time dep). K-4 added `crates/zlayer-agent/tests/windows_cluster_join_e2e.rs`, a two-peer mixed-workload cluster test (Linux peer + MiniWindows) deploying both a Linux service (alpine) and a Windows service (nanoserver) in one deployment; asserts Running status, cross-OS overlay DNS resolution (`svc-linux.overlay.local` ↔ `svc-win.overlay.local`), and clean teardown (`#[ignore]`'d by default, runs via `cargo test -- --ignored` on MiniWindows with `ZLAYER_LINUX_PEER` set).
- **Phase L — Native Windows image builder.** L-1 added Windows base-image templates: `crates/zlayer-builder/src/templates/dockerfiles/windows-nanoserver.Dockerfile` and `windows-servercore.Dockerfile`, matching `Runtime::{WindowsNanoserver, WindowsServerCore}` variants, and detection heuristics that surface them when `*.sln` / `*.csproj` / `*.vcxproj` / `*.exe` are present in the build context (user overrides `--platform` / `os:` always win). L-2 introduced the explicit `os:` field in ZImagefile YAML and a `--platform` CLI flag on `zlayer build`, both feeding a unified `ImageOs` target-OS context down the builder pipeline. L-4 shipped the native Windows backend at `crates/zlayer-builder/src/backend/hcs/` implementing `BuildBackend` directly on top of `zlayer-agent::windows::{scratch, layer}` — the same NTFS primitives the runtime uses — covering scratch layer creation, `RUN` via `HcsCreateProcess`, NTFS diff capture via `BackupRead`, and an OCI manifest writer producing `os: windows` + `architecture: amd64` metadata; no Docker Desktop dependency. L-5 added `crates/zlayer-builder/src/windows/deps.rs::validate_dockerfile()` which catches `RUN choco install ...` / `RUN winget install ...` on nanoserver base images at parse time and emits an actionable error pointing users at servercore or a multi-stage build (detect-and-error for this iteration; auto-multistage injection is a follow-up). L-7 taught the pipeline engine to group builds into mixed-OS waves, dispatching Linux stages to buildah and Windows stages to the HCS backend within a single `ZPipeline.yaml` run. L-8 added `crates/zlayer-builder/tests/windows_templates.rs` (~8 cross-platform tests for template parsing + Runtime detection heuristics across `*.sln` / `*.csproj` / `*.vcxproj` / `*.exe`) and `crates/zlayer-builder/tests/windows_build_e2e.rs` (Windows-gated, `#[ignore]`'d) exercising the HCS backend end-to-end: nanoserver pull → scratch layer → trivial COPY → NTFS diff capture → OCI manifest write, plus the multi-stage-rejection and choco-on-nanoserver-rejection paths.

### Changed
- **`zlayer-types`: `utoipa` is now a hard dependency, not an optional Cargo feature.** The previous `utoipa = { workspace = true, optional = true }` + `[features] utoipa = ["dep:utoipa"]` setup gated all `derive(utoipa::ToSchema)` / `derive(IntoParams)` / `schema(...)` / `param(...)` attributes behind `#[cfg_attr(feature = "utoipa", ...)]`. Every consumer that wanted OpenAPI schema generation had to opt in via `features = ["utoipa"]`, and the only consumer that actually did was `zlayer-spec` — which depended on `zlayer-types` solely to re-export the spec module and didn't need utoipa at all. Worse, the API DTOs were already half-migrated to unconditional derives in the working tree, leaving `cargo build --features ssr` failing with ~60 `trait bound ToSchema not satisfied` errors on field-types in `src/spec/types.rs` and `src/storage.rs` whose derives were still cfg-gated. Removed the `[features]` block and the `optional = true` flag in `crates/zlayer-types/Cargo.toml`; stripped every `#[cfg_attr(feature = "utoipa", ...)]` and `#[cfg(feature = "utoipa")]` gate across `src/api/*.rs`, `src/spec/types.rs`, and `src/storage.rs` (~102 sites). Dropped the `features = ["utoipa"]` request from `crates/zlayer-spec/Cargo.toml:14` and `crates/zlayer-api/Cargo.toml:27`. utoipa adds a small but unconditional dependency cost to every zlayer-types consumer; in practice every workspace consumer either generates schemas or sits behind one that does, so making it unconditional is the simpler model.
- **macOS package maps sharded by first-letter for smaller fetches and reviewable git diffs.** `https://zachhandley.github.io/RepoSources/maps/{distro}.json` (~5 MB monolith, 19,775 mappings) is replaced by `{distro}/{a..z,_misc}.json` per-letter shards plus a `{distro}/index.json` manifest. The resolver now computes the set of shards needed for an `apt-get install` line — each input package plus its `try_name_transforms` variants (`-dev` stripped, `lib` prefix stripped, trailing version digits stripped) — and fetches them in parallel via `tokio::task::JoinSet`. A typical 5-package step drops from a 5 MB monolith fetch to ~600 KB across 3-5 shards. The generator (`scripts/generate-package-maps.py`) writes shards via `.tmp-{shard}.json` then `os.rename` for atomicity, and writes `{distro}/index.json` last so a verifier always sees a complete shard set or the previous run's index. Weekly regens now produce git diffs covering only shards with actual mapping changes (most runs touch 0–3 files) instead of a full 5 MB rewrite. The legacy `{distro}.json` is deleted on first run; only the in-tree resolver consumes this data and ships in lockstep — no external readers. HTTP 404 on a per-letter fetch (some distros legitimately have no entries for some letters, e.g. `q.json`) is cached as an empty mapping for 7 days so the resolver doesn't re-hit the network. Files: `scripts/generate-package-maps.py`, `crates/zlayer-builder/src/macos_image_resolver.rs`, `.forgejo/workflows/update-package-maps.yml`, `.forgejo/workflows/verify-reposources.yml`. New `compute_required_shards`, `load_shards`, `load_one_shard`, `shard_key` helpers in the resolver; new `write_sharded_map_files`, `write_distro_index`, `shard_key`, `SHARDS` in the generator; verifier groups assertions by shard letter and fetches each unique shard exactly once per distro (~24 small fetches/run vs. today's 4 × 5 MB).
- **`zlayer-agent` now consumes the patched libcontainer/libcgroups via published `zlayer-libcontainer` / `zlayer-libcgroups` crates instead of a git override.** `crates/zlayer-agent/Cargo.toml` previously declared `libcontainer = { version = "0.6.0", git = "https://github.com/ZachHandley/youki", branch = "zlayer-patches", optional = true }`. At publish time `cargo publish` strips the git URL and packages the dep as plain `libcontainer = "0.6.0"`, so anyone installing `zlayer-agent` from crates.io with `--features youki-runtime` resolved the **unpatched upstream 0.6.0** — the cgroup-v2 EBUSY / DinD fix (PR #3347) and the upstream 0.6.1 patches silently disappeared for downstream consumers. Replaced with `libcontainer = { package = "zlayer-libcontainer", version = "0.6.1-zlayer.1", optional = true }`. The `zlayer-libcontainer` and `zlayer-libcgroups` crates are published from `github.com/ZachHandley/youki@zlayer-patches` by `.github/workflows/publish-zlayer-fork.yml` on every push to that branch — version suffix auto-increments from the highest existing `-zlayer.N` on crates.io. The local Rust import name stays `libcontainer` via the `package =` rename, so no source files in `zlayer-agent` change. See `docs/youki-fork.md` for the publish workflow and retirement procedure.
- **H-7: Linux-workload dispatch policy is no longer silent.** `CompositeRuntime::select_for` in `crates/zlayer-agent/src/runtimes/composite.rs` used to fall through to the Primary (HCS) runtime when a Linux container landed on a Windows node without a WSL2 delegate. It now returns a new `AgentError::RouteToPeer` variant that the scheduler catches and uses to re-place the workload on a Linux peer. When no capable peer exists, the service fails with an actionable error naming both remediations: `enable --install-wsl yes` or `add a Linux peer to the cluster`.
- **L-3: Windows-aware Dockerfile translation.** `crates/zlayer-builder/src/buildah/mod.rs::BuildahCommand` is now OS-aware via a new `target_os: ImageOs` context. `RUN` shell-form instructions emit `cmd.exe /S /C <cmd>` on Windows targets (`/bin/sh -c` on Linux); `SHELL` overrides still apply to both; `WORKDIR` uses an OS-appropriate mkdir strategy. The same translator is reused by the Phase L-4 HCS builder backend.
- **L-6: OS-aware backend routing at build start.** `detect_backend(ImageOs)` now routes `ImageOs::Windows` on a Windows host straight to the Phase L-4 HCS backend (previously an error stub), `ImageOs::Linux` everywhere keeps going through buildah, and a Linux host receiving `ImageOs::Windows` returns an actionable error directing the user to a Windows node. This is the single dispatch point consumed by both `zlayer build` and the L-7 pipeline.
- **K-3: Composite dispatch E2E tests updated for Phase G / H-7.** Removed the "F-9 Unsupported is expected" workaround from `composite_dispatch_e2e.rs::composite_dispatches_linux_spec_to_wsl2` now that G-2 delivers real OCI bundle writes. `composite_falls_through_to_primary_when_no_platform_specified` rewritten (or retired) to reflect H-7's strict `RouteToPeer` dispatch policy. Tests that can run without real hardware had their `#[ignore]` attributes removed.
- **Documentation consolidation: Windows promoted to first-class platform in all public docs and CLI surface caught up.** `README.md`, `CLAUDE.md`, `RUNTIME_GUIDE.md`, `DEPLOYING_IMAGES.md`, and `TESTING.md` previously claimed Linux + macOS only despite Windows shipping HCS native runtime, WSL2 delegate, Wintun overlay, SCM service, and `install.ps1` in this release. `README.md` rewritten end-to-end: Windows installation (`irm https://zlayer.dev/install.ps1 | iex`), `zlayer-windows-amd64.zip` artifact, Windows requirements row, Windows column added to the Multi-Node Resource Detection table, HCS Runtime + WSL2 Delegate Runtime sections under Runtime Modes, and a full CLI Reference catch-up adding the ~25 subcommand families that had drifted out of the docs (`daemon install/start/stop/status`, `image`, `container`, `system`, `secret`, `env`, `network`, `variable`, `task`, `workflow`, `notifier`, `volume`, `auth`, `user`, `group`, `permission`, `audit`, `project`, `credential`, `sync`, `job`, `windows compact`, `docker install/uninstall`, `tui`, `completions`). Broken links to deleted `V1_SPEC.md` / `ACTION.md` / `MAC_FAILING.md` removed everywhere. Project Structure listings in `README.md`, `CLAUDE.md`, and `RUNTIME_GUIDE.md` now name every directory under `crates/` (8 previously-missing crates added: `zlayer-client`, `zlayer-consensus`, `zlayer-docker`, `zlayer-git`, `zlayer-hcs`, `zlayer-hns`, `zlayer-paths`, `zlayer-wsl`). `DEPLOYING_IMAGES.md` gained a Windows Image Building section (HCS backend, nanoserver/servercore templates, `os: windows` ZImagefile field, `--platform windows` build flag, mixed-OS pipeline waves). `TESTING.md` gained a Windows section (`cargo test --no-default-features --features hcs-runtime,wsl`, `scripts/windows-remote-check.sh`, link to `docs/windows-ci-runner.md`, table of Windows test files, elevated-shell note). `install.py` no longer rejects Windows — it now resolves `windows-amd64.zip`, extracts via `zipfile`, installs to `%LOCALAPPDATA%\ZLayer\bin`, and updates user PATH via `winreg` + `WM_SETTINGCHANGE` broadcast. The web frontend (`crates/zlayer-web/`) hero copy + feature cards refreshed to make Linux / macOS / Windows peers, and the `/docs` install section now shows both `install.sh` and `install.ps1` paths. Marketing site footer copyright bumped to 2026. New per-crate READMEs added for the 10 crates that previously lacked one: `zlayer-client`, `zlayer-docker`, `zlayer-git`, `zlayer-hcs`, `zlayer-hns`, `zlayer-paths`, `zlayer-secrets`, `zlayer-tui`, `zlayer-web`, `zlayer-wsl`. `crates/zlayer-agent/README.md` updated to remove stale "Windows/macOS: Uses Docker directly" claim and document the HCS / WSL2 delegate / composite dispatch model. Duplicate `crates/zlayer-web/zlayer_logo.png` at the crate root removed (the served copy lives in `assets/`).

### Removed
- **Thin Windows CLI build (`--features wsl` alone) removed from `.forgejo/workflows/build.yml`.** Phase F-7a already demoted `wsl` to a delegate runtime inside the HCS `CompositeRuntime` (not a standalone backend), and `bin/zlayer`'s source references `zlayer_agent::` unconditionally, so the thin build no longer compiled. The two Windows jobs (`build-windows-amd64` / `build-windows-amd64-hcs`) are now one job, `build-windows-amd64`, built with `--no-default-features --features hcs-runtime,wsl` (the combo `.forgejo/workflows/ci.yaml` already validates in `check-windows` / `test-windows`). The job runs the binary once as a smoke test (`zlayer.exe --version`), regenerates shell completions, runs `cargo test --package zlayer-hcs`, and packages `zlayer.exe` + `completions/` into `zlayer-windows-amd64.zip`. `zlayer-windows-amd64-hcs.exe` is also uploaded as a one-release compatibility alias (scheduled for removal after the next release); `scripts/check-existing-packages.sh`, `scripts/upload-packages.sh`, and `.forgejo/workflows/release.yml` all know about both names for now.

### Fixed
- **`zlayer-manager`: `test_service_summary_deserialize` now constructs valid JSON.** The `e0ad80f Add zlayer-types` + `71cdcbc Fold zlayer-spec` refactor relocated `ServiceSummary` from a local definition in `crates/zlayer-manager/src/api_client.rs` to the canonical `zlayer_types::api::services::ServiceSummary`, which carries a required `endpoints: Vec<ServiceEndpoint>` field with no `#[serde(default)]`. The hand-written test JSON literal at `crates/zlayer-manager/src/api_client.rs:1822-1830` was authored against the old local DTO and never included `endpoints`, so post-refactor the test panicked at runtime (`missing field 'endpoints' at line N column N`). Added `"endpoints": []` to the literal. Verified: the exact CI command `cargo test --package zlayer-manager --lib --features ssr` now passes 158/158 (was 157/158).
- **macOS package resolver: missing/unavailable Homebrew formulas now fail the build.** `crates/zlayer-builder/src/macos_image_resolver.rs::install_with_deps` previously logged a `warn!` and continued on resolve/install failure, producing a silently-broken sandbox where the requested package wasn't actually present. The function now distinguishes the **root** formula (the one supplied by the caller) from **transitive** dependencies discovered during the BFS: failure on the root → `Err(BuildError::RegistryError)`; failure on a transitive dep → `warn!` + skip (preserves resilience to optional deps). `crates/zlayer-builder/src/sandbox_builder.rs` was updated to propagate the error with the original Linux package name + distro in the message: `Linux package 'libssl-dev' resolved to Homebrew formula 'openssl@3' which is not available; check RepoSources mapping for debian_12`. The signature of `map_linux_packages` changed from `Vec<(String, bool)>` to `Vec<(String, String, bool)>` = `(linux_name, brew_formula, skipped)` so the linux name is available at the install site for error context.
- **macOS package resolver: hermetic seeded-cache test added.** A new `test_map_linux_packages_with_seeded_cache` writes a fixture `PackageMapFile` JSON to a tmpdir and asserts `map_linux_packages` returns the expected `(linux_name, brew, skipped)` triples deterministically — runs in CI, no network. This is the regression guard for the sharded layout. (The pre-existing `test_map_linux_packages_live_reposources` remains a manual smoke test; its `#[ignore]` gating lands in 0.11.5.)
- **macOS resolver → RepoSourceSyncer: HMAC-signed POST replaces shared-clock auth.** The fire-and-forget cache-warm POST to `https://reposync.blackleafdigital.com/formula` previously sent a `zlayer-repo-sync: <ISO-timestamp>` header and the server accepted any timestamp within ±5 minutes — anyone who could read a clock could forge the call. The resolver now signs the POST body with HMAC-SHA256 using a shared secret from `ZLAYER_REPOSYNC_HMAC_SECRET`, header `x-reposync-signature: sha256=<hex>`. If the env var is unset (developer machine, CI without the credential) the POST is skipped entirely with a `debug!` — the resolver still works, only the upstream cache warm is a no-op. The matching server-side validator change is in `BlackLeafDigital/functions/RepoSourceSyncer/src/helpers.ts` (constant-time compare via `crypto.timingSafeEqual`). Removed the now-unused `utc_iso8601` helper. Files: `crates/zlayer-builder/src/macos_image_resolver.rs`, `crates/zlayer-builder/Cargo.toml` (`hmac` dep added).
- **Windows build: `zlayer-builder` HCS backend uses `oci-client` types via `zlayer-registry` re-exports.** `crates/zlayer-builder/src/backend/hcs/scratch.rs` previously imported `oci_client::secrets::RegistryAuth` (line 22) and `oci_client::manifest::OciImageManifest` (line 208) directly, but `oci-client` is declared `optional = true` in `crates/zlayer-builder/Cargo.toml` and only enabled by the `cache` feature. The HCS backend is Windows-only and does not require `cache`, so default-feature Windows builds failed with `unresolved module or unlinked crate oci_client` — every release/CI Windows job was broken. `crates/zlayer-registry/src/lib.rs` now re-exports `oci_client::manifest::OciImageManifest` next to the existing `RegistryAuth` re-export, and `scratch.rs` imports both via `zlayer_registry::{ImagePuller, OciImageManifest, RegistryAuth}`. No `Cargo.toml` change — the `cache` gating is preserved for non-Windows builds (sandbox-push code path), and the Windows build picks up `oci-client` transitively through `zlayer-registry`'s unconditional dep.
- CI: `gix` workspace dep bumped `0.81` → `0.82` in `Cargo.toml:249`. Pre-emptive: `gix-actor 0.40.1` (released Apr 2026) silently switched its `winnow` dep from `0.7` to `1.0` without bumping its own minor, leaving it incompatible with `gix-object 0.58.0` (still on `winnow 0.7`). Our lockfile currently pins `gix-actor 0.40.0` so today's CI is green, but any `cargo update` would re-pull 0.40.1 and break all three check jobs (Linux/macOS/Windows) with three `E0308` errors in `gix-object/src/parse.rs` complaining `expected ErrMode<E>, found winnow::error::ErrMode<_>`. Bumping `gix` to `0.82` pulls the new `gix-object 0.59.0` that natively uses `winnow 1.0` and matches `gix-actor 0.40.1`. `cargo check --workspace --all-targets` clean.
- CI: `cargo clippy` on macOS now passes on `crates/zlayer-overlay/src/interface/macos.rs`. `v4_netmask`'s single-character locals (`p`, `a`–`d`) tripped `clippy::many_single_char_names`; renamed to `bits` / `octet0`–`octet3`. The `prefixlen` binding inside `add_address`'s IPv6 branch tripped `clippy::similar_names` against the `prefix_len` parameter one scope up; renamed to `prefix_str`. Fix is source-level only — no behavioral change.
- CI: Windows clippy on `crates/zlayer-builder/tests/windows_build_e2e.rs` now passes. `clippy::doc_markdown` flagged `BackupRead` and `diff_id` in the module-level doc comment; both are now fenced with backticks.
- CI: Added `Install Git Bash` step after `Setup Rust` in `check-windows` (and the `test-windows` job should get the same once `shell: bash` is needed there) in `.forgejo/workflows/ci.yaml`. Without it, `shell: bash` on the MiniWindows LocalSystem-run runner resolved to `wsl.exe bash` and failed with `WSL_E_LOCAL_SYSTEM_NOT_SUPPORTED` — mirrors the Git-Bash install already present in `build.yml`'s Windows jobs.
- **CI: `build.yml::build-windows-amd64`'s `Test zlayer-hcs on Windows` step no longer aborts the runner with exit 255.** The `open_nonexistent_process_errors_cleanly` integration test at `crates/zlayer-hcs/tests/integration.rs:241` passes a zeroed `HCS_SYSTEM` null handle to `HcsOpenProcess` and expects a clean HRESULT — which works fine in interactive user sessions but crashes the test process via SEH under the Forgejo runner's LocalSystem / services-session-0 account. Added a `is_session_zero()` / `skip_if_session_zero()` helper next to the existing elevation probe in the same file (uses `ProcessIdToSessionId` from `windows::Win32::System::RemoteDesktop`, which required adding `"Win32_System_RemoteDesktop"` to the HCS crate's windows-feature list) and gated the null-handle test on it. Interactive user sessions still run the test; service-session CI reports `skipping … — running in Windows services session 0` and proceeds cleanly. Also added `cargo test --package zlayer-hcs --no-fail-fast` to `scripts/windows-remote-check.sh` so the local MiniWindows pre-push check now mirrors what `build.yml:333` actually runs — previously the script only exercised `--workspace --lib`, which excludes integration-test binaries, leaving a scope gap between local "green" and the CI failure.
- **CI: MiniWindows speedup — sccache + registry cache + rust-lld + Defender exclusions.** The MiniWindows runner took ~7m49s for cold `cargo check --workspace` vs ~12s warm (39× gap). Root causes were compounding: (a) the `check-windows` / `test-windows` jobs skipped `s3-cache` entirely (registry/git cache never populated between runs); (b) `setup-rust@main`'s sccache support was available but never wired (`sccache: true` + S3 inputs); (c) MSVC `link.exe` is the slow default on a 1,089-crate workspace; (d) Windows Defender real-time-scanned every `.rlib` / `.exe` / `.pdb` rustc wrote. Changes: turned on `sccache: true` with the same S3 secrets the Linux `s3-cache` uses in `.forgejo/workflows/ci.yaml::check-windows`, `ci.yaml::test-windows`, `.forgejo/workflows/build.yml::build-windows-amd64`; added `Restore cache` / `Save cache` steps to `check-windows` and `test-windows` (matching the Linux `check` job); added `[target.x86_64-pc-windows-msvc] linker = "rust-lld"` to `.cargo/config.toml` so the bundled `rust-lld` replaces `link.exe` (no extra component install — ships with the pinned toolchain); documented mandatory Windows Defender exclusions on `C:\ProgramData\forgejorunner\workdir`, `C:\src\ZLayer`, `%USERPROFILE%\.cargo`, `%USERPROFILE%\.rustup` as step 5b of the replication runbook in `docs/windows-ci-runner.md`. Not using cargo-chef: it's a Docker-layer caching pattern; sccache is the Rust-native equivalent and this CI isn't layered Docker.
- **`install.ps1`: removed dead `zlayer-linux` binary fetch; added proactive WSL2 install.** Pre-F-7a the installer staged a Linux `zlayer` binary in `$LOCALAPPDATA\ZLayer\bin\zlayer-linux` for when the Windows CLI proxied commands to a zlayer daemon inside WSL2. After F-7a / Phase G, the `Wsl2DelegateRuntime` invokes `youki` (a separate binary that lives inside the distro) via `wsl.exe -d <distro> -- youki …` — nothing in the runtime ever references `zlayer-linux`. Stripped the 30-line "Download WSL2 support files" block. In its place, the installer now probes `wsl.exe --status`; when WSL2 is absent, it fires `Start-Process wsl.exe -ArgumentList --install,--no-distribution -Verb RunAs -Wait` (UAC is the consent gate). Declined UAC or a non-zero exit prints an actionable retry command and moves on without failing the install — Windows-native containers (HCS) still work without WSL2.
- **CI: dropped redundant "default features" Windows runs.** `.forgejo/workflows/ci.yaml::check-windows` ran `cargo check` and `cargo clippy` twice on Windows — once on default features and again on `hcs-runtime,wsl`; `test-windows` likewise ran `cargo test --workspace --lib` twice. `build.yml:326` always ships `--no-default-features --features hcs-runtime,wsl` (thin-CLI Windows was retired in F-7a), so the default-features runs were validating a build no downstream consumer uses and doubling MiniWindows-runner time. Dropped the default-features steps from both jobs and from `scripts/windows-remote-check.sh`; only the composite `hcs-runtime,wsl` runs remain.
- **CI: `build.yml::build-linux-arm64` no longer fails at apt install.** The `runs-on: arm64` self-hosted label is a macOS arm64 host, and the job had no `container:` key, so the `setup-system-deps` action hit `apt-get is only available on Linux`. Added `container: node:20-bullseye` to the job (multi-arch manifest list — resolves to the native arm64 variant on the arm64 host, no cross-build — matches the in-repo `publish-sdks.yml:110-111` precedent that already pairs `runs-on: arm64` with a Linux container, and mirrors `build-linux-amd64` in the same file). Also aligned the apt package list with the amd64 job (`protobuf-compiler libseccomp-dev cmake build-essential pkg-config libssl-dev clang git`) so the container has the full toolchain both jobs need.
- **Release pipeline unblocked — three publish-time bugs.** (1) `crates/zlayer-agent/Cargo.toml` was `publish = false`, which made `cargo-publish-workspace` filter the crate out of the topological publish order. But `zlayer-builder` has a `cfg(target_os = "windows")` dep on `zlayer-agent`, and cargo validates target-gated deps against the registry at package time regardless of host — so `publish-all-crates` died on `zlayer-builder` with `no matching package named zlayer-agent found`. The comment block on zlayer-agent (re: `youki-runtime` being off-by-default so the crate publishes cleanly) had always intended for it to publish; the `publish = false` line was a stale leftover. Removed — letting it default to unrestricted. (2) `publish-py-binding-build-linux-{x86_64,aarch64}` in `.forgejo/workflows/publish-sdks.yml` ran inside stock `quay.io/pypa/manylinux_2_28_*` containers which lack `node`, so `actions/checkout@v4` (a `node20` JS action) failed before any step ran. Added a `Bootstrap Node.js for actions (manylinux)` step as the first step of each job that installs node via the NodeSource RPM repo (`curl -fsSL https://rpm.nodesource.com/setup_20.x | bash - && yum install -y nodejs`); a bare `run:` step doesn't need node, so the bootstrap works on a node-less container and every subsequent JS action then works. Pattern lifted from `Blazen/.forgejo/workflows/build-artifacts.yaml:76-92`. (3) `crates/zlayer-api/Cargo.toml` declared `readme = "README.md"` but the file didn't exist, which made `uvx maturin sdist` (run from `zlayer-py`) fail with `readme path ... does not exist or is invalid` while walking the workspace dep graph. Created `crates/zlayer-api/README.md` with a short description and links to the public CLI and generated clients. Same README mirrored into the proprietary fork (`zlayer-zql/crates/zlayer-api/README.md`) since the sdist build would hit the same failure once that pipeline reaches the step.
- **Release pipeline unblocked — `zlayer-proxy` / `zlayer-secrets` `publish = false` + Windows wheel `maturin: command not found`.** Two follow-ups to the prior publish-time fix bullet. (1) `crates/zlayer-proxy/Cargo.toml` and `crates/zlayer-secrets/Cargo.toml` were both `publish = false`, but `crates/zlayer-agent/Cargo.toml` declares versioned workspace deps on both (`zlayer-proxy.workspace = true`, `zlayer-secrets.workspace = true`). `cargo-publish-workspace` filtered both crates out of the topo order, so when the agent's own `cargo publish` ran it died with `no matching package named zlayer-proxy found` (and the same trap was waiting for `zlayer-secrets`). Removed the `publish = false` line from each — defaults to publishable, both crates already carry the metadata crates.io needs (name, version, description, license, repository). The Kahn topo-sort in the action now publishes proxy and secrets ahead of agent. (2) `.forgejo/workflows/publish-sdks.yml::publish-py-binding-build-windows-x86_64` was the lone wheel-build job still using the `uv tool install maturin` + bare `maturin build` two-step pattern — `uv tool install` drops the binary in `%USERPROFILE%\.local\bin`, which is not on PATH in the next `shell: bash` step, so the build died with `maturin: command not found` (exit 127). Replaced with a single `uvx maturin build --release --out dist` step matching the linux-x86_64, linux-aarch64, macos-x86_64, macos-aarch64, and sdist jobs already in this file. `uv` itself is provisioned on the MiniWindows runner via the chocolatey safety net in `ci.yaml::check-windows`, so no additional setup is needed here. Mirror: same `uvx maturin` rewrite applied to `zlayer-zql/.forgejo/workflows/publish-sdks.yml` in three jobs (macos-x86_64, macos-aarch64, windows-x86_64) for consistency, even though the macOS jobs may have intermittently worked depending on PATH.
- **OpenAPI Go client now publishable from a release.** `clients/go/` was generated with module path `github.com/BlackLeafDigital/zlayer-go` — that path is a 404 on GitHub, so `go get` could not resolve it. Every Go consumer (notably ZArcRunner) was masking this with a local `replace github.com/BlackLeafDigital/zlayer-go => ../ZLayer/clients/go` directive that never survives a container build. Fix mirrors the existing `publish-plugin-sdk-go` pattern at `.forgejo/workflows/publish-sdks.yml:653`: renamed the module to `github.com/BlackLeafDigital/ZLayer/clients/go` (`clients/go/go.mod`, the 34 `clients/go/test/api_*_test.go` import lines, the user-facing examples in `clients/README.md`, and the generated `clients/go/README.md` placeholder + per-API `clients/go/docs/*.md` `GIT_USER_ID/GIT_REPO_ID` placeholders); patched `scripts/generate-clients.sh::generate_go` so future regenerations emit the correct module path AND pass `gitUserId=BlackLeafDigital,gitRepoId=ZLayer/clients/go` so the README import snippet stays correct; switched `packageVersion` from a hardcoded `0.1.0` (already drifting — repo is on 0.10.x) to a `0.0.0-dev` sentinel that the new publish job stamps to the real version at tag time. New `publish-openapi-go` job in `.forgejo/workflows/publish-sdks.yml` (immediately after `publish-plugin-sdk-go`) seds `0.0.0-dev` → `inputs.version` across `clients/go/**.{go,md}`, runs `go vet` on the stamped tree, makes a one-off detached-HEAD commit, tags `clients/go/v<VERSION>` against that commit, pushes only the tag (Forgejo + GitHub mirror via `GH_ACCESS_TOKEN` with the same `*zdb*` safety guard as `publish-plugin-sdk-go`); the branch never moves and the dev sentinel stays in tree. Registered as `sdks/openapi-go` in `.forgejo/build-config.yaml` (no `registry:` block — same as `plugin-sdk-go`, idempotency via `ci-step-status` markers). Deleted `clients/go/git_push.sh` (a generator-emitted helper for pushing a generated client to its own standalone git repo — meaningless for a monorepo subpath publish) and added it to `clients/go/.openapi-generator-ignore` so it doesn't come back. Consumer migration in ZArcRunner is a separate follow-up: drop the local `replace`, pin `github.com/BlackLeafDigital/ZLayer/clients/go v<VERSION>`, project-wide rename `import "github.com/BlackLeafDigital/zlayer-go"` → `import "github.com/BlackLeafDigital/ZLayer/clients/go"`. Mirror: `zlayer-zql/scripts/generate-clients.sh::generate_go` updated identically but with module path `forge.blackleafdigital.com/BlackLeafDigital/zlayer-zql/clients/zdb-go` and no GitHub-mirror push (zlayer-zql is Forgejo-only by policy; no OpenAPI Go client is currently committed there, so no workflow / build-config changes needed yet — the script fix prevents drift when one is.)
- **`zlayer-types` refactor follow-up: Linux/Windows runtime callsites now stringify `ImageSpec.name`.** The `e0ad80f Add zlayer-types` + `71cdcbc Fold zlayer-spec` refactor changed `ImageSpec.name: String` → `ImageReference` (re-export of `oci_client::Reference`) but only updated callsites that compile on macOS, where the refactor was performed. Platform-gated runtime code (`crates/zlayer-agent/src/runtimes/youki.rs` on Linux, `hcs.rs` and `wsl2_delegate.rs` on Windows) and the cross-platform `crates/zlayer-agent/tests/composite_dispatch_e2e.rs` integration test still passed `&spec.image.name` (now `&ImageReference`) to functions and `HashMap<String, _>` lookups that take `&str` — every Linux and Windows CI job failed with `expected &str, found &Reference` / `the trait Borrow<Reference> is not implemented for String`. Fix introduces a single `let image_name = spec.image.name.to_string();` local at the top of each affected function (matching the pattern already used by `macos_sandbox.rs:1622`, `wasm.rs:1035`, `service.rs:119`, `composite.rs:190`) and rewrites the in-function callsites to take `&image_name`. The `Runtime::pull_image(&self, image: &str)` trait contract and the `images: HashMap<String, CachedImage>` cache key types are unchanged. The README example in `crates/zlayer-agent/README.md` is updated to match. Verified against the live MiniWindows host via `scripts/windows-remote-check.sh`.
- **Windows build + clippy iteration against MiniWindows (rounds 1–5).** Resolved the Windows `Send`/`Sync`, `cfg`-gate, export-visibility, dependency-placement, and clippy issues surfaced by the `scripts/windows-remote-check.sh` validation loop across five rounds. Round 1: `unsafe impl Sync` added to `OwnedSystem` / `OwnedOperation` / `OwnedProcess` in `crates/zlayer-hcs/src/handle.rs` and `OwnedNetwork` / `OwnedEndpoint` / `OwnedNamespace` in `crates/zlayer-hns/src/handle.rs` (handles are documented thread-safe; the `Send`-only posture was breaking 56 `*mut c_void cannot be shared/sent between threads safely` errors once async paths held them across `.await`); `crates/zlayer-agent/src/bundle.rs` split `get_device_major_minor` / `get_device_type` cleanly via `#[cfg(unix)]` with a Windows `Unsupported` stub, and the rootfs symlink at line 371 now picks `tokio::fs::symlink` (Unix) vs `tokio::fs::symlink_dir` (Windows — `CreateSymbolicLinkW` with the directory flag); `crates/zlayer-agent/src/runtimes/hcs.rs::inspect_detailed` restructured to clone the `Arc<RwLock<Option<i32>>>` out of the container entry before awaiting, eliminating an E0597 borrow-across-await; `crates/zlayer-agent/src/overlay_manager.rs::attach_to_interface` and the `std::os::fd::AsFd` import are now `#[cfg(target_os = "linux")]`-gated, and the non-Linux netlink stubs in `crates/zlayer-agent/src/netlink.rs` restricted to Unix-non-Linux (macOS) so the `OwnedFd` / `BorrowedFd` signatures no longer break Windows. Round 2: introduced `SendHandle<T>(pub T)` in `crates/zlayer-hcs/src/handle.rs` — a `#[repr(transparent)]` `Copy` newtype with `unsafe impl Send + Sync` and `Deref<Target = T>`, re-exported from `lib.rs` — because async bodies still held RAW `HCS_SYSTEM` / `HCS_OPERATION` / `HCS_PROCESS` (which are `!Send + !Sync` and blocked by the orphan rule) across `.await`; async paths in `operation.rs`, `system.rs`, `process.rs`, `enumerate.rs`, and `handle.rs` refactored to wrap locals in `SendHandle`. Round 3: `OwnedSystem::as_raw()` / `OwnedProcess::as_raw()` / `OwnedOperation::as_raw()` and the `ComputeSystem::raw()` / `ComputeProcess::raw()` pass-throughs now return `SendHandle<T>` at source — Rust 2021 disjoint-field captures meant `handle.0` inside a `move` closure captured only the inner `!Send` field and defeated the wrapper; closures now deref via `*handle` (forcing whole-variable capture) and FFI call sites deref with `*handle` when passing to the `Hcs*` C APIs, clearing the 14 remaining E0277 `Send` and 2 `Sync` errors. Round 4: ungated 17 top-level `Commands::*` variants in `bin/zlayer/src/cli.rs` (`Project`, `Env`, `Task`, `Workflow`, `User`, `Sync`, `Notifier`, `Variable`, `Group`, `Auth`, `RegistryCredential`, `Permission`, `GitCredential`, `Credential`, `Webhook`, `Audit`, `GroupMember`) plus their sub-enums and `CliBuildKind` / `CliUserRole` helpers — every handler dispatches over cross-platform HTTP so the `#[cfg(unix)]` gates were wrong; replaced the `_assert_signature` helper in `bin/zlayer/src/commands/join.rs:805` (its `impl Future<Output = Result<()>>` bound conflicted with `join`'s HRTB `for<'a> fn(&'a Cli, ...) -> _`) with a direct fn-pointer let-binding; promoted `zlayer_paths::is_root` to `pub` at crate root with Unix/Windows/fallback bodies (unblocking 5 `bin/zlayer` call sites) and declared `pub mod firewall;` in `zlayer-overlay/src/lib.rs` (unblocking 3 call sites) — both landed in H-5/H-6 but their exports were unreachable on Windows; moved `zlayer-init-actions` and `zlayer-docker` out of `[target.'cfg(unix)'.dependencies]` in `bin/zlayer/Cargo.toml` into top-level `[dependencies]` (both are cross-platform — init-actions uses only reqwest/tokio, docker only gates its `socket/` module on `cfg(unix)`) so the Windows build resolves them; added the missing `Win32_NetworkManagement_WindowsFirewall` and `Win32_System_Com` feature flags on the `zlayer-overlay` `windows` dep so H-6's `INetFwPolicy2` COM bindings link on Windows. Round 5: re-applied the G-6 WSL2 consent API (`ensure_wsl_backend_ready_with_consent` and `WslError::{InstallRefused, RebootRequired}`) that an earlier parallel-agent race had lost — consumer code in `node.rs` / `join.rs` already expected this API. Finally, two clippy sweeps: scoped `#[allow(unsafe_code)]` at the crate/module level on `zlayer-hcs`, `zlayer-hns`, and the Windows-only modules in `zlayer-overlay` (they exist specifically to wrap unsafe HCS/HCN/Wintun APIs, while the workspace-wide `-W unsafe-code` policy remains in force everywhere else) and fixed 7 `doc_markdown`, 1 `single_match_else`, and 1 `borrow_as_ptr` regression; cleaned up stylistic lints in `bin/zlayer` (3 × `used_underscore_binding`, 2 × `no_effect_underscore_binding`, 3 × `needless_return` in `views/dashboard.rs`, 1 × `doc_markdown`). `cargo clippy --workspace --all-targets -- -D warnings` is now clean on MiniWindows.

## [0.10.86]

### Fixed
- CI: `publish-sdks.yml` no longer cross-compiles. Linux x86_64 and aarch64 wheels build inside the stock upstream `quay.io/pypa/manylinux_2_28_*` containers — the aarch64 slot on a native `runs-on: arm64` runner. No zigbuild, no `dpkg --add-architecture`, no apt `:arm64` packages. libseccomp is satisfied by `libseccomp-devel` from yum inside the container. All maturin/twine invocations migrated to `uvx` to drop the intermediate `uv tool install` step.
- CI: `publish-plugin-sdk-c` now installs the WASI SDK via the new `setup-wasi-sdk@main` shared action and passes `CMAKE_TOOLCHAIN_FILE=$WASI_SDK_PREFIX/share/cmake/wasi-sdk.cmake`. The C SDK targets `wasm32-wasip2`, so the host-compiler build was never going to succeed.
- CI: Go SDK module path in `clients/zlayer-sdk/go/go.mod` now matches the consumer-facing `github.com/BlackLeafDigital/ZLayer/clients/zlayer-sdk/go` URL. All internal imports and README examples updated.
- CI: `plugin-sdk-rust` registry URL in `.forgejo/build-config.yaml` now points at `https://crates.io` (it was still set to the private Forgejo cargo registry while the workflow actually publishes to crates.io).

### Changed
- CI: `release.yml` now explicitly pushes `main` and the triggering release tag to the GitHub mirror from inside the `merge-to-main` job. The Forgejo→GitHub push mirror was disabled to stop proprietary refs from syncing; the workflow owns the GitHub state directly now. Each push uses a full literal refspec (never `--mirror`/`--all`/`refs/*:refs/*`) with a tag allowlist and a `zdb`-path guard.
- CI: `publish-sdks.yml` push of the Go SDK subpath tag is now explicit to the GitHub remote (same narrow-refspec pattern) so `go get` from github.com continues to resolve.

### Removed
- Root docs: deleted `ACTION.md`, `V1_SPEC.md`, `ZLAYER_SECRETS_MANAGER.md`, and `WORKFLOWS.md` (stale planning notes).
- `GPU_ROADMAP.md` trimmed to items that aren't already implemented (CDI, Apple Silicon, GPU metrics, vendor env-var injection, gang scheduling are all live and no longer listed as future work).
- `MAC_MPS_RUNTIME.md` rewritten as a concise reference for the existing `SandboxRuntime` + `VmRuntime` backends, replacing the 3,700-line design doc.

### Added
- **Phase F: Composite Windows runtime + Windows-native CLI.** The native Windows daemon now joins the cluster as a single peer holding a `CompositeRuntime` (`crates/zlayer-agent/src/runtimes/composite.rs`) that dispatches per-container between Phase E's `HcsRuntime` (Windows base images) and a new `Wsl2DelegateRuntime` (Linux base images, executed by shelling out to `youki` inside the WSL2 distro via `zlayer_wsl::distro::wsl_exec` — no in-distro daemon, no HTTP helper). Dispatch reads `ServiceSpec.platform.os` first, then falls back to image-manifest `config.os` inspection at pull time (cached per image ref). Same composite pattern extended to macOS (`SandboxRuntime` + `VmRuntime`). `zlayer serve` on Windows now boots the native daemon (no longer launches a daemon inside WSL2). New `.wslconfig` merge for `networkingMode = "mirrored"` so Linux containers in the distro share the host's network stack — required for the Windows host's Wintun/boringtun overlay (Phase E) to reach Linux containers transparently. Windows `DaemonClient` ungated: `zlayer-client::DaemonClient` now compiles + connects on Windows via TCP loopback (`127.0.0.1:3669`), reading the local-admin bearer from a new `zlayer_paths::default_admin_bearer_path()` file persisted by `zlayer-api::server::serve_bound`. All 27 CLI command modules (`deploy`, `ps`, `logs`, `exec`, `node`, `tunnel`, `secret`, `image`, `container`, etc.) are now cross-platform; `lifecycle::stop` and `commands::daemon` (start/stop/status/restart) gained Windows variants that route through `DaemonClient` instead of direct-agent calls. `node init`, `node join`, and `zlayer join` remain Unix-only (overlay setup + GPU detect) but emit actionable error messages on Windows pointing users at a Linux peer for cluster bootstrap. New `check-windows` job in `.forgejo/workflows/ci.yaml` (parallel to `check-macos`) compiles + clippy-checks both feature combinations on every PR — closes the gap where Windows-gated code only hit the release-chain builds. New Windows-gated integration tests at `crates/zlayer-agent/tests/composite_dispatch_e2e.rs` (5 `#[ignore]` tests) exercise the real composite path with HCS + WSL2 youki on a real Windows host. Files: `crates/zlayer-agent/src/runtimes/{composite,wsl2_delegate}.rs` (new), `crates/zlayer-agent/src/{lib,runtimes/mod}.rs`, `crates/zlayer-agent/Cargo.toml`, `crates/zlayer-wsl/src/wslconfig.rs`, `crates/zlayer-client/{Cargo.toml,src/lib.rs,src/daemon_client.rs}`, `crates/zlayer-paths/src/lib.rs`, `crates/zlayer-api/{Cargo.toml,src/server.rs}`, `crates/zlayer-overlay/src/health.rs`, `crates/zlayer-registry/src/{client,image_config}.rs`, `crates/zlayer-spec/src/types.rs`, `bin/zlayer/{Cargo.toml,src/main.rs,src/daemon.rs,src/commands/{mod,serve,lifecycle,daemon,node,join}.rs}`, `.forgejo/workflows/ci.yaml`.
- **Phase F: composite Windows runtime + Windows-native CLI.** The native Windows daemon now joins the cluster as a single peer holding a `CompositeRuntime` (`crates/zlayer-agent/src/runtimes/composite.rs`) that dispatches per-container between Phase E's `HcsRuntime` (Windows base images) and a new `Wsl2DelegateRuntime` (Linux base images, executed by shelling out to `youki` inside the WSL2 distro via `zlayer_wsl::distro::wsl_exec` — no in-distro daemon, no HTTP helper). Dispatch reads `ServiceSpec.platform.os` first, then falls back to image-manifest `config.os` inspection at pull time (cached per image ref). Same composite pattern extended to macOS (`SandboxRuntime` + `VmRuntime`). `zlayer serve` on Windows now boots the native daemon (no longer launches a daemon inside WSL2). New `.wslconfig` merge for `networkingMode = "mirrored"` so Linux containers in the distro share the host's network stack — required for the Windows host's Wintun/boringtun overlay (Phase E) to reach Linux containers transparently. Windows `DaemonClient` ungated: `zlayer-client::DaemonClient` now compiles + connects on Windows via TCP loopback (`127.0.0.1:3669`), reading the local-admin bearer from a new `zlayer_paths::default_admin_bearer_path()` file persisted by `zlayer-api::server::serve_bound`. All 27 CLI command modules (`deploy`, `ps`, `logs`, `exec`, `node`, `tunnel`, `secret`, `image`, `container`, etc.) are now cross-platform; `lifecycle::stop` and `commands::daemon` (start/stop/status/restart) gained Windows variants that route through `DaemonClient` instead of direct-agent calls. `node init`, `node join`, and `zlayer join` remain Unix-only (overlay setup + GPU detect) but emit actionable error messages on Windows pointing users at a Linux peer for cluster bootstrap. New `check-windows` job in `.forgejo/workflows/ci.yaml` (parallel to `check-macos`) compiles + clippy-checks both feature combinations on every PR — closes the gap where Windows-gated code only hit the release-chain builds. New Windows-gated integration tests at `crates/zlayer-agent/tests/composite_dispatch_e2e.rs` (5 `#[ignore]` tests) exercise the real composite path with HCS + WSL2 youki on a real Windows host. Files: `crates/zlayer-agent/src/runtimes/{composite,wsl2_delegate}.rs` (new), `crates/zlayer-agent/src/{lib,runtimes/mod}.rs`, `crates/zlayer-agent/Cargo.toml`, `crates/zlayer-wsl/src/wslconfig.rs`, `crates/zlayer-client/{Cargo.toml,src/lib.rs,src/daemon_client.rs}`, `crates/zlayer-paths/src/lib.rs`, `crates/zlayer-api/{Cargo.toml,src/server.rs}`, `crates/zlayer-overlay/src/health.rs`, `crates/zlayer-registry/src/{client,image_config}.rs`, `crates/zlayer-spec/src/types.rs`, `bin/zlayer/{Cargo.toml,src/main.rs,src/daemon.rs,src/commands/{mod,serve,lifecycle,daemon,node,join}.rs}`, `.forgejo/workflows/ci.yaml`.
- **Phase F-7b Batch B: persist the local-admin bearer token to disk.** The minted local-admin JWT that `bind_dual_with_local_auth` already produces is now written to `<data_dir>/admin_bearer.token` (`%ProgramData%\ZLayer\admin_bearer.token` on Windows, `/var/lib/zlayer/admin_bearer.token` on Linux as root, `~/.zlayer/admin_bearer.token` otherwise) the moment `serve_bound` starts — so a local `DaemonClient` can read it back and inject it into the `Authorization` header. On Windows this is load-bearing: the TCP loopback listener has no peer-credential bypass and Batch A's `read_local_bearer()` in `zlayer-client::daemon_client` — which the Windows `DaemonClient::{try_connect, try_connect_to, connect_to}` already call — was returning `None` pending this batch. On Linux/macOS it is informational (the UDS middleware still auto-injects into Unix-socket requests), but the file exists so other tooling can authenticate against the loopback listener without touching the socket. The stored value is the raw JWT — the `Bearer ` prefix is stripped on write — so consumers can build their own header string. Unix persistence runs an extra `chmod 0o600` right after the write (SYSTEM+Administrators ACL inheritance is enough on Windows). Failures at every step (`create_dir_all`, `write`, `set_permissions`) are logged via `tracing::warn!` and are **not** fatal — dev environments without write permission to `%ProgramData%` or `/var/lib` still get a working daemon, just without the persisted token. New public helpers in `zlayer-paths`: `ZLayerDirs::admin_bearer_path(&self) -> PathBuf` and free fn `default_admin_bearer_path() -> PathBuf`. New private helper in `zlayer-api::server`: `persist_admin_bearer(&str)` called from both the Unix and Windows `serve_bound` paths. Batch A's `TODO(F-7b Batch B)` stub in `zlayer-client::daemon_client::read_local_bearer` is filled in: it now reads + trims the file, returning `None` on missing/empty/unreadable (permission-denied, typical dev failure mode) so the existing fall-through to the daemon's default auth path is preserved. `zlayer-paths` grew two unit tests (`admin_bearer_path_is_under_data_dir`, `default_admin_bearer_path_matches_system_default`). `zlayer-api` `Cargo.toml` gained a new workspace dep on `zlayer-paths` (previously not wired through this crate). Linux workspace `cargo check` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo test -p zlayer-paths --lib admin_bearer` / `cargo test -p zlayer-api --lib` (625 tests) / `cargo test -p zlayer-client --lib` (16 tests) all green. Files: `crates/zlayer-paths/src/lib.rs`, `crates/zlayer-api/src/server.rs`, `crates/zlayer-api/Cargo.toml`, `crates/zlayer-client/src/daemon_client.rs`.
- **Phase F-7a: `zlayer serve` runs the native Windows daemon.** `commands::serve::serve()` and `daemon::init_daemon()` now compile and run on Windows, replacing the previous WSL2-daemon proxy. On Windows the daemon binds a TCP loopback listener (`tcp://127.0.0.1:3669`, per `ZLayerDirs::default_socket_path()`) instead of a Unix domain socket; Linux and macOS keep the existing dual TCP+UDS path byte-identical. Daemonization is a foreground no-op on Windows (operators wrap `zlayer.exe serve` in a service or `Start-Process` — `nix::unistd::daemon` / `fork` is Unix-only). The `wsl` feature no longer gates `serve`: it now only turns on the WSL2 delegate runtime inside the HCS `CompositeRuntime` from Phase F-6. Unix-only code paths are surgically cfg-gated: the stale-daemon `libc::kill` SIGTERM/SIGKILL escalation, the `find_udp_port_holder` / pgrep-style WG-port holder kill, the `process_alive(pid)` probe, the stale Unix-socket `remove_file` cleanup, `tokio::signal::unix::SignalKind::terminate`, and the systemd `sd_notify(READY=1)` call all live under `#[cfg(unix)]`; on Windows the daemon relies on Ctrl+C for shutdown and surfaces a clear "address already in use" error instead of nuking processes. `NodeConfig` + its load/save/detect helpers (`current_timestamp`, `detect_local_ip`, `generate_node_id`, `load_node_config`, `save_node_config`) moved from `commands::node` (Unix-only) into `crate::daemon` so the daemon init path can reach them on every platform; `commands::node` re-exports them for its Unix CLI handlers — no public-facing signature changes. New Windows-only `bind_dual_with_local_auth` / `serve_bound` / `BoundListeners` variants in `zlayer-api::server` bind TCP only and still mint the local admin JWT bearer (reserved for Phase F-7b's Windows transport) — the Unix versions are unchanged, including the Unix-socket auth-injection middleware. The workspace `mingw-w64`-gated cross-compile errors on `aws-lc-sys`/`ring`/`libsqlite3-sys`/`lzma-sys`/`bzip2-sys`/`zstd-sys` are environmental and explicitly out of scope; the only pre-existing Rust-level Windows defect surfaced during verification is `crates/zlayer-overlay/src/health.rs:399` calling `tokio::net::UnixStream` without a `cfg(unix)` gate (left as-is — outside Phase F-7a scope). Cross-platform daemon deps (`zlayer-api`, `zlayer-overlay`, `zlayer-proxy`, `zlayer-scheduler`, `zlayer-secrets`, `zlayer-storage`, `secrecy`) moved from `cfg(unix)` to `[dependencies]` so the Windows build can reach them. The catch-all match arm for non-Unix CLI commands now points at Phase F-7b (`DaemonClient` + CLI ungating) instead of the legacy WSL messaging. Linux workspace build + `cargo test --workspace --lib` + `cargo clippy --workspace --all-targets -- -D warnings` all clean. Files: `bin/zlayer/src/main.rs`, `bin/zlayer/src/daemon.rs`, `bin/zlayer/src/commands/mod.rs`, `bin/zlayer/src/commands/serve.rs`, `bin/zlayer/src/commands/node.rs`, `bin/zlayer/Cargo.toml`, `crates/zlayer-api/src/server.rs`, `crates/zlayer-api/src/lib.rs`.
- **Phase E: first-class Windows container overlay IPs (HCN Transparent).** Windows containers now get real cluster-routable overlay IPs instead of being NAT'd behind the host. Full Linux parity: a Windows container's `ipconfig` shows a `10.200.x.y` address, and cross-OS traffic preserves the container's source IP end-to-end. Built on five new primitives in `zlayer-overlay` / `zlayer-hns` (T1–T5): (1) `NodeSliceAllocator` that carves the cluster CIDR into deterministic per-node `/28` slices, (2) typed `EndpointPolicy` schemas (`OutBoundNatPolicySetting`, `SdnRoutePolicySetting`, `AclPolicySetting`) matching hcsshim's wire format byte-for-byte, (3) `find_primary_adapter()` via `GetAdaptersAddresses(AF_INET, GAA_FLAG_INCLUDE_GATEWAYS)` with `ZLAYER_HCN_UPLINK_ADAPTER` env override, (4) `Network::create_transparent()` that binds a Transparent HCN network to a physical uplink with a `/28` IPAM subnet and a `NetAdapterName` policy, and (5) `EndpointAttachment::create_overlay()` that creates an endpoint with a caller-chosen IP plus the three per-endpoint policies (`OutBoundNAT{Exceptions:[cluster_cidr]}`, `SDNRoute{DestinationPrefix:cluster_cidr, NeedEncap:false}`, `ACL{Allow, In, RemoteAddresses:cluster_cidr}`). Phase 2 plumbs slice awareness through `OverlayBootstrap` (T6), constrains the agent-local `IpAllocator` to the per-node slice + persists state to `<data_dir>/agent_ipam.json` (T7), and propagates `slice_cidr: String` through `Request::RegisterNode` / `NodeInfo` / `ClusterJoinResponse` / `AddMemberParams` so every node registers with its slice in Raft (T8). Phase 3 switches `HcsRuntime` from HNS NAT to HCN Transparent (T9), adds `Runtime::get_container_namespace_id()` to the trait (T10), adds `OverlayManager::attach_container_hcn(namespace_id, service_name, ip_override, autoclean)` parallel to the Linux `attach_container(pid, ...)` (T11), installs a catch-all `cluster_cidr → Wintun` host route so longest-prefix-match sends local-slice traffic to the vSwitch and remote traffic to the boringtun tunnel (T12), and branches `service.rs` on `cfg(target_os = "windows")` (T13). Phase 4 deletes the in-progress HNS NAT code outright: `EndpointAttachment::create` (NAT flavor), `NAT_NETWORK_SUBNET`, `NAT_NETWORK_NAME`, and `NatNetwork` state are gone (T14–T15). Phase C NAT work was never released (`git tag` is at `v0.10.85`), so this is a direct swap with no migration. macOS (sandbox + port-override + proxy) and Linux (veth + netns + `/32` host route) paths are completely unchanged; all Windows-specific code is `#[cfg(target_os = "windows")]`-gated. Also fixes a latent IP-collision bug that existed in the shipped Linux path — the agent-local `IpAllocator` is now bounded to the per-node `/28` slice so two nodes' agents cannot hand out the same container IP. New Windows-gated integration test at `crates/zlayer-agent/tests/windows_overlay_e2e.rs` (4 `#[ignore]` tests) exercises the real HCN APIs on a Windows host. Primary files: `crates/zlayer-overlay/src/{allocator,bootstrap,config,transport}.rs`, `crates/zlayer-agent/src/{overlay_manager,service,runtime,runtimes/hcs}.rs`, `crates/zlayer-hns/src/{adapter,attach,network,schema}.rs`, `crates/zlayer-api/src/handlers/cluster.rs`, `crates/zlayer-scheduler/src/raft.rs`, `bin/zlayer/src/{daemon,commands/node,commands/serve}.rs`, `crates/zlayer-paths/src/lib.rs`.
- **Wintun cluster-CIDR host route on Windows (Phase 3 / T12 of Windows overlay work).** On Windows, `OverlayTransport::configure_windows` now installs a catch-all on-link host route for the full cluster CIDR (e.g. `10.200.0.0/16`) pointed at the Wintun adapter, immediately after the IP layer comes up and before the ingress/egress/timers loops spawn. HCN auto-installs the more-specific per-node `/28 → vSwitch` route for local-slice container IPs, so Windows longest-prefix-match sends local traffic to the vSwitch (for intra-node container-to-container hops) and remote traffic to Wintun — where boringtun's egress loop picks it up, encapsulates, and forwards to the owning peer. Without this route the kernel has no forwarding entry for remote-container packets, they fall back to the default gateway, and cross-node overlay traffic silently leaks out of the physical NIC instead of transiting the WireGuard tunnel. New optional `cluster_cidr: Option<String>` field on `zlayer_overlay::OverlayConfig` (serde-default `None`) holds the full cluster CIDR alongside the existing `overlay_cidr` (which has always stored the per-node slice / host IP, e.g. `10.200.0.0/28` or `10.200.0.1/32`, not the cluster range). `OverlayBootstrap::start` now populates `cluster_cidr: Some(self.config.cidr.clone())` at all three `OverlayConfig` construction sites (main transport creation in `start()`, plus the two transient `add_peer` / `remove_peer` fallback paths when the live transport isn't in hand). Route install uses the existing `InterfaceOps::add_route_via_dev(dest, prefix_len, name)` Windows backend (`WindowsIpHelperOps` → `CreateIpForwardEntry2` with `MIB_IPPROTO_NETMGMT = 3` and an unspecified on-link next hop — the same path already used for the `overlay_cidr`/slice route). Failure is non-fatal: parse errors, "route already exists", and missing `cluster_cidr` all log a `tracing::warn!` and continue so adapter bringup stays idempotent across daemon restarts and pre-cluster-CIDR configs. Linux/macOS paths are byte-identical — the route-install block is fully `#[cfg(windows)]`-gated. Full `cargo check --workspace --all-targets` and `cargo clippy --workspace --all-targets -- -D warnings` clean on Linux; 117 `zlayer-overlay` lib tests pass. Files: `crates/zlayer-overlay/src/config.rs`, `crates/zlayer-overlay/src/bootstrap.rs`, `crates/zlayer-overlay/src/transport.rs`, `crates/zlayer-overlay/tests/overlay_e2e.rs`.
- **Per-node `/28` slice CIDR plumbed through cluster join (Phase 2 / T8 of Windows overlay work).** The leader now carves its cluster CIDR (`10.200.0.0/16` by default) into `/28` slices via `zlayer_overlay::NodeSliceAllocator` and hands each joining node its own non-overlapping slice alongside the existing flat overlay IP, fixing the latent IP-collision bug where every agent's IPAM independently allocated container IPs from the full cluster `/16`. `Request::RegisterNode`, `NodeInfo`, and `AddMemberParams` in `zlayer-scheduler` gained a `slice_cidr: String` field (`#[serde(default)]`, empty string = pre-slice-aware registration, preserves legacy semantics). `ClusterJoinResponse` gained a matching `slice_cidr: String` field. `ClusterApiState` gained `slice_allocator: Arc<RwLock<NodeSliceAllocator>>` + `slice_allocator_path: Option<PathBuf>`; the `cluster_join` handler now assigns a `/28` slice to the joining raft node ID right after overlay-IP allocation (via `NodeSliceAllocator::assign(&raft_node_id.to_string())`), persists a snapshot next to `ip_allocator.json` (as `slice_allocator.json`) on every assignment, threads the slice into `AddMemberParams.slice_cidr` so it lands in the Raft state machine, and surfaces it in the response. `bin/zlayer/src/commands/serve.rs` initialises the slice allocator at startup — loads from disk if `slice_allocator.json` exists (via `NodeSliceAllocator::restore`), falls back to a fresh allocator built against `node_config.overlay_cidr` at `zlayer_overlay::DEFAULT_SLICE_PREFIX = 28`, and wires the allocator + path into `ClusterApiState::with_internal_token`. `bin/zlayer/src/daemon.rs` leader-registration path now reads the leader's own slice + overlay IP back from the live `OverlayManager` (`slice_cidr()` / `node_ip()`) instead of the legacy hardcoded `"10.200.0.1"`; falls back to the hardcode when overlay is disabled (host-network mode) or when the manager is unconfigured. `bin/zlayer/src/commands/node.rs` parses `NodeJoinResponse.slice_cidr` into `Option<ipnet::IpNet>` (empty → `None`, malformed → hard error), persists it on `BootstrapConfig.slice_cidr` in `overlay_bootstrap.json` so the worker daemon's IPAM allocator can constrain itself to the slice (T7), and prints the assigned slice in the `node join` success summary. `handle_node_init` keeps the legacy hardcoded leader overlay IP with `slice_cidr: String::new()` (the daemon re-registers with the correct slice once `OverlayBootstrap::init_leader` has persisted bootstrap state). `zlayer_overlay::lib` re-exports `NodeSliceAllocator`, `NodeSliceAllocatorSnapshot`, `DEFAULT_SLICE_PREFIX`, and the `ipnet` crate so consumer crates can parse slice CIDRs without a direct `ipnet` dep. Four existing scheduler tests (`test_node_registration`, `test_force_leader_state_save_load`, `test_force_leader_state_save_load_ipv6_overlay`, `test_node_registration_with_ipv6_overlay`) and the `test_cluster_join_response_serialize` API test updated to include the new `slice_cidr` field. Full workspace `cargo test --workspace --lib` (~3200 tests) and `cargo test --workspace --doc` pass; `cargo clippy --workspace --all-targets -- -D warnings` clean. Files: `crates/zlayer-scheduler/src/raft.rs`, `crates/zlayer-api/src/handlers/cluster.rs`, `crates/zlayer-overlay/src/lib.rs`, `bin/zlayer/src/commands/serve.rs`, `bin/zlayer/src/commands/node.rs`, `bin/zlayer/src/daemon.rs`.
- **Windows overlay packet loop via `boringtun::noise::Tunn` (Phase D5 of native Windows containers).** `zlayer-overlay`'s Windows transport now drives the full `WireGuard` noise pipeline end-to-end against the Wintun adapter landed in Phase D3 — no more "packet loop pending" stub. `OverlayTransport` gained four Windows-only fields (`udp: Option<Arc<UdpSocket>>`, `peers: Arc<DashMap<[u8; 32], WindowsPeerState>>`, and three `JoinHandle<()>` slots for the spawned tasks). Per-peer state is a new `WindowsPeerState { tunn: Arc<tokio::sync::Mutex<Tunn>>, endpoint: Arc<parking_lot::RwLock<Option<SocketAddr>>>, last_handshake_sec: Arc<AtomicU64>, allowed_ips: Arc<Vec<ipnet::IpNet>>, persistent_keepalive: Option<u16> }` struct; `DashMap` lets ingress/egress/timers mutate concurrently without a global lock, the async Mutex wraps `Tunn` (every boringtun call needs `&mut self`), and `parking_lot::RwLock` handles the read-dominated endpoint field for NAT switches. `configure(..)` is now `&mut self` on every platform (unchanged semantics on Linux/macOS — callers already had `mut` bindings; the existing `transport.configure(..)` call in `OverlayBootstrap::start` and the four `tests/overlay_e2e.rs` callers all used `let mut manager` / `let mut transport`). The Windows branch of `configure` orchestrates the full bring-up: (1) apply IP/route via the Phase-D3 `WindowsIpHelperOps`, (2) bind a Tokio `UdpSocket` on `OverlayConfig::local_endpoint` (IPv4/IPv6 family follows the configured address), (3) call `add_peer_windows` per seeded peer to construct a fresh `Tunn` via `boringtun::noise::Tunn::new(StaticSecret::from(our_priv_bytes), PublicKey::from(peer_pub_bytes), preshared=None, persistent_keepalive, index=0, rate_limiter=None)` — boringtun assigns its own session indices and per-tunnel rate limiter, so the spec's `index=0` / `None` defaults are correct — and drop it into the `DashMap`, (4) spawn three driver tasks with `Arc` clones of the shared state. **Ingress** (`UDP → decapsulate → Wintun`): 65536-byte scratch buffer, `recv_from`, snapshot `(pubkey, state)` pairs from the DashMap to release shard locks before awaiting the async per-peer Mutex, then try each peer's `Tunn::decapsulate(Some(src.ip()), &datagram, &mut out)` in turn — `WriteToTunnelV4`/`WriteToTunnelV6` copy the cleartext, drop the Mutex, and forward to `WindowsTun::send`; `WriteToNetwork` emits a handshake-response or cookie-reply back over UDP; `Done`/`Err` means the packet wasn't for this peer, keep scanning. On a match we also refresh `endpoint` (implements WG's "endpoint follows sender" semantics for NAT rebinds) and `last_handshake_sec` (atomic unix-seconds for the `check_peer_handshake` surface). After a successful decap we loop with an empty datagram to drain boringtun's internal packet queue (documented `TunnResult::WriteToNetwork`-until-`Done` re-call pattern). **Egress** (`Wintun → encapsulate → UDP`): read a clear IP packet via `WindowsTun::recv`, parse the destination IP with a new `parse_dst_ip` helper (reads bytes 16..20 for IPv4 after `packet[0] >> 4 == 4`, bytes 24..40 for IPv6 after `== 6`, returns `None` for truncated or unknown-version packets), linear-scan peers for the first whose `allowed_ips` contains `dst_ip`, read the stored endpoint (drop if `None` — handshake hasn't finished yet), then `Tunn::encapsulate(&clear, &mut ciphertext)` with a 65536+32 buffer (boringtun requires dst ≥ src.len() + 32 and ≥ 148 — comment pins this invariant). `WriteToNetwork` emits over UDP; `Done` means the packet was queued inside boringtun pending handshake completion (no-op). **Timers**: `tokio::time::interval(250 ms)` with `MissedTickBehavior::Delay` — on each tick snapshot the peer set, call `Tunn::update_timers(&mut out)` per peer with a 148-byte scratch buffer (the max WG handshake init length — documented and commented), emit `WriteToNetwork` results to the stored endpoint. This is what fires persistent-keepalives and re-initiates stale handshakes, matching the reference boringtun driver's cadence. `add_peer` on Windows now decodes `self.config.private_key` via a new `decode_key_b64` helper (base64 → 32-byte array, errors on length mismatch) and delegates to `add_peer_windows`. `remove_peer` decodes the base64 pubkey and calls `peers.remove`. `status` synthesizes a UAPI-compatible `wg show` dump from the DashMap (hex + b64 public keys, endpoint, allowed_ips, keepalive, `last_handshake_time_sec`, terminal `errno=0`) so the Linux/macOS and Windows responses share the same key=value shape the NAT traversal parser already consumes. `update_peer_endpoint` writes to the state's `RwLock<Option<SocketAddr>>`; `check_peer_handshake` reads `last_handshake_sec`. `shutdown` aborts the three task handles in order, drops the UDP Arc, clears the peer map, then drops the Wintun adapter — mirroring the clean shutdown semantics of the Linux `DeviceHandle::drop`. Three new helper fns (`decode_key_b64`, `parse_dst_ip`, `build_tunn`) are `#[cfg(windows)]`-gated; six new unit tests (`parse_dst_ip` for v4/v6/truncated/unknown-version, `decode_key_b64` roundtrip + wrong-length) are also gated so Linux/macOS test runs are unaffected. No new dependencies required — `wintun-bindings`, `boringtun`, `parking_lot`, `dashmap`, `base64`, and `ipnet` were all already on the `cfg(windows)` dep list from Phase D3. boringtun API verified against 0.7.0 source: `Tunn::new` returns `Self` directly (not `Result`), `update_timers(&mut self, &mut [u8])` emits `TunnResult`, `decapsulate` takes `Option<IpAddr>` for the source and the re-call-with-empty-datagram drain pattern is spec-documented. Linux/macOS paths are byte-identical (all additions live inside `#[cfg(windows)]` blocks; the signature change to `configure(&mut self, ..)` was already compatible with every call site). Files: `crates/zlayer-overlay/src/transport.rs`.
- **Windows overlay networking foundation via Wintun + IP Helper (Phase D3 of native Windows containers).** `zlayer-overlay` gained a Windows-native adapter + IP configuration path so the overlay transport compiles and stands up the host network layer on Windows without shelling out to `netsh` / PowerShell. New modules: `crates/zlayer-overlay/src/tun/{mod.rs, windows.rs}` (`WindowsTun` wraps `wintun-bindings 0.7` — RAII handle over `wintun.dll` + `Adapter` + `Session`, 4 MiB ring, optional `verify_binary_signature` Authenticode check, `Adapter::open`-before-`create` idempotency; async `recv`/`send` tunnel packets through `spawn_blocking` since Wintun itself is synchronous), `crates/zlayer-overlay/src/interface/windows.rs` (`WindowsIpHelperOps` implements `InterfaceOps` via `windows-rs 0.62`'s `Win32::NetworkManagement::IpHelper` — `add_address` programs `MIB_UNICASTIPADDRESS_ROW` via `InitializeUnicastIpAddressEntry` + `CreateUnicastIpAddressEntry`, `add_route_via_dev` programs `MIB_IPFORWARD_ROW2` via `InitializeIpForwardEntry` + `CreateIpForwardEntry2` with `MIB_IPPROTO_NETMGMT = 3` for user-installed routes and unspecified next-hop for on-link; `ConvertInterfaceAliasToLuid` converts name→LUID every call so the trait stays name-based; `delete_link` / `set_link_up` are explicit no-ops because Wintun owns the adapter lifecycle and the session is "up" from creation). `interface.rs` `platform_ops()` dispatcher gained a `cfg(windows)` arm; the old stub backend remains as fallback for unknown targets. `transport.rs` now forks on `cfg(windows)`: `create_interface` stands up the Wintun adapter (`WindowsTun::new` in `spawn_blocking`), `configure` successfully applies IP address + route via the new IP Helper backend *and then* returns an error flagging that the `boringtun::noise::Tunn` packet pipeline is not yet wired (the known limitation — the follow-up task delivers the per-peer `Tunn` + UDP/Wintun select loop). `add_peer` / `remove_peer` / `status` / `update_peer_endpoint` / `check_peer_handshake` each carry a Windows arm that returns the same "packet loop pending" error so the overall shape matches Linux/macOS. `shutdown` + `Drop` correctly tear down the Wintun adapter on Windows. boringtun's `device` feature (libc-gated, Unix-only) now only pulls in on `cfg(not(windows))`; Windows takes plain-default `boringtun = "0.7.0"` so `noise::Tunn` is available without dragging in the Unix-only TUN/UAPI plumbing. DLL distribution: **no `include_bytes!`** — `WindowsTun::new` locates `wintun.dll` at runtime via three paths in order: `%ProgramData%\ZLayer\wintun\wintun.dll` (installer-provided), the directory containing `zlayer.exe` (sibling), and the current working directory; on miss, returns `OverlayError::NetworkConfig` with a message pointing at <https://www.wintun.net>. Keeps the repo free of WireGuard-LLC-copyrighted blobs while matching the same distribution pattern that `wireguard-nt` / `boringtun` use. New workspace `zlayer-paths` dep on `zlayer-overlay` (feeds the `%ProgramData%\ZLayer\wintun\` lookup). New `cfg(windows)` deps: `wintun-bindings = "0.7"` (features = ["async", "verify_binary_signature"]), `windows = "0.62"` (features = Win32_Foundation + Win32_NetworkManagement_{IpHelper,Ndis} + Win32_Networking_WinSock), `parking_lot` + `dashmap` (for the Windows-only per-peer state map — placeholder for the packet-loop follow-up). Linux and macOS paths are **byte-identical** to pre-D3: `DeviceHandle` + UAPI + `/var/run/wireguard/<iface>.sock` unchanged. Three new Windows-only unit tests under `interface::windows::tests` verify `SOCKADDR_INET` round-trips for IPv4/IPv6 and `address_family()` constant mapping; one more in `tun::windows::tests` validates the DLL locator is total. `cargo clippy --workspace --all-targets -- -D warnings` green on Linux. Files: `crates/zlayer-overlay/Cargo.toml`, `crates/zlayer-overlay/src/{lib.rs,interface.rs,transport.rs}`, `crates/zlayer-overlay/src/interface/windows.rs` (new), `crates/zlayer-overlay/src/tun/{mod.rs,windows.rs}` (new).
- **Windows container networking via HCN (Phase C of native Windows containers).** New `zlayer-hns` crate wraps `windows-rs 0.62`'s `Win32_System_HostComputeNetwork` bindings: safe Rust `Network`/`Endpoint`/`Namespace` types (RAII handles over the `void*` HCN handles), matching serde schemas for `HostComputeNetwork`/`HostComputeEndpoint`/`HostComputeNamespace`/`EndpointStats`/`ModifyNamespaceSettingRequest` with exact hcsshim/hcn v2 PascalCase quirks (`IPConfigurations` two-cap, `IpAddress` one-cap), and high-level `EndpointAttachment::{create, teardown}` for the canonical 2026 flow (HostDefault namespace + endpoint attach via `HcnModifyNamespace`). `HcsRuntime` now lazily creates a single NAT network (`zlayer-nat`, subnet `10.88.0.0/16`) on first `create_container`, creates an `EndpointAttachment` per container, references the namespace GUID in the HCS container document's `Container.Networking.Namespace`, and reads the endpoint IP + properties via `HcnQueryEndpointProperties` so `get_container_ip` now returns a real address. Startup zombie reconcile helper (`list_owned_endpoints("zlayer")` + `delete_endpoint_and_namespace`) is in place but not yet wired into daemon boot. All HCN calls run under `spawn_blocking` to avoid stalling Tokio. Graceful degradation: if HCN is unavailable, containers still start with no attached network. Proxy/tunnel crates verified Windows-portable without code changes (pure Tokio + rustls + tokio-tungstenite). `zlayer-paths` now resolves `%ProgramData%\ZLayer\*` on Windows (instead of falling back to `/var/lib/zlayer` and silently failing). Files: `crates/zlayer-hns/*` (new), `crates/zlayer-agent/src/runtimes/hcs.rs`, `crates/zlayer-agent/Cargo.toml`, `crates/zlayer-paths/src/lib.rs`, workspace `Cargo.toml` (new member + dep alias).
- **`zlayer-paths` Windows defaults now point at `%ProgramData%\ZLayer` (Phase C.3 of native Windows containers).** `ZLayerDirs::default_data_dir()` on Windows switched from the per-user `%LOCALAPPDATA%\ZLayer` to the system-wide `%ProgramData%\ZLayer` root (with a `C:\ProgramData\ZLayer` literal fallback when the env var is stripped — e.g. under a bare `NetworkService` / `LocalService` account). HCS-backed Windows nodes run as `SYSTEM`, so per-user dirs are unreachable; downstream crates (proxy, tunnel, registry cache, certs) that fan out via `ZLayerDirs` sub-methods (`certs()`, `secrets()`, `logs()`, `cache()`, …) now resolve to a single machine-wide tree that every ZLayer service can share. `detect_data_dir()` also grew a Windows branch that probes `%ProgramData%\ZLayer\daemon.json` before falling back to `default_data_dir()`, matching the existing `/var/lib/zlayer/daemon.json` probe on Linux. New internal helper `windows_program_data_root()` wraps `std::env::var_os("PROGRAMDATA")` with the literal fallback; `home_dir_or_tmp()` was gated `#[cfg(not(target_os = "windows"))]` to silence the previous `dead_code` warning on the Windows target. No public API change (method signatures and subdirectory names are byte-identical to Phase B). Two new Windows-gated unit tests cover `PROGRAMDATA=C:\TestProgramData` and the env-missing fallback, asserting that `data_dir`, `certs`, `secrets`, `logs`, `default_run_dir`, `default_log_dir`, and `default_socket_path` all resolve under the ProgramData root. `cargo check -p zlayer-paths --target x86_64-pc-windows-gnu --all-targets` is green. Files: `crates/zlayer-paths/src/lib.rs`.
- **`RuntimeConfig::Hcs` variant + HCS wiring into `create_runtime` / `create_auto_runtime` (Phase B.4.4–B.4.5 of native Windows containers).** `zlayer-agent`'s `RuntimeConfig` gained a Windows-only `Hcs(HcsConfig)` variant that lets callers opt into the native Host Compute Service runtime. `create_runtime` dispatches `Hcs(cfg)` to `HcsRuntime::new(cfg).await` (wrapped in `Arc`). `create_auto_runtime` on Windows now tries `HcsRuntime` first and only falls back to Docker when HCS is unavailable — replacing the previous log-only "will use Docker Desktop (WSL2 backend)" branch, so a stock Windows Server / Windows 11 Pro host picks the native runtime without Docker Desktop installed. The previous `RuntimeConfig::Wsl2` variant is preserved for one release as a `#[deprecated]` alias that warns and dispatches to `HcsRuntime::new(HcsConfig::default())` rather than erroring out with `"WSL2 runtime is not yet implemented"` — existing `runtime: wsl2` configs keep working through the deprecation window. `HcsRuntime::new` signature moved from `(HcsConfig, Arc<zlayer_registry::ImagePuller>) -> Result<Self>` (sync) to `(HcsConfig) -> Result<Self>` (async), constructing the registry internally from `zlayer_registry::CacheType::from_env()` (mirroring `WasmRuntime::new`'s pattern) so the dispatch site doesn't have to thread a puller through. The old three-arg form is preserved as `HcsRuntime::new_with_registry(config, registry)` for callers that need a specific cache backend. Files: `crates/zlayer-agent/src/lib.rs`, `crates/zlayer-agent/src/runtimes/hcs.rs`.
- **HCS-backed `Runtime` implementation for native Windows containers (Phase B.4.1–B.4.3 of native Windows containers).** New Windows-only `HcsRuntime` in `crates/zlayer-agent/src/runtimes/hcs.rs` drives the Host Compute Service directly via the `zlayer-hcs` bindings — no Docker daemon, no WSL2 indirection. Implements the full [`Runtime`] trait for the MVP deploy path: `pull_image` / `pull_image_with_policy` (delegates to `crate::windows::unpacker::unpack_windows_image` and caches the resulting `UnpackedImage` keyed by image reference); `create_container` (builds a schema v2.1 `ComputeSystem` JSON document — `Owner="zlayer"`, parent layer chain in child-to-parent order, scratch layer path, optional `Processor.Count` from `spec.resources.cpu.ceil()`, optional `Memory.SizeInMB` parsed from `spec.resources.memory` via `bundle::parse_memory_string`, optional `Hostname` from `spec.hostname` — and calls `ComputeSystem::create`); `start_container` / `stop_container` (graceful `HcsShutDownComputeSystem` with `TimeoutSeconds` options + escalation to `HcsTerminateComputeSystem` on elapsed timeout) / `remove_container` (terminate → drop system handle → `WritableLayer::detach_and_destroy` to tear down WCIFS and the scratch directory); `kill_container` (Windows-native — maps every POSIX signal to a forced terminate after standard signal-name validation); `wait_container` / `wait_outcome` (backed by a background task that subscribes to `events::subscribe` and populates a shared `Arc<RwLock<Option<i32>>>` when `HcsEventKind::SystemExited` fires; `ServiceDisconnect` maps to exit code `-1` / `WaitReason::RuntimeError`); `get_container_stats` (calls `ComputeSystem::read_statistics` and translates via a new `translate_stats(&Statistics) -> ContainerStats` helper — `ProcessorStats.TotalRuntime100ns` converted from 100-ns ticks to microseconds via `/10`, `MemoryStats.MemoryUsagePrivateWorkingSetBytes` as `memory_bytes`, `memory_limit` reported as `u64::MAX` since HCS doesn't surface a hard limit in the Statistics property); `exec` (creates an `HcsCreateProcess` with stdout/stderr pipes requested and polls `ProcessStatus.ExitCode` at 100 ms cadence up to 60 s, returning the exit code with empty stdout/stderr — full pipe-streaming is deferred to a follow-up); `exec_stream` (buffered fallback matching the trait's default behaviour); `inspect_detailed` (returns last observed exit code; network / port fields stay empty until Phase C); `list_images` / `remove_image` / `tag_image` (operate on the in-memory image cache — `remove_image` also best-effort calls `wclayer::destroy_layer` on each layer directory). `get_container_ip`, `get_container_pid`, and `prune_images` are stubbed with clean `Ok(None)` / `Ok(PruneResult::default())` returns rather than `Unsupported`, since network attachment (Phase C) and image pruning are deferred features and the rest of the agent is expected to tolerate these being inert. `container_logs` / `get_logs` surface `AgentError::Unsupported` with a message pointing users at `zlayer exec` until container-level stdio capture lands. New public surface: `HcsRuntime`, `HcsConfig { storage_root, default_isolation, default_scratch_size_gb }`, `IsolationMode { Process, Hyperv }`, `OWNER_TAG: &str = "zlayer"`, and a free `list_owned_systems() -> Result<Vec<String>>` helper that the agent boot path can call to surface zombie compute systems from a previous run. Four unit tests cover `translate_stats` (100-ns-to-usec conversion + private-working-set mapping, and zero-defaults when fields are absent), `extract_exit_code` JSON payload parsing for the event-stream path, and `extract_process_exit_code` for both nested (`{"ProcessStatus":{"ExitCode":…}}`) and flat shapes. The module is `#[cfg(target_os = "windows")]`; its `mod.rs` registration and re-exports are Windows-gated to match. `RuntimeConfig::Hcs` dispatch wiring is deliberately deferred to Phase B.4.4 so this file can land on its own. Files: `crates/zlayer-agent/src/runtimes/hcs.rs` (new), `crates/zlayer-agent/src/runtimes/mod.rs`.
- **Platform resolver honors `os.version` for Windows multi-platform indexes (Phase B.2.3 of native Windows containers).** `zlayer-spec`'s `TargetPlatform` gained an optional `os_version: Option<String>` field (serialized as `osVersion`, omitted when `None`) plus a `with_os_version(..)` builder and an `as_detailed_str()` diagnostic formatter. `TargetPlatform::new(os, arch)` is unchanged and still constructs with `os_version: None` — no existing caller breaks. `Display` is also unchanged (still `linux/amd64`-style) so log lines don't shift; version-aware diagnostics use the new `as_detailed_str()` which renders `windows/amd64 (os.version=10.0.26100.1)` when set. The `Copy` derive was dropped (required because `String` isn't `Copy`); two internal call sites that relied on `Copy` were switched to `.as_ref().map(..)` / `.platform.as_ref()` pattern (zlayer-py getter, zlayer-scheduler `can_place_on_node` + `no_suitable_node_reason`). In `zlayer-registry`, `build_platform_resolver` now runs a Windows-only pre-pass when the caller pinned an `os_version`: it prefers manifest entries whose `platform.os.version` either equals the pin exactly OR starts with it (so `10.0.26100` matches every `10.0.26100.*` build variant). On no-version-match the resolver falls back to the existing "any windows/amd64" pass — a bootstrap index that ships only one build stays pullable. Behavior for non-Windows targets, and for Windows targets without a pinned `os_version`, is byte-identical to Phase A. Macos `darwin→linux` fallback is preserved. Five new `zlayer-spec` tests cover builder/YAML roundtrip/omit-when-none/detailed-str/Display-unchanged; five new `zlayer-registry` tests cover prefix match, exact match, no-version-set fallback, no-match-found fallback, and the non-Windows "ignored os_version" path — all via a new `mk_entry(..)` helper that fabricates `ImageIndexEntry` values without touching the network. Files: `crates/zlayer-spec/src/types.rs`, `crates/zlayer-registry/src/client.rs`, `crates/zlayer-py/src/spec.rs`, `crates/zlayer-scheduler/src/placement.rs`.
- `GET /api/v1/containers` and `GET /api/v1/containers/{id}` responses now include five new optional fields populated from the runtime's inspect result (§3.15 of `ZLAYER_SDK_FIXES.md`): `ports` (`Vec<PortMapping>`, translated back from bollard's `NetworkSettings.Ports` — handles static host-port bindings and ephemeral `host_port = None` entries), `networks` (`Vec<NetworkAttachmentInfo>` with `{network, aliases, ipv4}` per attachment, mirroring `NetworkSettings.Networks`), `ipv4` (`Option<String>` — first non-empty IPv4 across attached networks, preferring the `bridge` network to match `Runtime::get_container_ip`), `health` (`Option<ContainerHealthInfo>` with `{status, failing_streak, last_output}` — reads Docker's native `ContainerState.Health` so images with a baked-in `HEALTHCHECK` surface correctly; status normalises to `"none"` / `"starting"` / `"healthy"` / `"unhealthy"`), and `exit_code` (`Option<i32>` — only populated when `ContainerState.Status == exited | dead`, so running containers don't surface stale zero values). All five are `#[serde(skip_serializing_if)]`-gated, so existing SDK clients that don't know about them see the same wire shape as before, and missing fields on ingress default to empty / `None` (backwards-compat verified by the new `test_container_info_deserialize_backwards_compat` test). New `Runtime::inspect_detailed(id) -> Result<ContainerInspectDetails>` trait method — default impl returns an empty record (preserving Youki / WASM / MacOS sandbox / mock behaviour), Docker override translates a single `inspect_container` call. Translation logic is a free function `translate_inspect_details(&ContainerInspectResponse) -> ContainerInspectDetails` in `crates/zlayer-agent/src/runtimes/docker.rs` so it can be unit-tested without a live daemon. New `NetworkAttachmentInfo` and `ContainerHealthInfo` wire DTOs (with `From<NetworkAttachmentDetail>` / `From<HealthDetail>` impls bridging the runtime-level structs) registered in the OpenAPI schemas. Files: `crates/zlayer-agent/src/runtime.rs`, `crates/zlayer-agent/src/runtimes/docker.rs`, `crates/zlayer-api/src/handlers/containers.rs`, `crates/zlayer-api/src/openapi.rs`.
- Bridge-network runtime wiring + `CreateContainerRequest.networks` (§3.2 of `ZLAYER_SDK_FIXES.md`). The previously-metadata-only `/api/v1/container-networks` endpoints now connect to a real Docker daemon when the binary is built with the `docker` feature and `bollard` can reach the local socket; otherwise the state falls back to metadata-only mode with a single warning logged on first use. New `DockerBridgeNetworkRuntime` in `zlayer-api` (gated by a new `docker` feature that forwards `bollard` as an optional dep) implements `BridgeNetworkRuntime::{create, delete, connect, disconnect}` via bollard's `create_network` / `remove_network` / `connect_network` / `disconnect_network`. Error mapping: bollard's `DockerResponseServerError` status code maps `404 → RuntimeError::NotFound`, `409 → RuntimeError::AlreadyExists`, everything else → `RuntimeError::Failed`; non-`DockerResponseServerError` variants fall back to a string match against the error's `Display` for the same three buckets. `POST /api/v1/containers` now accepts an optional `networks: Vec<NetworkAttachmentRequest>` field (each entry: `{network, aliases?, ipv4_address?}`). Attachments are validated up-front (registry plumbed in, network exists by id or name, IPv4 parses), then applied after `start_container` succeeds. On any attach failure the handler rolls back partial progress — disconnecting already-attached networks and stopping + removing the container — before returning the error. `BridgeNetworkApiState` gained `resolve_network_id`, `attach_to_registry`, and `detach_from_registry` public helpers so external handlers (the container create flow) can track attachments without depending on the handler-module's private helpers. `ContainerApiState` gained a fluent `with_bridge_networks(..)` setter. Serve wiring in `bin/zlayer/src/commands/serve.rs` builds the `BridgeNetworkApiState` (with the Docker-backed runtime when reachable), nests `build_container_network_routes` at `/api/v1/container-networks`, and threads the state into `ContainerApiState`. `NetworkAttachmentRequest` registered in the `OpenAPI` schema list. Files: `crates/zlayer-api/Cargo.toml` (new `docker` feature, `bollard` as an optional dep), `crates/zlayer-api/src/handlers/{containers.rs, container_networks.rs, container_networks_docker.rs (new), mod.rs}`, `crates/zlayer-api/src/{lib.rs, openapi.rs}`, `bin/zlayer/{Cargo.toml, src/commands/serve.rs, src/main.rs}`.
- Inline registry auth on `POST /api/v1/containers` and `POST /api/v1/images/pull` (§3.10 of `ZLAYER_SDK_FIXES.md`). Both DTOs now accept two mutually-precedence-ordered optional fields: `registry_credential_id: Option<String>` (looked up in the persistent `RegistryCredentialStore`) and `registry_auth: Option<RegistryAuth>` (inline `{username, password, auth_type}`, never persisted, never logged). `registry_auth` wins when both are supplied. Resolution flows into a new third argument on `Runtime::pull_image_with_policy(&self, image, policy, auth: Option<&RegistryAuth>) -> Result<()>`: the Docker runtime threads the inline creds into bollard's `create_image` via `DockerCredentials` (`X-Registry-Auth` header), while Youki / WASM / macOS sandbox / macOS VM / mock runtimes accept the new parameter for trait conformance and fall through to their existing hostname-based `AuthResolver` lookup — so `None` everywhere preserves pre-§3.10 behaviour. `ContainerApiState::with_registry_store` / `ImageState::with_registry_store` builder methods wire the persistent store into the handlers; without the store configured, a request carrying `registry_credential_id` is rejected with `400 Bad Request`. New public types on `zlayer-spec`: `RegistryAuth`, `RegistryAuthType` (serde `snake_case`, `#[default] = Basic`). Both registered in the OpenAPI schemas. Files: `crates/zlayer-spec/src/types.rs`, `crates/zlayer-agent/src/runtime.rs`, `crates/zlayer-agent/src/runtimes/{docker,youki,wasm,macos_sandbox,macos_vm}.rs`, `crates/zlayer-agent/src/{service,job}.rs`, `crates/zlayer-api/src/handlers/{containers,images}.rs`, `crates/zlayer-api/src/openapi.rs`.
- `CreateContainerRequest` (`POST /api/v1/containers`) now accepts an optional `restart_policy` field (Docker-style), mapping to bollard's `HostConfig.RestartPolicy`. Per §3.4 of `ZLAYER_SDK_FIXES.md`. New `zlayer_spec::ContainerRestartPolicy { kind: ContainerRestartKind, max_attempts: Option<u32>, delay: Option<String> }` with `ContainerRestartKind` one of `no` / `always` / `unless_stopped` / `on_failure` (all serde `snake_case`). `max_attempts` is forwarded as `maximum_retry_count` for `on_failure` only — Docker ignores it for the other kinds. `delay` is accepted (validated as a humantime string on the API boundary) but intentionally not forwarded: bollard's `RestartPolicy` has no per-kind delay field — Docker applies its own exponential backoff starting at 100ms — so when a caller specifies `delay` we emit a `tracing::warn!` at translation time and drop it. Names deliberately namespaced as `ContainerRestartPolicy` / `ContainerRestartKind` (not `RestartPolicy` / `RestartKind`) to avoid colliding with the existing `PanicPolicy` / `PanicAction` types, which govern post-panic behavior rather than runtime-level restarts. Added to `ServiceSpec.restart_policy` as `Option<ContainerRestartPolicy>` (serde-default, None = Docker's `"no"` default). New `translate_restart_policy(..)` in `zlayer-agent`'s docker runtime handles the translation. Both types registered in the `OpenAPI` schema list. Files: `crates/zlayer-spec/src/types.rs`, `crates/zlayer-api/src/handlers/containers.rs`, `crates/zlayer-api/src/openapi.rs`, `crates/zlayer-agent/src/runtimes/docker.rs`.
- `GET /api/v1/containers/{id}/wait` response now includes optional `reason`, `signal`, and `finished_at` fields alongside the existing `exit_code` (§3.12 of `ZLAYER_SDK_FIXES.md`). Fields are serde `skip_serializing_if = Option::is_none`, so clients that only read `exit_code` keep seeing the same shape. `reason` is one of `"exited"` / `"signal"` / `"oom_killed"` / `"runtime_error"` (serialized snake_case from the new `zlayer_agent::runtime::WaitReason` enum); `signal` carries a canonical name like `"SIGKILL"` / `"SIGTERM"` derived from `exit_code - 128` (falls back to `"signal_<n>"` for unknown numbers); `finished_at` is an RFC3339 string. A new `Runtime::wait_outcome(&ContainerId) -> Result<WaitOutcome>` trait method backs this — the default impl delegates to the existing `wait_container` and synthesizes `WaitReason::Exited`, so every non-Docker runtime keeps working unchanged. The `DockerRuntime` implementation reads `state.oom_killed`, `state.exit_code`, `state.finished_at`, and `state.error` from `inspect_container` after the wait stream resolves, and classifies accordingly: `oom_killed == Some(true)` → `OomKilled`; exit code in `[129, ..)` → `Signal` with a best-effort signal name; non-empty `error` with `exit_code == 0` → `RuntimeError`; otherwise `Exited`. The wait handler also publishes a matching `container.oom` event on the bus (in addition to the existing `container.die`) when the runtime reports an OOM kill, so subscribers of `GET /api/v1/events` filtering on `container.oom` now actually see OOM events. New helpers: `zlayer_agent::runtime::{WaitOutcome, WaitReason, signal_name_from_exit_code}` plus `ContainerEvent::oom(..)` constructor. Files: `crates/zlayer-agent/src/runtime.rs`, `crates/zlayer-agent/src/runtimes/docker.rs`, `crates/zlayer-api/src/handlers/containers.rs`, `crates/zlayer-api/src/event_bus.rs`.
- `GET /api/v1/containers/{id}/stats` now supports optional `?stream=true[&interval=<seconds>]` query params. Default behavior is unchanged — no query string returns a single-shot JSON `ContainerStatsResponse`. With `stream=true` the handler switches to Server-Sent Events and emits one `event: stats\ndata: <ContainerStatsResponse JSON>` per tick. `interval` is in seconds; default 2; clamped to `[1, 60]`. Also accepts the legacy alias `interval_seconds`. Keep-alive cadence is 15s, matching the deployment/events SSE streams. On runtime error mid-stream (e.g. container exited → `AgentError::NotFound`) the stream emits a final `event: close\ndata: {"reason":"exited"}` (or `"runtime_error"` for other failures) and then ends — clients are expected to reconnect if they want to continue sampling a restarted container. Slow subscribers are dropped gracefully via `tokio::time::MissedTickBehavior::Delay` (no backlog replay). New `StatsQuery` `IntoParams` struct (discoverable via utoipa `params(..., StatsQuery)`). Handler return type switched to `Result<Response>` via `IntoResponse`, matching the existing `get_container_logs` dual JSON/SSE pattern. Files: `crates/zlayer-api/src/handlers/containers.rs`. Per §3.14 of `ZLAYER_SDK_FIXES.md`.
- `GET /api/v1/events?follow=true[&label=k=v]` — daemon-wide container lifecycle event stream (Server-Sent Events). Emits `container.start`, `container.die`, `container.oom`, and `container.health` events for every standalone container lifecycle transition handled through the API layer. Each SSE event carries `{kind, id, labels, exit_code?, reason?, status?, at}` JSON. Query params: `follow: Option<bool>` (default `true`; `false` returns an empty SSE that closes immediately), `label: Vec<String>` (repeatable, `k=v`; AND semantics — an event passes only if all label filters match). Under the hood a new `ContainerEventBus` (`tokio::sync::broadcast::channel(1024)`) lives on `ContainerApiState`; lifecycle handlers (`create`, `start`, `stop`, `delete`, `kill`, `restart`, `wait`) publish to the bus, and `GET /api/v1/events` subscribes. Slow subscribers that fall behind the 1024-event buffer receive a terminal `close` SSE event (`{reason: "lagged", dropped: N}`) and the stream ends — the client is expected to reconnect. Keep-alive cadence: 15s. `Start` fires on successful create-and-start + `POST /start` + the start half of `POST /restart`. `Die` fires on `POST /stop`, `DELETE`, `POST /kill`, the stop half of `POST /restart`, and `GET /wait` (with the observed exit code). `Oom` emission requires the richer wait-result surface (§3.12) and is deferred; `Health` emission requires the health-monitor integration (§3.3) and is also deferred. Follow-up: agent-layer runtime events (crashes/OOM detected outside the API) are not wired yet — today only API-initiated transitions emit. Files: `crates/zlayer-api/src/event_bus.rs` (new), `crates/zlayer-api/src/handlers/events.rs` (new), `crates/zlayer-api/src/handlers/containers.rs`, `crates/zlayer-api/src/handlers/mod.rs`, `crates/zlayer-api/src/lib.rs`, `crates/zlayer-api/src/router.rs`, `crates/zlayer-api/src/openapi.rs`, `bin/zlayer/src/commands/serve.rs`. Per §3.13 of `ZLAYER_SDK_FIXES.md`.
- Named volume CRUD: `POST /api/v1/volumes` (create), `GET /api/v1/volumes/{name}` (inspect), plus an enriched `GET /api/v1/volumes` list response. `POST` accepts `{name, size?, tier?, labels?}`; `name` must match `^[a-z0-9][a-z0-9_-]{0,63}$`; `size` is validated via `zlayer_spec::validate_memory_format` (e.g. `"512Mi"`, `"10Gi"`); `tier` is one of `local|cached|network` (matches `zlayer_spec::StorageTier`'s serde rename). Creation mkdirs `state.volume_dir/{name}` and writes a hidden `.metadata.json` sidecar recording `{labels, size, tier, created_at}`. Responses use the new `VolumeInfo` DTO (`{name, path, size_bytes?, labels, created_at, in_use_by}`); legacy implicit volumes (autocreated by `StorageSpec::Named` mounts) synthesize `created_at` from directory mtime and expose empty labels. The list handler returns `Vec<VolumeInfo>` (strict superset of the old `VolumeSummary` — existing SDK deserializers ignore the new fields). `DELETE /api/v1/volumes/{name}` now also refuses to remove a volume whose `in_use_by` is non-empty unless `?force=true`; `in_use_by` and the empty-check both ignore the sidecar. A new public trait `VolumeUsageSource` on `VolumeApiState` is the seam for wiring `in_use_by` from a concrete container registry (currently `None` in `zlayer serve`; `in_use_by` is empty until the sibling VolumeMount type-discriminator work lands). `size_bytes` computation excludes the sidecar. `require_role("operator")` gates create + delete. Files: `crates/zlayer-api/src/handlers/volumes.rs`, `crates/zlayer-api/src/router.rs`, `crates/zlayer-api/src/openapi.rs`, `crates/zlayer-api/src/lib.rs`.
- `VolumeMount` DTO on `CreateContainerRequest` (`POST /api/v1/containers`) gained a Docker-compatible `type` discriminator (`bind` | `volume` | `tmpfs`, new enum `VolumeMountType`) and `source` is now `Option<String>` (tmpfs has no source). Omitted `type` preserves legacy bind-mount behavior (translates to `StorageSpec::Bind`). `type: "volume"` translates to `StorageSpec::Named { name: source, target, readonly, tier: default, size: None }` (i.e. `source` is a named-volume identifier, validated against the same `^[a-z0-9][a-z0-9_-]{0,63}$` pattern the `/volumes` handler enforces). `type: "tmpfs"` translates to `StorageSpec::Tmpfs { target, size: None, mode: None }` and rejects requests that set a non-empty `source`. Bind sources must be absolute paths. `VolumeMountType` registered in the OpenAPI schemas. Files: `crates/zlayer-api/src/handlers/containers.rs`, `crates/zlayer-api/src/openapi.rs`. Per §3.6 of `ZLAYER_SDK_FIXES.md`.
- `CreateContainerRequest` (`POST /api/v1/containers`) now accepts three new optional fields — `hostname: Option<String>`, `dns: Vec<String>`, `extra_hosts: Vec<String>` — mirroring Docker's `--hostname` / `--dns` / `--add-host` flags. Fields round-trip through a new three-field set on `ServiceSpec` (`hostname`, `dns`, `extra_hosts`) and are wired into the Docker runtime via bollard's `Config.hostname`, `HostConfig.dns`, and `HostConfig.extra_hosts`. `dns` entries are validated as IPv4/IPv6 addresses; `extra_hosts` entries are validated as `hostname:ip` (with the literal `host-gateway` accepted as the ip half, per bollard/Docker convention for `host.docker.internal:host-gateway`). When `host_network` is set the three fields are skipped at the runtime layer since the container inherits the host's resolver / `/etc/hosts` / kernel hostname. The `zlayer docker run` shim grew matching `--dns` / `--add-host` flags and now forwards `--hostname` instead of warning that it's unsupported; the compose converter at `crates/zlayer-docker/src/compose/convert.rs` likewise threads `hostname` / `dns` / `extra_hosts` from compose into the generated `ServiceSpec` (removing the old "skipping" warnings). Files: `crates/zlayer-spec/src/types.rs`, `crates/zlayer-api/src/handlers/containers.rs`, `crates/zlayer-agent/src/runtimes/docker.rs`, `crates/zlayer-docker/src/cli/run.rs`, `crates/zlayer-docker/src/compose/convert.rs`.
- Windows WSL2 backend now auto-configures `%UserProfile%\.wslconfig` with a `vhdSize` cap sized to ~80% of free host disk on first setup, plus `sparseVhd = true` so the backing `ext4.vhdx` grows and reclaims cleanly. Idempotent: re-running `zlayer serve` only writes when values actually need to change, and existing user-set keys in `[wsl2]` and any other sections are preserved (parse/round-trip is via `toml_edit`, not a blind overwrite). If the user already has a vhdSize ≥ the computed floor it is left alone — we only raise the floor. Override via env `ZLAYER_WSL_VHD_GB=<N>` (GiB) or the new `zlayer serve --vhd-gb <N>` flag on Windows. Floor is 64 GiB regardless of inputs. Files: `crates/zlayer-wsl/Cargo.toml` (adds `toml_edit`, `fs4`; drops unused `winreg`), `crates/zlayer-wsl/src/wslconfig.rs` (new — `ensure_wslconfig`, `compute_default_gb`, `merge_doc`), `crates/zlayer-wsl/src/paths.rs` (new `install_dir` / `vhdx_path` helpers), `crates/zlayer-wsl/src/setup.rs` (hook before `setup_distro`; new `ensure_wsl_backend_ready_with_vhd_gb`), `crates/zlayer-wsl/src/lib.rs` (module registration), `bin/zlayer/src/cli.rs` (`--vhd-gb` on `Serve`), `bin/zlayer/src/main.rs` (threads the override through).
- `zlayer windows compact` — new Windows-only subcommand that reclaims freed space inside WSL2 back to the Windows host. Flow: gracefully stops the ZLayer daemon inside the `zlayer` distro, polls health until it exits (10 s cap), runs `wsl.exe --shutdown` to release `vmcompute.exe`'s lock on the vhdx, and then compacts the file via `Optimize-VHD -Mode Full` (Hyper-V PowerShell cmdlet, preferred when available) or a scripted `diskpart compact vdisk` fallback (works on Win 11 Home where Hyper-V isn't installed). Reports before/after bytes and which backend ran. Gated `cfg(all(target_os = "windows", feature = "wsl"))`. Files: `crates/zlayer-wsl/src/compact.rs` (new — `compact_distro`, `CompactReport`, `CompactMethod`), `crates/zlayer-wsl/src/shell.rs` (new — `wsl_control`, `powershell`, `diskpart_script` helpers), `bin/zlayer/src/cli.rs` (new `WindowsCommands` enum with `Compact { force }`), `bin/zlayer/src/commands/windows.rs` (new handler), `bin/zlayer/src/commands/mod.rs` (module registration), `bin/zlayer/src/main.rs` (dispatch).
- **Foreign-layer `urls[]` redirect fallback on 404 (Phase B.2.2 of native Windows containers).** `zlayer-registry`'s image pull path now honours the `urls` field on layer descriptors when the primary registry does not carry the blob — the pattern that Microsoft Container Registry (MCR) and other foreign-layer hosts use to deliver non-distributable Windows base layers (media types `application/vnd.docker.image.rootfs.foreign.diff.tar.gzip` and `application/vnd.oci.image.layer.nondistributable.v1.*`). New `ImagePuller::pull_blob_with_urls(image, digest, auth, urls, expected_size)` wraps the existing oci-client blob fetch: on success it behaves exactly like `pull_blob`; on a not-found failure (404 `ServerError`, `ImageManifestNotFoundError`, `reqwest` status 404, or an OCI `RegistryError` envelope carrying `BlobUnknown` / `ManifestBlobUnknown` / `ManifestUnknown` / `NotFound` / `NameUnknown`) and a non-empty `urls[]` list, it issues sequential `GET`s against each URL (capped at `MAX_FOREIGN_LAYER_REDIRECTS = 5` to guard against circular redirect spam), verifies the response SHA-256 against the descriptor digest, optionally checks `Content-Length` when `expected_size >= 0`, and returns the first match to be cached via the existing `BlobCacheBackend::put` (which re-verifies the digest). The first redirect failure is logged at `warn`, every success at `info`, and on total exhaustion the last redirect error is returned — if none were attempted, the original primary error is preserved. `pull_blob` is now a thin wrapper over `pull_blob_with_urls(..., &[], None)`, so non-foreign-layer callers are unchanged. `pull_image`'s layer loop passes `layer.urls.as_deref().unwrap_or(&[])` and `Some(layer.size)` so Windows base layers transparently resolve to MCR without CLI opt-in. `reqwest` added to `zlayer-registry`'s deps (uses workspace default features — no OpenSSL, no `--insecure`). New unit tests cover `is_blob_not_found` across every relevant `OciDistributionError` variant, pin the redirect cap at 5, and exercise `fetch_blob_from_url`'s error path for malformed / unreachable targets. Files: `crates/zlayer-registry/Cargo.toml`, `crates/zlayer-registry/src/client.rs`.
- **Platform-aware scheduling foundation (Phase A of native Windows containers).** Services can now declare a target platform via a new optional `platform: { os, arch }` field on `ServiceSpec`; the scheduler filters candidate agents by matching platform, leaving services Pending with a clear `"no agent matches required platform <os>/<arch>"` reason when nothing fits. Backward-compat: omitting `platform` (or sending `None`) preserves today's "any agent" semantics, and agents that predate platform reporting (`os`/`arch` = `None` on `NodeInfo`) are treated as wildcard matches so a rolling upgrade doesn't render the cluster unschedulable. New types in `zlayer-spec`: `OsKind { Linux, Windows, Macos }` (serde lowercase, OCI-string `linux`/`windows`/`darwin` via `as_oci_str`), `ArchKind { Amd64, Arm64 }` (OCI strings `amd64`/`arm64`), `TargetPlatform { os, arch }` with `Display` returning `linux/amd64`-style and `from_rust_os`/`from_rust_arch` constructors that map `std::env::consts::{OS, ARCH}` (`x86_64 → Amd64`, `aarch64 → Arm64`, `linux/windows/macos → matching OsKind`). All three registered in OpenAPI. Agents populate their own `os`/`arch` at cluster-join time from `std::env::consts` — wired through `ClusterJoinRequest` (`crates/zlayer-api/src/handlers/cluster.rs`), `Request::RegisterNode`, `AddMemberParams`, `apply_register_node`, and persisted on `NodeInfo` (`crates/zlayer-scheduler/src/raft.rs`) with `#[serde(default, skip_serializing_if = "Option::is_none")]` everywhere so postcard2-serialized snapshots from older agents still decode. Two CLI bootstrap sites (`bin/zlayer/src/commands/node.rs` for `node init` + `node join`) and one daemon-startup leader self-register (`bin/zlayer/src/daemon.rs`) all populate the new fields. Placement filter lives in `can_place_on_node` (`crates/zlayer-scheduler/src/placement.rs`) right after the healthy check; pending-reason logic extracted to a `no_suitable_node_reason(...)` helper. Image registry's manifest selector (`crates/zlayer-registry/src/client.rs`) gained an opt-in `TargetPlatform` override via a new `ImagePuller::with_platform(cache, target)` constructor; existing `new`/`with_cache` callers keep their signatures and internally pass `None` (back-compat preserved, including macOS `darwin → linux` fallback for hosts without an explicit override). PyO3 bindings (`crates/zlayer-py/src/spec.rs`) accept `platform=(os, arch)` as a kwarg on `create_service_spec` (lowercase string pair, validated against `linux|windows|macos` × `amd64|arm64` with explicit `PyValueError` messages) and expose a `platform` getter returning `Option<(String, String)>` in OCI form. New tests: 5 in `zlayer-spec` (round-trip, OCI strings, runtime-const detection), 3 in `zlayer-scheduler` placement (mismatched cluster stays Pending with the platform reason, no-platform service places freely, legacy `os/arch=None` node still accepts a service requiring a specific platform). Files: `crates/zlayer-spec/src/types.rs`, `crates/zlayer-api/src/openapi.rs`, `crates/zlayer-api/src/handlers/{containers.rs, cluster.rs}`, `crates/zlayer-api/src/storage/{mod.rs, deployments.rs}`, `crates/zlayer-docker/src/{cli/run.rs, compose/convert.rs}`, `crates/zlayer-scheduler/src/{raft.rs, lib.rs, placement.rs}`, `crates/zlayer-agent/src/runtimes/docker.rs`, `crates/zlayer-agent/tests/{docker_runtime_test.rs, wasip2_integration_test.rs, wasm_runtime_test.rs}`, `crates/zlayer-py/src/spec.rs`, `crates/zlayer-registry/{Cargo.toml, src/client.rs}`, `bin/zlayer/src/commands/node.rs`, `bin/zlayer/src/daemon.rs`.

### Changed
- Secrets handlers now enforce per-environment RBAC on env-scoped requests instead of blanket admin-only gates. `POST/DELETE /api/v1/secrets`, `POST /api/v1/secrets/{name}/rotate`, and `POST /api/v1/secrets/bulk-import` require `environment:write` on the target env; `GET /api/v1/secrets` (list) and `GET /api/v1/secrets/reveal-all` require `environment:read`. `GET /api/v1/secrets/{name}?reveal=true` on an env-scoped secret requires `environment:write` (plaintext exfil is a more sensitive op than metadata read). Admin short-circuits every check. Legacy scope paths (no `?environment=` query) remain admin-only. `SecretsState` gained a new `perm_store: Option<Arc<dyn PermissionStorage>>` field and a `with_rbac(..)` constructor; `zlayer serve` now wires `bundle.permissions` into it. When `perm_store` is `None` (tests / legacy setups) the handlers fall back to the pre-existing `require_admin` gate. Files: `crates/zlayer-api/src/handlers/secrets.rs`, `bin/zlayer/src/commands/serve.rs`.

### Added
- `zlayer manager init`: integrates with the secrets store for admin bootstrap. New flags: `--email <addr>`, `--password <value>` (exposed on argv — warned), `--password-file <path>` (trailing-newline trimmed), `--random` (generates a 32-char alphanumeric, printed once), `--no-prompt`, `--env-file <path>` (points the spec's `ZLAYER_BOOTSTRAP_PASSWORD_FILE` at an existing path; skips daemon calls), and `--no-bootstrap` (legacy commented-env spec). `--env-file` and `--no-bootstrap` are mutually exclusive. In the default "integrated" branch, the command connects to the daemon via `DaemonClient::connect`, ensures a global `bootstrap` environment exists (`list_environments(None)` → `create_environment("bootstrap", None, Some("Admin bootstrap credentials"))`), stores the password under `ZLAYER_BOOTSTRAP_PASSWORD` via `set_secret_in_env`, and emits a `manager.zlayer.yml` whose env block references `$secret://bootstrap/ZLAYER_BOOTSTRAP_PASSWORD` (resolved by zlayer-agent at container start). Interactive password entry uses the existing `dialoguer::Password` with confirmation; no new crate deps. `handle_manager` is now async. Files: `bin/zlayer/src/cli.rs`, `bin/zlayer/src/commands/manager.rs`, `bin/zlayer/src/main.rs`.
- `zlayer-secrets`: new `$secret://<env>/<KEY>[/<field>]` URL-form reference in `SecretsResolver::resolve_value`, alongside the existing `$S:` forms. Requires an `EnvScopeProvider` trait object attached via the new `SecretsResolver::with_env_resolver(..)` builder method; this decouples the resolver from any particular env-storage backend. Without it, `$secret://` refs fail with a clear `SecretsError::Provider` message. Malformed refs (`$secret://env` with no `/KEY`, or `$secret://env/`) return `SecretsError::InvalidName`. Field extraction (`$secret://bootstrap/database/password`) reuses the existing JSON helper. Crate-public exports bumped to include `EnvScopeProvider`. No API/CLI/serve wiring changed in this crate. Files: `crates/zlayer-secrets/src/{lib.rs,provider.rs}`.
- `zlayer secret grant|revoke|permissions` CLI subcommands — thin secrets-oriented wrapper over the permission API for managing per-environment access. `grant <user-email-or-id> <env-id-or-name> <read|execute|write>` hits `POST /api/v1/permissions` with `resource_kind="environment"`, resolving email → user id via `list_users` and name → env id via the existing `resolve_env_id` helper. `revoke <user> <env>` finds the matching grant row via `GET /api/v1/permissions?user=<id>` and `DELETE`s it by id (no-op with a friendly message when no grant exists). `permissions <env>` lists every grant on an environment via a new `GET /api/v1/permissions/by-resource?kind=environment&id=<env_id>` endpoint (a thin wrapper over `PermissionStorage::list_for_resource`), supporting `--output table|json`. Files: `crates/zlayer-api/src/handlers/permissions.rs`, `crates/zlayer-api/src/router.rs`, `crates/zlayer-api/src/openapi.rs`, `crates/zlayer-client/src/daemon_client.rs`, `bin/zlayer/src/cli.rs`, `bin/zlayer/src/commands/secret.rs`.
- One-shot daemon-startup migration: every existing admin user now receives an explicit `environment:write` `StoredPermission` row for every existing environment (global + project-scoped). Admins already short-circuit the `PermissionStorage::check()` path, so this is not required for correctness — it makes the grants visible to `GET /api/v1/permissions/by-resource?kind=environment&id=<env>` and the manager UI's permission listings instead of implicit-by-role. Idempotent: gated on an audit-log marker row (`resource_kind="migration"`, `action="env_admin_grants_migrated"`) written once on completion, so subsequent starts skip the migration. Runs after `bootstrap_admin` so CI-provisioned admins are covered. Files: `bin/zlayer/src/bootstrap_admin_env_grants.rs` (new), `bin/zlayer/src/main.rs`, `bin/zlayer/src/commands/serve.rs`.

### Changed
- `zlayer-agent`: point `libcontainer` at our youki fork (`ZachHandley/youki`, `zlayer-patches` branch) so we can cherry-pick open upstream fixes without waiting for youki releases. 0.6.1 has been one-click-away from shipping for 7+ weeks ([tagpr PR #3440](https://github.com/youki-dev/youki/pull/3440)) and the DinD-breaking cgroup-v2 nested-exec fix ([PR #3347](https://github.com/youki-dev/youki/pull/3347) / issue [#3342](https://github.com/youki-dev/youki/issues/3342)) still hasn't been merged to `main`. Initial patch carried on our fork: #3347 retry-cgroup-join-on-EBUSY. See `docs/youki-fork.md` for the branch layout and sync procedure.

### Fixed
- CI: `build.yml` and `publish-sdks.yml` no longer leave downstream jobs trying to download gitea artifacts that don't exist for the current `run_id`. The `ci-step-status` per-step markers (stored as JSON in a Forgejo generic package, keyed by version) persist across workflow runs indefinitely, but `gitea-upload-artifact@v4` uploads are scoped per `run_id` with 1-day retention — so any redispatch where a build job was previously marked complete would skip its upload, and dependent jobs (`image-build`, `build-macos-amd64`, `build-linux-arm64`, and in `publish-sdks.yml` the final `publish-py-binding`) would crash on `Artifact not found for name: …`. Fix: at the top gate of each affected workflow, invoke the existing `ci-step-status@main` action in `mode: purge-version` to wipe all markers for the version before any build job runs. In `build.yml` the purge is appended to the `check-existing` job and gated on `steps.check.outputs.need_build == 'true'` (so the "all final artifacts already exist" fast path is untouched); in `publish-sdks.yml` it's appended to the `detect` job and gated on `steps.detect.outputs.services-to-release-flat != '[]'` (so when `detect-changes --check-registry` reports nothing needs publishing, nothing is purged). `keep-markers: ''` overrides the action's default `release-complete` whitelist since neither workflow uses that sentinel. Within-run "re-run failed jobs" is preserved because `check-existing`/`detect` doesn't re-run and the markers from sibling jobs stay intact. `e2e.yml` has no cross-job artifact downloads and is not affected. Files: `.forgejo/workflows/build.yml`, `.forgejo/workflows/publish-sdks.yml`.
- CI: release tags no longer get created before e2e passes. Previously `ci.yaml`'s `auto-tag` job ran in parallel with the e2e dispatch — the tag existed before the first integration test started, and `build.yml` was already producing release artifacts for it regardless of e2e outcome. Reworked the chain to `ci → e2e → build (auto-tag on success) → release`. The git tag now only exists once all platform builds (linux amd64/arm64, macOS amd64/arm64, Windows amd64, image build) succeed, so tag existence implies "this version was successfully built." No change to branch history — tags are refs, creating one doesn't touch `dev`. Files: `.forgejo/workflows/{ci.yaml,e2e.yml,build.yml}`.
- CI: Windows thin CLI build (`cargo build --package zlayer --no-default-features --features wsl`) no longer fails to compile `bin/zlayer/src/commands/manager.rs`. The integrated bootstrap path in `handle_manager_init` (Branch C — the one that connects to the daemon via `DaemonClient`, ensures the `bootstrap` env exists, and writes `ZLAYER_BOOTSTRAP_PASSWORD` via `set_secret_in_env`) has been split into a `#[cfg(unix)]` helper carrying the original logic and a `#[cfg(not(unix))]` sibling that `bail!`s with a WSL2 hint, matching the Unix-gated `DaemonClient` in `crates/zlayer-client/src/lib.rs`. The supporting helpers `ensure_bootstrap_env`, `emit_integrated_spec`, `validate_email`, `prompt_email_interactive`, `resolve_password`, and `prompt_password_interactive` are also gated `#[cfg(unix)]` to silence dead-code warnings under `-D warnings` on the thin build. The cross-platform `--no-bootstrap` and `--env-file` branches continue to work from `zlayer.exe`.

### Added
- `zlayer run --env <slug> [--no-global] [--merge <slug>]... [--dry-run [--unmask]] -- <cmd> [args...]` — top-level CLI command that spawns a local process with secrets injected as environment variables. Resolution order (right wins on collision): parent process env → `global` env (implicit, unless `--no-global`) → each `--merge <slug>` env left-to-right → `--env <slug>` (highest priority). Child inherits parent env; `--dry-run` prints `KEY=***` lines (or plaintext with `--unmask` for admins) and exits 0. Exit code of the child propagates via `std::process::exit`. Unknown `global` env is treated as a silent skip; any other resolution failure is a hard error. Files: `bin/zlayer/src/cli.rs`, `bin/zlayer/src/commands/mod.rs`, `bin/zlayer/src/commands/run.rs` (new), `bin/zlayer/src/commands/secret.rs` (made `resolve_env_id` `pub(crate)`), `bin/zlayer/src/main.rs`.
- `GET /api/v1/secrets/reveal-all?environment={env_id}` — admin-only batch reveal returning every secret in an environment as plaintext in a single round-trip. Powers `zlayer run --env <id> -- <cmd>`, which previously had to loop `reveal_secret_in_env` per key. New DTO `RevealAllSecretsResponse { environment, secrets: HashMap<String, String> }` and client method `DaemonClient::reveal_all_secrets_in_env(env_id)`. Files: `crates/zlayer-api/src/handlers/secrets.rs`, `crates/zlayer-api/src/router.rs`, `crates/zlayer-api/src/openapi.rs`, `crates/zlayer-client/src/daemon_client.rs`.
- **OpenID Connect / SSO sign-in.** Configure one or more providers via `ZLAYER_OIDC_<NAME>_{ISSUER,CLIENT_ID,CLIENT_SECRET,REDIRECT_URL}` env vars (optional `DISPLAY_NAME` + `SCOPES`). New endpoints: `GET /auth/oidc/providers` (list), `GET /auth/oidc/{provider}/start` (302 to provider's authorize URL with CSRF + PKCE + nonce), `GET /auth/oidc/{provider}/callback` (verifies the ID token and issues a ZLayer session). First sign-in creates a passwordless local user row and a `(provider, subject) → user_id` link (new `oidc_identities.db`); repeat sign-ins return the linked user. Existing password-login users are linked by email on first OIDC sign-in rather than duplicated. Built on the `openidconnect` crate with lazy discovery caching — a provider whose discovery URL is unreachable at boot does not block daemon startup. Manager login page surfaces "Sign in with X" buttons for each configured provider.

### Changed
- User account lifecycle routed through a new `IdentityManager` facade (`crates/zlayer-api/src/identity.rs`). Creating, deleting, or changing the role of a user now updates the user store AND the credential store atomically (with rollback on partial failure), closing a drift window where `PATCH /api/v1/users/{id}` could change a role without propagating it to the credential's role array. Handlers `bootstrap`, `create_user`, `delete_user`, `update_user`, and the env-var admin bootstrap in `bin/zlayer` all now go through the facade. Added `CredentialStore::set_roles` as the supporting primitive. Read paths (`list_users`, `get_user`, `me`) unchanged.

### Added
- `zlayer user set-password`: non-interactive password rotation. New flags: `--email <addr>` (alternative to the positional user id, resolved via `list_users`), `--password <value>` (inline), `--password-file <path>` (trailing newline trimmed), `--random` (generate + print a 32-char alphanumeric once), and `--no-confirm` to skip the "are you sure" prompt. Interactive prompt remains the default when no source flag is supplied. Backing API (`POST /api/v1/users/{id}/password`) was already present and is unchanged.
- `zlayer-manager`: non-interactive admin bootstrap via `ZLAYER_BOOTSTRAP_EMAIL` + `ZLAYER_BOOTSTRAP_PASSWORD` (or `ZLAYER_BOOTSTRAP_PASSWORD_FILE`, preferred in production so the secret doesn't leak via `/proc/<pid>/environ`). When the users table is empty at startup the daemon creates the initial admin (Argon2id hash + user row) **before** the HTTP listener accepts, closing the "first-request-wins-admin" race for CI / IaC / k8s deployments. `ZLAYER_BOOTSTRAP_DISPLAY_NAME` optional (defaults to email local-part). Browser `/auth/bootstrap` flow unchanged for interactive installs. `images/ZImagefile.zlayer-manager` and the spec emitted by `zlayer manager init` now document the contract as commented-out env entries.

## [0.10.104]

### Added
- `wasm.oci: false` ZImagefile opt-out — skip OCI layout + push, still produce the raw `.wasm` with full caching / wasm-opt / adapter support.
- `docs/wasm-portability.md` — consumer compatibility matrix (wkg, Spin, wasmCloud, runwasi, ORAS, crane) + WIT-world portability guidance.
- ZImagefile `runtime: wasm` now delegates to WASM build mode with target autodetection (cargo-component, wasm32-wasip*, jco, componentize-py).
- `zlayer build` with `-t <tag>` now pushes WASM OCI artifacts to the registry, matching the container push flow.
- Integration test `wasm_oci_e2e` verifies the WASM builder->OCI pipeline end-to-end (manifest artifactType, layer media type, runtime routing).
- Docker-compat server endpoints: per-container stop/kill/start/restart/exec
  under `/api/v1/containers/{id}/{action}`; `POST /api/v1/images/{tag,pull}`
  for single-image ops. OpenAPI coverage included.
- `Runtime::kill_container(signal)` and `Runtime::tag_image(src, dst)` added on all backends (youki, docker, macos_*, wasm). Exposed as `POST /api/v1/containers/{id}/kill` and `POST /api/v1/images/tag`. Unblocks full Docker CLI compatibility (kill/tag).
- `zlayer docker` image subcommands (images, rmi, tag, pull, push, build) now work — bridged via zlayer-client. `build` maps Docker args to BuildSpec and returns the build id.
- `zlayer docker` container subcommands (ps, logs, stop, kill, start, restart, rm, exec, run) now work — bridged to the daemon via `zlayer-client`. `run` converts Docker args to a ServiceSpec and deploys.
- `zlayer docker compose up/down/ps/logs` now work end-to-end. `up` converts compose YAML to DeploymentSpec via existing `compose_to_deployment` and deploys via `DaemonClient`.
- Docker Engine API compat: `GET /info` returns real container/image counts, host OS/arch/ncpu. `GET /events` emits a live SSE-style event stream.
- Docker Engine API compat: real `GET /images/json`, `GET /images/{name}/json`, `POST /images/create` (pull), `POST /images/{name}/tag`, `DELETE /images/{name}`. `docker images`, `docker pull`, `docker tag`, `docker rmi` now work.
- Docker Engine API compat: real `GET /containers/json`, `GET /containers/{id}/json`, `GET /containers/{id}/logs`, `POST /containers/{id}/{start,stop,kill,restart}`, `DELETE /containers/{id}`. `docker ps`, `docker logs`, `docker start/stop/kill/restart/rm` now work against the zlayer-managed socket.
- Python binding (`zlayer-py`): new `zlayer.Client(socket=None, host=None)`
  class for driving a remote zlayer daemon from Python
  (ps/deploy/logs/exec/build/stop/status/scale). Unix-only (the daemon
  socket is Unix).
- Python binding: `zlayer.ensure_daemon(system=False, version=None)` bootstraps the zlayer binary from Python (userland to `~/.local/share/zlayer/bin/`; `system=True` runs `sudo zlayer daemon install`).
- Example: `examples/python/deploy_hello.py` — complete end-to-end example deploying a trivial service via the Python binding.
- Python binding: `tests/test_client.py` integration tests spawn a real daemon and exercise `Client` end-to-end (ps, status, deploy/stop).
- Python binding packaged for PyPI (full `pyproject.toml` metadata, `zlayer` distribution name, abi3-py38 wheels).
- Internal: DaemonClient extracted to zlayer-client library crate for reuse
  by zlayer-docker / zlayer-py / language SDKs. No behavior change.
- `zlayer-client` gains per-container stop/start/restart/exec and server-side `pull_image` + `start_build` methods, matching the Docker-compat and Python SDK needs.
- `zlayer-client::DaemonClient` gains `kill_container(id, signal)` and `tag_image(src, dst)` methods, completing the Docker-compat API surface.
- OpenAPI spec coverage: regenerated `openapi.json` now documents all REST domains (>=50 paths including nodes, overlay, tunnels, networks, proxy, cluster, environments, variables, projects, webhooks, credentials, groups, permissions, audit, users, tasks, workflows, notifiers, syncs, volumes, storage, cron, jobs, and the existing deployments/services/builds/secrets).
- Generated TypeScript REST client at `clients/typescript/` (from `openapi.json`, 117 paths, typescript-fetch generator). Ready for a façade package to wrap.
- New npm package `@zlayer/client` (at `clients/typescript-client/`): ergonomic TypeScript SDK wrapping the generated OpenAPI client. Methods: deploy, ps, logs, exec, build, stop, status, scale, kill, tag. `ensureDaemon()` optional binary bootstrap (userland default, `system: true` escalates).
- CI: shell completions (bash/zsh/fish/powershell/elvish) emitted during release builds and bundled into every platform's release artifacts.
- CI: `.forgejo/workflows/python-publish.yml` builds wheels for manylinux x86_64/aarch64, macOS x86_64/aarch64, and Windows, and publishes `zlayer` to PyPI on version tags.
- CI: `.forgejo/workflows/node-publish.yml` publishes `@zlayer/client` (and `@zlayer/api-client`) to npm on version tags.

### Changed
- CI: SDK publishing moved to `.forgejo/workflows/publish-sdks.yml`, guarded by `detect-changes` registry-check. `release.yml` triggers it via workflow_dispatch. `python-publish.yml` and `node-publish.yml` folded in. New `.forgejo/build-config.yaml` defines the `sdks` service group.
- WASM builds via `zlayer build` now produce a real OCI artifact
  (`artifactType: application/vnd.wasm.component.v1+wasm` or
  `module.v1+wasm`) instead of a bare `.wasm` file with null `oci_path`. The
  builder writes a full OCI image layout (`oci-layout`, `blobs/sha256/...`,
  `index.json`) next to the compiled WASM binary in a `<module>-oci`
  directory, and `BuiltImage.image_id` now uses the first user tag when
  present or a proper `wasm:sha256:...` manifest digest ref as a fallback
  (previously the image id embedded the full filesystem path).

### Fixed
- CI: dev pushes now actually dispatch `build.yml` after `auto-tag`. The
  earlier removal of the direct dispatch assumed the `trigger-e2e → e2e.yml
  → trigger-build` chain would handle it, but that chain is gated on
  `inputs.version != ''` which is always empty for branch pushes, so
  `build.yml` never ran. Restored the explicit `curl` dispatch inside the
  `auto-tag` job, passing the freshly-created `vX.Y.Z` as `version`. Also
  removed all unreachable `startsWith(github.ref, 'refs/tags/')`
  conditionals from ci.yaml — the workflow has no `tags:` filter, so those
  branches were dead code and were what made the broken reasoning look
  plausible.
- CI: `publish-sdks.yml` now actually runs. The `if:` gates used
  `contains(fromJson(services-to-release), fromJson('{"group":"sdks",...}'))`,
  but Forgejo's Act-based runner can't compare objects
  ("Compare not implemented for types: left: map, right: map"). Switched
  all 13 SDK gates to the flat form
  `contains(fromJson(services-to-release-flat), 'sdks/<service>')` and
  added a new `services-to-release-flat` output to the detect-changes
  action emitting a JSON array of `"group/service"` strings. A prior
  output-name mismatch (action.yml declared hyphenated names but the
  source called `core.setOutput("services_to_release", ...)` with
  underscores) was also fixed — every `fromJson(output)` was receiving
  an empty string and blowing up downstream.

## [0.10.103]

### Added
- **Shell completion generation.** New `zlayer completions <SHELL>` subcommand
  emits a completion script for `bash`, `zsh`, `fish`, `powershell`, or
  `elvish` to stdout, built from the live clap command tree. Install with
  e.g. `zlayer completions bash > /etc/bash_completion.d/zlayer`.
- Install scripts now drop shell completions for bash, zsh, fish (Linux/macOS)
  and PowerShell (Windows) after installing the binary. Fail-soft: install
  succeeds even if completion drop fails.

### Fixed
- Forgejo CI: `build.yml` now uses `christopherhx/gitea-upload-artifact` and `christopherhx/gitea-download-artifact` forks (upstream `actions/upload-artifact@v4` and `actions/download-artifact@v4` error with `GHESNotSupportedError` on Forgejo).
- TUI build progress counter denominator (`[X/Y] instructions`) previously grew in lockstep with the numerator because totals were summed from events as they arrived; now emitted once up-front via `BuildEvent::BuildStarted { total_stages, total_instructions }`.
- **Latent clap CLI-definition bugs that made `zlayer completions` (and
  `--help` on the affected subcommands) panic in debug builds.** Four
  issues in `bin/zlayer/src/cli.rs` surfaced by `clap_complete::generate`
  walking the whole subcommand tree:
  - `manager init`'s custom `--version` flag collided with clap's
    auto-injected `--version`; now annotated with `disable_version_flag`.
  - `job trigger`, `job status`, and `cron status` declared a non-required
    positional (`deployment`) before a required one (`job`/`cron`).
    `deployment` is now a `--deployment` flag, matching the pattern
    already used by `zlayer logs`.
  - The global `--detach` flag's `short = 'd'` collided with docker
    compat's `docker volume create -d <driver>`. The short form is
    dropped (use `--detach`); `-b/--background` is unaffected.

## [0.10.102]

### Fixed
- **Container log-follow SSE now terminates when the container exits.**
  `GET /api/v1/containers/{id}/logs?follow=true` previously polled forever,
  keeping the SSE body open even after the container had exited. Clients
  (notably ZArcRunner) blocked indefinitely on the next read. The follow
  stream now checks `Runtime::container_state` every fourth poll (plus the
  initial poll), performs a final log drain on observing `Exited` or
  `Failed`, and closes the stream. Detection latency ≤ 2s. Covered by
  `container_log_follow_stream_terminates_on_exit`.
- **`zlayer container logs` no longer fails with "Failed to parse JSON
  response from daemon".** The `/logs` (non-follow) endpoint returns plain
  text, but the daemon client was calling `serde_json::from_slice` on the
  body. `DaemonClient::get_container_logs` now returns `Result<String>`
  via a new `decode_text_body` helper; the caller in `commands/container`
  was simplified to a direct print. Covered by
  `container_logs_cli_decodes_plain_text` and
  `container_logs_cli_rejects_invalid_utf8`.

## [0.10.101]

### Added
- **`crates/zlayer-proxy/README.md`** so the crate's `Cargo.toml`
  `readme = "README.md"` line points at an actual file.

### Changed
- **CI workflows (`.forgejo/workflows/ci.yaml`, `.forgejo/workflows/build.yml`)
  now use the `setup-system-deps` action for all package installs.**
  Replaced inline `apt-get install`, `brew install`, and `choco install`
  steps with
  `https://forge.blackleafdigital.com/Public/actions/setup-system-deps@main`
  so apt runs get dpkg-lock timeout + noninteractive guards uniformly
  and OS detection / package-manager selection lives in one place.
  The Windows `build-windows-amd64` job also gains an
  `install-git-bash` step after `setup-rust`, which installs Git on the
  Forgejo Windows runner and fixes the `Cannot find: bash in PATH`
  failure that was blocking the protobuf and packaging `shell: bash`
  steps. `ci.yaml` also now installs build deps explicitly on every
  cargo job rather than relying on the runner image to preinstall
  `protoc` / `libseccomp-dev`.

### Fixed
- **`zlayer-git` reflog failure on machines without global git config.**
  `pull_ff` and `checkout` previously panicked with
  `"reflog messages need a committer which isn't set"` on CI runners
  and inside fresh containers / rootless sandboxes — any environment
  without a `user.name` / `user.email` in `~/.gitconfig` or the
  `GIT_COMMITTER_*` env vars. gix 0.81 is stricter than the `git` CLI
  (which silently falls back to `whoami@hostname`) and refuses
  reflog-writing ref updates without an identity. A new internal
  `ensure_committer` helper installs an in-memory fallback
  (`ZLayer <zlayer@localhost>`) on each opened `Repository` when none
  is configured; applied at every blocking entry point that writes
  reflogs (`fetch`, `pull_ff`, `checkout`). Per-instance mutation via
  `Repository::config_snapshot_mut` — no env vars, no global mutex.
  Caller-supplied identities (user config, env vars) take precedence.

## [0.10.100]

### Added
- **Manager UI `/projects` + `/projects/:id` pages**
  (`crates/zlayer-manager/src/app/pages/projects.rs` and
  `project_detail.rs`). The largest page in the Manager — a list view
  with a nested detail route. List page has a single table (Name, Git
  URL, Branch, Auto-deploy badge, Updated, Actions); row clicks anywhere
  except the Delete button navigate to `/projects/{id}` via
  `use_navigate()`. The **New project** modal drives
  `manager_create_project` with name + git URL + branch + build-kind
  selector (dropdown of `dockerfile | compose | zimagefile | spec`,
  matching the daemon's `BuildKind` exactly) + build path + auto-deploy
  toggle + poll-interval slider (0–3600s, step 30, 0 = disabled).
  Detail page uses the shared `TabBar` with four tabs: **Source**
  (read-only git URL, editable branch, **Pull now** button that drives
  `POST /api/v1/projects/{id}/pull` and shows the resulting
  `ProjectPullResponse` inline plus the truncated HEAD SHA), **Build**
  (segmented build-kind buttons, build path, deploy spec path,
  auto-deploy toggle, poll-interval slider, Save button), **Credentials**
  (merged registry + git credential picker populated from
  `GET /api/v1/credentials/registry` and `/api/v1/credentials/git`;
  attach writes `registry_credential_id` / `git_credential_id` via
  `PATCH /api/v1/projects/{id}`; detach sends the empty string to clear
  — see `update_project` in `handlers/projects.rs` for the empty-string
  semantics), and **Envs & Deployments** (linked-deployments table
  backed by `GET /api/v1/projects/{id}/deployments`, with a **Link
  deployment** name picker and an unlink confirmation modal, plus a
  webhook card showing the template URL + masked secret with a
  Show/Hide toggle and a **Rotate webhook secret** button gated by a
  warning `ConfirmDeleteModal`). Eleven server fns added in
  `app/server_fns.rs`: `manager_list_projects`, `manager_get_project`,
  `manager_create_project`, `manager_update_project`,
  `manager_delete_project`, `manager_pull_project`,
  `manager_list_project_deployments`,
  `manager_link_project_deployment`,
  `manager_unlink_project_deployment`, `manager_get_project_webhook`,
  `manager_rotate_project_webhook`, plus a `manager_list_credentials`
  helper that merges both credential endpoints into a single
  `WireProjectCredential` shape for the picker. Wire types live in
  `crates/zlayer-manager/src/wire/projects.rs`: `WireProject`,
  `WireBuildKind` (four variants matching the daemon's `BuildKind`),
  `WireProjectSpec` (shared create + patch body with
  `skip_serializing_if = "Option::is_none"` everywhere so partial
  patches stay minimal), `WirePullResult`, `WireWebhookInfo` (mirrors
  `WebhookInfoResponse` — URL + secret), `WireProjectCredential`. Nine
  new route smoke tests in `tests/routes_200.rs` cover the full and
  minimum project shapes, all four build-kind wire encodings, the
  `WireProjectSpec` omit-none serialisation, pull-result round-trip,
  webhook-info round-trip, the `parse_wire` helper, and compile-check
  guards for both `Projects` and `ProjectDetail`. Sidebar entry added
  under the existing "Workspace" section using the Heroicons outline
  `folder` icon; routes `/projects` and `/projects/:id` mounted behind
  `<AuthGuard>` in `app/mod.rs`; pages exported via
  `app/pages/mod.rs::{Projects, ProjectDetail}`.
- **Manager UI `/audit` page**
  (`crates/zlayer-manager/src/app/pages/audit.rs`). Admin-only,
  read-only filterable/paginated audit log viewer backed by
  `GET /api/v1/audit`. Filter bar exposes the four daemon-supported
  predicates — user id (free-form, substring match on the server),
  resource kind (free-form string — the Manager deliberately does NOT
  pin this to a dropdown because new kinds get added without a client
  upgrade), since, and until — with "Apply" committing the form into
  the filter signals so typing doesn't spam the daemon. `datetime-local`
  inputs are converted to RFC-3339 UTC at apply time (`YYYY-MM-DDTHH:MM`
  → `YYYY-MM-DDTHH:MM:00Z`) so `chrono::DateTime<Utc>` on the daemon
  can parse them. Pagination is client-side because the daemon's list
  endpoint returns a bare `Vec<AuditEntry>` with no `{entries, total}`
  envelope or `offset` parameter — we over-fetch
  `(page + 1) * PER_PAGE + 1` rows at `limit = <N>` and slice the
  current page out client-side; the trailing "probe row" decides
  whether "Next" should be enabled. `PER_PAGE = 50` is a constant in
  the page module. Row shape: Time (pretty-formatted, full RFC-3339 in
  the `title=` tooltip), User (short 8-char prefix of the user id,
  full id in the tooltip), Action (badge), Resource (`kind:id` when the
  entry has a specific resource id, bare `kind` otherwise), IP, Details
  (collapsed `<details>/<summary>` disclosure showing a pretty-printed
  JSON block; the `user-agent` string is folded into the expanded
  pane). Admin gate at the component level: non-admins see an
  `alert-error` "Admin access required" banner and the page never
  issues a server call; the backend independently enforces admin-only
  via `actor.require_admin()?` in the handler. One server fn added to
  `app/server_fns.rs` — `manager_list_audit(filter: WireAuditFilter)
  -> Vec<WireAuditEntry>` — builds the query string by appending only
  non-empty `Option` fields (so `None` maps to "param omitted", not
  `?key=` which the daemon would deserialize as `Some("")`), with a
  local `pct_encode_query()` helper for RFC 3986 unreserved-set
  encoding of value strings. Wire types (`WireAuditEntry`,
  `WireAuditFilter`) live in `crates/zlayer-manager/src/wire/audit.rs`
  with round-trip tests in `tests/routes_200.rs` covering the full
  shape, explicit-null optionals, missing-optionals-via-serde-default,
  and `WireAuditFilter::default()` round-trip. Page-local unit tests
  cover the datetime-local → RFC-3339 conversion (with + without
  seconds + already-Z-terminated + empty), the pretty-printed
  timestamp formatter, the short-id truncator, the `page_has_next()`
  probe-row logic, and `describe_details()` for the summary preview
  (object / array / primitive / overflow-of-4+ keys). Sidebar entry
  added under the existing "System" section using the Heroicons
  outline `clipboard-document-list` icon, hidden for non-admin
  sessions via the same `CurrentUser::is_admin()` derived class that
  Groups and Permissions use; route `/audit` mounted behind
  `<AuthGuard>` in `app/mod.rs`; page exported via
  `app/pages/mod.rs::Audit`.
- **Manager UI `/permissions` page**
  (`crates/zlayer-manager/src/app/pages/permissions.rs`). Admin-only
  flat-list view of subject→resource access grants. Filter bar narrows
  by subject kind (`all | user | group`), subject id (substring match),
  and resource kind (dropdown of the eight kinds the daemon recognises:
  `deployment | project | secret | task | workflow | notifier |
  variable | environment`), all applied client-side on a union of the
  per-subject grant lists. This approach was dictated by the daemon's
  narrow filter surface (`GET /api/v1/permissions` accepts EXACTLY one
  of `?user=<id>` or `?group=<id>`) — we fan out one request per
  user/group rather than inventing a query parameter the server
  doesn't understand. Grant modal has cascading pickers: flipping the
  subject-kind toggle swaps between a user `<select>` (populated from
  `manager_list_users`, labelled `email (id)`) and a group `<select>`
  (from `manager_list_groups`, labelled `name (id)`), with a free-form
  text fallback if either list hasn't loaded yet. Resource kind is a
  fixed dropdown; resource id is a free-form text input (blank ==
  wildcard grant). Level selector exposes `read | execute | write`
  (the daemon also accepts `none` but a `none` grant is never useful).
  Revoke is an inline confirmation dialog that names the subject /
  resource / level to prevent fat-finger revocations. Admin gate at
  the component level: non-admins see an `alert-warning` "Admin role
  required" banner and no server calls are issued; the backend
  independently enforces admin-only on `POST` and `DELETE`. Three
  server fns added to `app/server_fns.rs` —
  `manager_list_permissions_for_subject`, `manager_grant_permission`,
  `manager_revoke_permission` — all cookie/CSRF-forwarded via
  `raw_request`. Wire types (`WirePermission`,
  `WireGrantPermissionRequest`) live in
  `crates/zlayer-manager/src/wire/permissions.rs` with round-trip
  tests in `tests/routes_200.rs` covering the specific-resource shape,
  the wildcard (`resource_id: null`) shape, and the `skip_serializing_if`
  on the grant request so a wildcard grant's JSON body omits
  `resource_id` entirely (matching the daemon's `#[serde(default)]`
  on `GrantPermissionRequest.resource_id`). Eight unit tests cover
  filter composition, subject/resource label formatting, and the
  fallback-to-raw-id behaviour when a grant's subject was deleted
  after the grant was created. Sidebar entry added under the existing
  "System" section using the Heroicons outline `shield-check` icon,
  hidden for non-admin sessions via the same `CurrentUser::is_admin()`
  derived class the Groups link uses; route `/permissions` mounted
  behind `<AuthGuard>` in `app/mod.rs`; page exported via
  `app/pages/mod.rs::Permissions`.
- **Manager UI `/groups` page**
  (`crates/zlayer-manager/src/app/pages/groups.rs`). Admin-only
  master-detail layout for user group management. Left column is a
  clickable groups table (name + description + Edit/Delete actions).
  Right column shows the selected group's detail: a header card with
  name/description/created timestamp, an "Add member" picker populated
  from `manager_list_users` minus the current membership, and a
  members table with display_name + email + Remove actions. The
  daemon's `GET /api/v1/groups/{id}/members` endpoint returns only
  `user_id` values — names/emails are joined client-side against the
  users resource; unknown ids are rendered verbatim as `(unknown
  user)` rather than dropped. Seven server fns added to
  `app/server_fns.rs`: `manager_list_groups`, `manager_create_group`,
  `manager_update_group` (PATCH, supported by the daemon),
  `manager_delete_group`, `manager_list_group_members`,
  `manager_add_group_member`, `manager_remove_group_member` — all
  cookie/CSRF-forwarded via `raw_request`. Wire types
  (`WireUserGroup`, `WireGroupMembers`, `WireCreateGroup`,
  `WireUpdateGroup`, `WireAddMember`) live in
  `crates/zlayer-manager/src/wire/groups.rs` with round-trip tests in
  `tests/routes_200.rs` (null description, empty members list,
  skip-serializing partial patches). Admin gate at the component
  level — non-admins see an `alert-error` "Admin access required"
  banner and no server calls are issued; the backend independently
  enforces admin-only on every mutation. Sidebar entry added under
  the existing "System" section using the Heroicons outline
  `user-group` icon, hidden entirely for non-admin sessions via a
  `CurrentUser::is_admin()` derived class so regular users don't see
  a link that would only show them an error page; route `/groups`
  mounted behind `<AuthGuard>` in `app/mod.rs`; page exported via
  `app/pages/mod.rs::Groups`.
- **Manager UI `/workflows` page**
  (`crates/zlayer-manager/src/app/pages/workflows.rs`). Master-detail
  layout for the daemon's named step-DAGs: left column is a clickable
  workflow table (name + step count + updated timestamp), right column
  shows the selected workflow's steps as a DaisyUI `steps steps-vertical`
  rail (semantic action badges only — `badge-primary` for run_task,
  `badge-secondary` for build_project, `badge-accent` for deploy_project,
  `badge-info` for apply_sync), a Run button with an in-flight
  spinner/disabled state, and a "Recent runs" panel where each row
  expands inline to reveal the per-step breakdown with `step-success` /
  `step-error` / `step-neutral` coloring keyed on the daemon's
  `StepResult.status` strings (`"ok" | "failed" | "skipped"`). Step
  output/error text renders in a `mockup-code` block beneath each step
  label. The Run button surfaces the finished run in a dedicated
  `RunResultModal` (for a side-by-side view) while ALSO prepending it to
  the history list so the user can keep browsing. Create modal is the
  hardest form in the manager so far — a dynamic `Vec<StepDraft>` backed
  by per-step `RwSignal`s for name, kind selector (run_task /
  build_project / deploy_project / apply_sync), variant-specific id
  field, and optional on_failure task id. Each draft carries a stable
  `key: u64` so move-up / move-down reorders don't re-mount inputs and
  lose focus. On submit the drafts are built into a
  `Vec<WireWorkflowStep>` via per-variant validation that surfaces
  inline as `alert-error` text before hitting the daemon. Edit modal is
  intentionally omitted because the daemon exposes no
  `PATCH /api/v1/workflows/{id}` endpoint (to change a workflow, delete
  and recreate — same pattern Tasks uses), consistent with the "match
  daemon API shapes exactly" ground rule. Six server fns added to
  `app/server_fns.rs` — `manager_list_workflows`, `manager_get_workflow`,
  `manager_create_workflow`, `manager_delete_workflow`,
  `manager_run_workflow`, `manager_list_workflow_runs` — all
  cookie/CSRF-forwarded through `raw_request`. Wire types
  (`WireWorkflow`, `WireWorkflowStep`, `WireWorkflowAction`,
  `WireWorkflowRun`, `WireStepResult`, `WireWorkflowSpec`) live in
  `crates/zlayer-manager/src/wire/workflows.rs` with round-trip tests in
  `tests/routes_200.rs` covering the `{"type": "<snake_case>"}` action
  tagging, null `project_id` on global workflows, and null `finished_at`
  for still-running runs. Sidebar entry added under the existing
  "CI/CD" section using the Heroicons outline `rectangle-stack` icon;
  route `/workflows` mounted behind `<AuthGuard>` in `app/mod.rs`; page
  exported via `app/pages/mod.rs::Workflows`.
- **Manager UI `/tasks` page**
  (`crates/zlayer-manager/src/app/pages/tasks.rs`). Master-detail layout
  for the daemon's named runnable scripts: left column is a clickable
  task table, right column shows the selected task's body, a Run button
  with an in-flight spinner/disabled state, a "Recent runs" table
  (truncated run id + started timestamp + computed duration + colored
  exit-code badge), and a "Latest output" panel that renders the most
  recent run's stdout/stderr in `mockup-code` blocks. Selection is
  stored as an id in a `RwSignal<Option<String>>` so it survives list
  refetches. Runs live in a page-local `RwSignal<Vec<WireTaskRun>>`
  that seeds from `GET /api/v1/tasks/{id}/runs` whenever the selection
  changes and prepends the newly-returned run after each successful
  `POST /api/v1/tasks/{id}/run` call — no polling. Includes a
  dependency-free RFC-3339 parser (`rfc3339_to_unix_ms`,
  `ymd_to_unix_days`) so the WASM/hydrate build doesn't pull in chrono,
  matching the same pattern `secrets.rs` established. Create + Delete
  modals are wired; an Edit modal is intentionally omitted because the
  daemon exposes no `PATCH /api/v1/tasks/{id}` endpoint (to change a
  task, delete and recreate), consistent with the "match daemon API
  shapes exactly" ground rule. Six server fns added to
  `app/server_fns.rs` — `manager_list_tasks`, `manager_get_task`,
  `manager_create_task`, `manager_delete_task`, `manager_run_task`,
  `manager_list_task_runs` — all cookie/CSRF-forwarded through
  `raw_request`. Wire types (`WireTask`, `WireTaskRun`, `WireTaskSpec`)
  live in `crates/zlayer-manager/src/wire/tasks.rs` with round-trip
  tests in `tests/routes_200.rs` (null `project_id`, null
  `exit_code`/`finished_at` for still-running jobs). Sidebar entry
  added under the existing "CI/CD" section using the Heroicons outline
  `play` icon; route `/tasks` mounted behind `<AuthGuard>` in
  `app/mod.rs`; page exported via `app/pages/mod.rs::Tasks`.
- **Manager UI `/secrets` page**
  (`crates/zlayer-manager/src/app/pages/secrets.rs`). Environment-scoped
  secrets management with an env tab filter (All + one per environment,
  including project-scoped envs via `?project=*`), a masked value column
  with per-row reveal that auto-hides after 10 seconds (via
  `gloo_timers::callback::Timeout` on hydrate, no-op on SSR), inline
  create/edit/delete modals, and a bulk `.env` import modal with a
  client-side `KEY=value` parser that mirrors the daemon's quote-stripping
  and `export ` handling. Uses the daemon's existing
  `GET/POST/DELETE /api/v1/secrets`, `GET /api/v1/secrets/{name}?reveal=true`,
  and `POST /api/v1/secrets/bulk-import` endpoints — no new API surface.
  Wire types (`WireSecret`, `WireEnvironment`, `BulkImportResult`) live in
  `crates/zlayer-manager/src/wire/secrets.rs` and are guarded by
  round-trip tests in `tests/routes_200.rs`. Sidebar entry added under the
  "Workspace" section using the Heroicons outline `key` icon; route wired
  at `/secrets` behind `<AuthGuard>`.
- **Manager UI `/variables` page**
  (`crates/zlayer-manager/src/app/pages/variables.rs`). First page of the
  Phase 4-8 Manager UI roll-out: a DaisyUI table of workspace variables
  with modals for creating, editing, and deleting. Edits push through the
  existing `PATCH /api/v1/variables/{id}` endpoint; creates use
  `POST /api/v1/variables` with an optional `scope` id (blank = global).
  Wired into the sidebar under a new "Workspace" section (above "CI/CD")
  and registered at `/variables` behind `<AuthGuard>`; the daemon still
  enforces admin-only on every mutation so non-admins see server-side
  errors rather than a hard client-side gate.
- **Shared Manager UI scaffolding used by the Variables page and every
  subsequent Phase 4-8 page**:
  - `crates/zlayer-manager/src/app/util/errors.rs` — `humanize_error`
    (moved out of `pages/bootstrap.rs`) plus a new `format_server_error`
    convenience wrapper that takes a `ServerFnError` directly. Tests
    cover the 409/bootstrap/network branches.
  - `crates/zlayer-manager/src/app/components/forms.rs` — five reusable
    modal/form primitives: `ConfirmDeleteModal`, `TextFieldModal`,
    `FormField`, `TabBar`, `Pagination`. DaisyUI semantic classes only;
    no hand-picked colors. Reactive signals for the "submitting" flag
    and optional error banner.
  - `crates/zlayer-manager/src/wire/` — new top-level module for wire
    types shared between SSR and hydrate builds. Contains
    `common::ErrorBody` and `variables::WireVariable`. Cannot depend on
    `zlayer-api` because hydrate compiles as WASM and has no access to
    that crate; the round-trip test in `tests/routes_200.rs` guards
    against field drift.
- **Real workflow action execution for `BuildProject`, `DeployProject`, and
  `ApplySync`** (`crates/zlayer-api/src/handlers/workflows.rs`). Previously
  these three `WorkflowAction` arms were log-only — the daemon printed a
  "would execute …" line and returned a placeholder string without touching
  anything. They now drive the real pipeline end-to-end:
  - `BuildProject` looks up the project, ensures the git working copy is
    cloned / fast-forwarded at `{clone_root}/{project_id}` (reusing the
    shared `ensure_project_clone` helper with HTTP pulls), registers a new
    build against the shared `BuildManager`, kicks off `spawn_build` (the
    same code path the `/api/v1/build/json` HTTP endpoint uses), blocks on
    `BuildManager::wait_for_build` for terminal status, and propagates
    `Complete` as step-ok / `Failed` as step-failed so the workflow's
    `on_failure` handler (if any) still gets a chance to run.
  - `DeployProject` requires a new `StoredProject::deploy_spec_path` field;
    when set, it reads that file from the cloned working copy, parses it as
    a `DeploymentSpec` (via `zlayer_spec::from_yaml_str`, exactly like the
    HTTP deployments endpoint and sync apply), upserts the stored
    deployment, and links it to the project so "list project deployments"
    surfaces the new record. When `deploy_spec_path` is `None` the step
    fails with a clear, actionable error message rather than silently
    succeeding.
  - `ApplySync` delegates directly to the shared `apply_sync_inner` helper,
    so workflow-triggered applies produce the same per-resource reconcile
    results (create / update / delete / skip) and the same
    `last_applied_sha` bookkeeping as a `POST /syncs/{id}/apply` HTTP
    request. The step output is the JSON-serialized `SyncApplyResponse` so
    downstream consumers can parse structured results rather than regexing
    a printf'd string.
- **`BuildManager::wait_for_build`** (`crates/zlayer-api/src/handlers/build.rs`).
  New async helper that blocks until a registered build reaches a terminal
  state (`Complete` or `Failed`), polling every 200ms with a hard 1-hour
  ceiling. Returns the final `BuildStatus` for the caller to inspect, or
  `None` when the build id is unknown. The workflow `BuildProject` action
  uses it to turn the existing fire-and-forget `spawn_build` pipeline into
  a synchronous step — no handler duplication, no alternate build code
  path. Covered by two new unit tests: one asserts `Complete` propagates
  from a concurrent writer, one asserts unknown ids return `None`.
- **`WorkflowsState` extended with every handle workflow steps can reach:**
  `project_store`, `deployment_store`, `sync_store`, `build_manager`,
  `clone_root`, and (optional) `git_creds`. The old two-field constructor
  `WorkflowsState::new(store, task_store)` is replaced with the full
  seven-argument form; `serve.rs` hands them through from the existing
  `StorageBundle` + `BuildState` + the shared `{data_dir}/projects` clone
  root (same path `ProjectState` and `SyncState` already resolve, so every
  surface that pulls a project's git repo ends up in the same place).
  A chainable `with_git_creds(...)` method attaches a credential store
  when private repo auth is required for `BuildProject`.
- **`StoredProject::deploy_spec_path: Option<String>`** (`crates/zlayer-api/src/storage/mod.rs`).
  New optional field that tells workflow `DeployProject` which YAML inside
  the cloned working copy to parse and apply. Defaults to `None` (explicit
  opt-in) and is serde `#[serde(default)]` so existing project rows in
  `projects.db` deserialize without a schema migration — the
  `SqlxProjectStore` serializes the whole struct as `data_json` so only
  the JSON shape matters. `CreateProjectRequest` / `UpdateProjectRequest`
  both accept the new field; passing `""` on update clears it.
- **`pub(crate)` helper `ensure_project_clone`** (`crates/zlayer-api/src/handlers/projects.rs`).
  Extracted from the HTTP `pull_project` handler so the workflow
  `BuildProject` action uses the exact same clone / fast-forward / auth
  resolution code path — no divergence between manual `POST /pull` and
  workflow-triggered builds.
- **Manager UI `/notifiers` page**
  (`crates/zlayer-manager/src/app/pages/notifiers.rs`). DaisyUI table of
  configured notifiers with columns for Name, Type (badge colored per
  kind — slack/primary, discord/secondary, webhook/accent, smtp/info),
  Enabled (clickable badge toggles via PATCH), Updated, and Actions
  (Test / Edit / Delete). Establishes the **discriminated-union form
  pattern** that future pages (workflows, syncs) should follow when they
  pick up similarly shaped tagged unions: a single `<select>` drives the
  active variant, variant-specific fields render conditionally, and
  switching variants clears the other variants' inputs so stale state
  never leaks into the POST body. The final `WireNotifierConfig` is
  built via a `match` on the selected variant. A per-row Test modal
  shows a spinner while `POST /api/v1/notifiers/{id}/test` is in flight,
  then surfaces the daemon's `NotifierTestResult` as an
  `alert-success`/`alert-error` with the returned message — the daemon
  returns upstream failures as HTTP 200 + `success: false`, so the UI
  never confuses "reachable but misconfigured" with "daemon unreachable".
  - Five server fns in `app/server_fns.rs`: `manager_list_notifiers`,
    `manager_create_notifier`, `manager_update_notifier`,
    `manager_delete_notifier`, `manager_test_notifier` — all
    cookie/CSRF-forwarded through `raw_request` exactly like the
    Variables fns. `manager_create_notifier` always submits the
    daemon's `{name, kind, config}` tuple with `kind` derived from
    `WireNotifierConfig::kind()`, and immediately follows with a PATCH
    when the caller asked for `enabled: false` (the daemon's create
    handler ignores enabled and always produces `enabled: true`).
  - Wire types in `crates/zlayer-manager/src/wire/notifiers.rs`
    (`WireNotifier`, `WireNotifierConfig` with
    `#[serde(tag = "type", rename_all = "snake_case")]`,
    `NotifierTestResult`). Zero dependency on `zlayer-api` — the
    hydrate/WASM build can consume these the same way it consumes
    `WireVariable`. Tests in `tests/routes_200.rs` round-trip each
    variant (slack, discord, webhook with + without optional fields,
    smtp), exercise the `kind()` helper, and assert
    `NotifierTestResult` parses both success and failure shapes.
  - Route `/notifiers` mounted behind `<AuthGuard>` in
    `app/mod.rs`; sidebar entry with a Heroicons `bell` icon in the
    existing "CI/CD" section of `app/components/sidebar.rs`; page
    exported via `app/pages/mod.rs::Notifiers`.

### Changed
- **`BuildManager::spawn_build` is now `pub(crate)`** so the workflow
  `BuildProject` action can invoke it directly. No behavior change; the
  function was previously a private helper inside the build handler.
- **`serve.rs` constructs `BuildState` and the shared `projects` clone root
  earlier** so `ProjectState`, `SyncState`, and the new `WorkflowsState` all
  reference the same `Arc<BuildManager>` and the same on-disk clone root.
  Before this change `ProjectState` and `SyncState` defaulted to
  `std::env::temp_dir().join("zlayer-projects")`, which survived nothing
  across reboots; they now use `{data_dir}/projects`.

### Fixed
- Workflow runs that included `BuildProject`, `DeployProject`, or `ApplySync`
  steps previously reported `"ok"` with a `"would execute …"` output even
  though nothing actually ran. Admins relying on workflows to automate
  builds / deploys / syncs would see green step results and no error, but
  find their deployments stale and registries empty. Those three arms now
  execute the real action and propagate failures the way every other
  workflow step does.

## [0.10.99]

### Added
- **Persistent `SQLite` backend for audit log storage (`SqlxAuditStore`).**
  Audit entries (who did what, when, to which resource) now survive daemon
  restarts. The new store lives in `crates/zlayer-api/src/storage/audit.rs`
  alongside the existing `InMemoryAuditStore` (kept for unit tests); both back
  the same `AuditStorage` trait so callers are backend-agnostic. Re-exported
  from `storage::mod` as `SqlxAuditStore`.
  - Hand-rolled (not built on `SqlxJsonStore`) because the filter is a
    multi-field range query — `user_id`, `resource_kind`, and `[since, until]`
    on `occurred_at` — rather than the single-column-peel pattern the generic
    adapter is tuned for. `list()` builds its dynamic `WHERE` clause via
    `sqlx::QueryBuilder` with parameterised binds, so filter composition is
    injection-safe.
  - Append-only schema: `audit_log(id PK, user_id, resource_kind, resource_id,
    action, data_json, occurred_at)`. Three supporting indexes —
    `idx_audit_user (user_id, occurred_at DESC)`,
    `idx_audit_resource (resource_kind, occurred_at DESC)`, and
    `idx_audit_time (occurred_at DESC)` — so every common filter shape
    (per-user timeline, per-kind timeline, global recent) serves from an
    ordered index scan rather than an in-memory sort. The canonical
    `AuditEntry` is also stored as `data_json` so new optional fields
    (e.g. `ip`, `user_agent`, `details`) can be added without a schema
    migration.
  - `list()` semantics match `InMemoryAuditStore` exactly: `since`/`until` are
    both inclusive bounds on `occurred_at`, `user_id` and `resource_kind` are
    strict-equality filters, results are ordered by `occurred_at DESC`, and
    `limit` is applied at the SQL level (so a `limit = 5` over a million rows
    stops at 5 without materialising the rest).
  - Seven new `#[tokio::test]` cases cover no-filter reverse-chronological
    ordering, each single-field filter, the inclusive-window semantics (10
    entries spread over 10s, middle 3 captured), the limit cap, the full
    combined filter (user + kind + window with negative cases for each
    axis), and a round-trip through an on-disk database to confirm the
    RFC3339 timestamp, `details` JSON, `ip`, and `user_agent` all survive
    close-and-reopen.
- **Persistent `SQLite` backend for task storage (`SqlxTaskStore`).**
  Named runnable scripts (tasks) and their execution records (task runs) now
  survive daemon restarts. The new store lives in
  `crates/zlayer-api/src/storage/tasks.rs` alongside the existing
  `InMemoryTaskStore` (kept for unit tests); both back the same `TaskStorage`
  trait so callers are backend-agnostic. Re-exported from `storage::mod` as
  `SqlxTaskStore`.
  - Primary `tasks` table managed by `SqlxJsonStore` with two peeled columns:
    `name` (single-column `UNIQUE`, so duplicate task names surface as
    `StorageError::AlreadyExists` directly from the adapter) and `project_id`
    (nullable, indexed) so `list(Some(project_id))` goes through `list_where`
    rather than a full-table scan.
  - Secondary `task_runs` history table, hand-rolled on the shared pool via
    the crate-private `SqlxJsonStore::pool()` accessor, with a composite
    index on `(task_id, started_at DESC)` so `list_runs` serves its
    newest-first ordering as an ordered index scan rather than an in-memory
    sort pass. `record_run` upserts by run id so re-recording the same run
    (e.g. to patch `stdout`/`finished_at` after the process exits) updates
    in place rather than duplicating.
  - `delete` runs a transaction that first wipes the task's runs in
    `task_runs`, then the row in `tasks`, so a crash mid-delete cannot leave
    orphan run rows behind.
- **`SqlxJsonStore::pool()` accessor** (crate-private) on
  `crates/zlayer-api/src/storage/sqlx_json.rs`. Exposes the underlying
  `SqlitePool` to sibling stores inside `storage::*` so they can create
  secondary tables (e.g. `task_runs`) and run cross-table transactions on
  the same pool without opening a second connection. Not part of the public
  API.
- **Persistent `SQLite` backend for user group storage (`SqlxGroupStore`).**
  User groups and their memberships now survive daemon restarts. The new
  store lives in `crates/zlayer-api/src/storage/groups.rs` alongside the
  existing `InMemoryGroupStore` (kept for unit tests); both back the same
  `GroupStorage` trait so callers are backend-agnostic. Re-exported from
  `storage::mod` as `SqlxGroupStore`.
  - Primary `groups` table managed by `SqlxJsonStore` with a single-column
    `UNIQUE(name)` constraint — duplicate-name inserts surface as
    `StorageError::AlreadyExists` straight out of the adapter.
  - Secondary `group_members` link table keyed by a composite
    `PRIMARY KEY (group_id, user_id)`, hand-rolled on the shared pool via
    the crate-private `SqlxJsonStore::pool()` accessor. A secondary
    `idx_group_members_user` index on `(user_id)` powers
    `list_groups_for_user` without a full-table scan. Double-add of the
    same pair is idempotent via `INSERT OR IGNORE`, matching
    `InMemoryGroupStore` which deduplicates through its `HashSet`.
  - `delete` runs a transaction that first wipes the group's membership
    rows in `group_members`, then the row in `groups`, so a crash
    mid-delete cannot leave orphan membership rows behind.
  - `add_member`, `remove_member`, and `list_members` preflight-check
    group existence and return `StorageError::NotFound` for unknown group
    ids, matching the existing `InMemoryGroupStore` behaviour.
- **Persistent `SQLite` backend for workflow storage (`SqlxWorkflowStore`).**
  Workflow definitions and their execution history now survive daemon
  restarts. The new store lives in `crates/zlayer-api/src/storage/workflows.rs`
  alongside the existing `InMemoryWorkflowStore` (kept for unit tests); both
  back the same `WorkflowStorage` trait so callers are backend-agnostic.
  Re-exported from `storage::mod` as `SqlxWorkflowStore`.
  - Primary `workflows` table managed by `SqlxJsonStore` with a single-column
    `UNIQUE(name)` constraint — duplicate-name inserts surface as
    `StorageError::AlreadyExists` straight out of the adapter.
  - Secondary `workflow_runs` history table, hand-rolled on the shared pool
    via the crate-private `SqlxJsonStore::pool()` accessor, with a composite
    index on `(workflow_id, started_at DESC)` so `list_runs` is an ordered
    index scan rather than a fetch-plus-sort-in-memory pass.
  - `delete` runs a transaction that first wipes the workflow's run history
    in `workflow_runs`, then the row in `workflows`, so a crash mid-delete
    cannot leave orphan run rows. The `InMemoryWorkflowStore` backend was
    updated to cascade-delete run history too, matching the new contract.
- **Persistent `SQLite` backend for variable storage (`SqlxVariableStore`).**
  Plaintext template variables now survive daemon restarts. The new store
  lives in `crates/zlayer-api/src/storage/variables.rs` alongside the
  existing `InMemoryVariableStore` (kept for unit tests); both back the same
  `VariableStorage` trait so callers are backend-agnostic. Re-exported from
  `storage::mod` as `SqlxVariableStore`.
- **Compound `UNIQUE` support in `JsonTable`.** `JsonTable<T>` gained a
  `unique_constraints: &'static [&'static [&'static str]]` field; each inner
  slice becomes a `UNIQUE(col_a, col_b, ...)` clause in the generated
  `CREATE TABLE` statement. This enables multi-column uniqueness (e.g.
  `UNIQUE(name, scope)` for variables) that `IndexSpec::unique` — a
  single-column flag — cannot express. Compound-constraint violations still
  surface as `StorageError::AlreadyExists` via the existing
  `map_sqlx_err` path.
- **Three new `SqlxJsonStore` lookup methods:**
  `list_where(column, value)` for `column = ?` filtering,
  `list_where_null(column)` for `column IS NULL` filtering, and
  `list_where_opt(column, Option<&str>)` as the unified primitive that
  accepts either. All three reject unknown columns with `StorageError::Other`
  (not a raw `Database` string) by validating against the static index list,
  matching the `get_by_unique` contract. Results are ordered by `id` ASC.
  The variable store uses these to implement its `list(scope)` and
  `get_by_name(name, scope)` trait methods without full-table scans.
- **Scope-isolation tests for `SqlxVariableStore`.** `(name, scope)`
  uniqueness is enforced by the schema for rows with non-`NULL` scope and by
  an application-level pre-write `get_by_name` check for global rows
  (SQLite treats `NULL` as distinct in `UNIQUE`, so the schema alone can't
  cover the `scope = None` case). Persistent round-trip across database
  reopen is covered.

### Changed
- **Sync apply is now a real reconcile (breaking change to
  `SyncApplyResponse`).** `POST /api/v1/syncs/{id}/apply` no longer returns a
  dry-run diff — it actually upserts `DeploymentSpec` manifests into the
  deployment store and, when `sync.delete_missing == true`, deletes
  deployments that are no longer present in the manifest directory.
  - The response shape is now
    `{ results: [SyncResourceResult], applied_sha?, summary }` where each
    `SyncResourceResult` is `{ resource, kind, action, status, error? }`
    with `action` in `{create, update, delete, skip}` and `status` in
    `{ok, error}`. Per-resource errors are surfaced on the individual
    result rather than aborting the whole apply.
  - `SyncState` now carries an `Arc<dyn DeploymentStorage>` alongside the
    sync store and clone root. `SyncState::new` takes the deployment store
    as a second argument; `bin/zlayer::commands::serve` wires it from the
    same persistent `SqlxStorage` used by the deployment routes.
  - `CreateSyncRequest` gains an optional `delete_missing` field so syncs
    can be created with destructive reconcile enabled up front; the default
    is still `false` (the safer behaviour).
  - On successful apply the sync's `last_applied_sha` is refreshed from
    `zlayer_git::current_sha` on the scan directory and persisted, so
    `zlayer sync ls` reflects the commit actually applied.
  - Standalone `kind: job` and `kind: cron` YAMLs are recorded as
    `action: skip` with a message explaining that job/cron records should
    be scheduled via `DeploymentSpec` (no standalone `JobStorage` /
    `CronStorage` exists on the API yet). Unknown kinds are skipped with
    `"unknown kind: {kind}"`. This replaces silent no-ops with visible
    skips so the caller sees what was reconciled and what wasn't.
  - The core apply logic is extracted as
    `handlers::syncs::apply_sync_inner(..)` (crate-private) so the
    upcoming workflow `ApplySync` action (Fix 3) can invoke the same
    reconcile path without going back out through HTTP.
  - `bin/zlayer::commands::sync_cmd::apply` now tabulates
    `results` per-row with action/status/kind/resource/detail columns and
    prints the `summary` and `applied_sha` headers. The old
    `diff.to_create / to_update / to_delete` summary is gone.
  - Six new tests in `crates/zlayer-api/src/handlers/syncs.rs` cover
    create, update, `delete_missing = false` → skip, `delete_missing = true`
    → delete, unknown kind → skip, and `kind: job` → skip-with-message,
    all driven through `apply_sync_inner` against in-memory sync +
    deployment stores.
- Existing `JsonTable` construction sites (`NOTIFIERS_TABLE`, `SYNCS_TABLE`,
  plus in-module test fixtures) now populate `unique_constraints: &[]`.
  No functional change — just the new field default. Downstream callers
  outside the workspace should do the same.
- **`SyncStorage` trait error type harmonised from `Result<_, String>` to
  `Result<_, StorageError>`.** Every method on the trait
  (`list`/`get`/`create`/`delete`/`update`) now returns the same structured
  error type used by every other storage trait in the crate, so the sync
  store is no longer the odd one out. The in-memory implementation maps its
  duplicate-name and missing-id cases to `StorageError::AlreadyExists` and
  `StorageError::NotFound` respectively — and the new persistent backend
  does the same — so HTTP handlers can rely on a single conversion path.
  This is a BREAKING change at the trait level but internal to the
  workspace; the only caller (`crates/zlayer-api/src/handlers/syncs.rs`)
  was updated in the same patch to drop its `.map_err(ApiError::Internal)` /
  `.map_err(ApiError::Conflict)` noise in favour of the new
  `From<StorageError> for ApiError` conversion, which preserves HTTP status
  semantics (`NotFound` → 404, `AlreadyExists` → 409).
- The `'static` bound on `SyncStorage` was dropped to match the other
  storage traits — it was defensive, not actually required by any caller.

### Added
- **Persistent `SQLite` backend for sync storage (`SqlxSyncStore`).**
  Sync records (git-backed resource-set pointers) now survive daemon
  restarts. The new store lives in `crates/zlayer-api/src/storage/syncs.rs`
  alongside the existing `InMemorySyncStore` (kept for unit tests); both
  back the same `SyncStorage` trait so callers are backend-agnostic.
  Re-exported from `storage::mod` and `zlayer_api::storage` as
  `SqlxSyncStore`.
  - Schema: `name` is a single-column `UNIQUE` peeled column (global name
    uniqueness enforced at the schema level); `project_id` is a nullable
    secondary index for fast project-scoped lookups.
  - Preflight name check on `create` yields a clean `StorageError::
    AlreadyExists` even before the adapter's upsert machinery runs; the
    schema-level UNIQUE is the backstop.
- **`StoredSync.delete_missing: bool` field**, defaulted to `false` via
  `#[serde(default)]` so existing serialised blobs without the field
  deserialise as "skip deletes" (the safer default). This is the
  per-record toggle that the upcoming real sync-apply implementation (Fix
  5) reads to decide whether resources present on the API but missing
  from the manifest should be deleted. Fresh syncs default to `false`;
  `StoredSync::new(...)` was updated to set the field explicitly.
- **`From<StorageError> for ApiError`** in `crates/zlayer-api/src/error.rs`.
  Maps `NotFound` → `ApiError::NotFound`, `AlreadyExists` → `Conflict`,
  `Serialization`/`Other`/`Database`/`Io` → `Internal` with descriptive
  prefixes. Callers can now just `.await?` on any `StorageError`-returning
  store method and get the correct HTTP status.
- **Persistent `SQLite` backend for permission storage (`SqlxPermissionStore`).**
  Resource-level permission grants now survive daemon restarts. The new store
  lives in `crates/zlayer-api/src/storage/permissions.rs` alongside the
  existing `InMemoryPermissionStore` (kept for unit tests); both back the same
  `PermissionStorage` trait so callers are backend-agnostic. Re-exported from
  `storage::mod` as `SqlxPermissionStore`.
  - Hand-rolled schema (NOT built on `SqlxJsonStore`) because `check()`
    requires SQL-side wildcard matching (`resource_id IS NULL`) and a
    `MAX(level)` aggregate across matching rows — neither is expressible
    through the generic JSON-table adapter. All five query columns
    (`subject_kind`, `subject_id`, `resource_kind`, `resource_id`, `level`)
    are first-class indexed SQLite columns. The full record is additionally
    serialized into `data_json` so `StoredPermission` can grow new fields
    without schema migrations.
  - Three covering indexes: `idx_perm_subject (subject_kind, subject_id)`
    for `list_for_subject`, `idx_perm_resource (resource_kind, resource_id)`
    for `list_for_resource`, and `idx_perm_check (subject_kind, subject_id,
    resource_kind, resource_id)` so the hot-path `check()` aggregate runs as
    an index scan instead of a table scan.
  - `grant()` is an upsert on the `id` primary key (same as
    `InMemoryPermissionStore` which keys by `id` in its `HashMap`) — two
    distinct ids with the same (subject, resource) coexist, and `check()`
    resolves them via `MAX(level)`. Deliberately no unique constraint on
    (subject, resource); re-granting with the same id replaces every field
    on the existing row.
  - `check()` is a single `SELECT MAX(level) WHERE subject_kind=? AND
    subject_id=? AND resource_kind=? AND (resource_id IS NULL OR
    resource_id=?)` that folds wildcard and exact-id grants in one round
    trip. `SubjectKind` serializes as `"user"`/`"group"` TEXT (matching the
    `Display` impl and `serde` wire form); `PermissionLevel` is stored as a
    small INTEGER (None=0, Read=1, Execute=2, Write=3) so comparisons work
    at the SQL layer.

### Fixed
- Cleaned up two `clippy::doc_markdown` hits and one `clippy::single_match_else`
  in `storage/sqlx_json.rs` introduced by the variable-store patch — the
  workspace now passes `cargo clippy --workspace --all-targets --
  -D warnings` again.
- **`zlayer serve` wires every persistent API store through a single
  `StorageBundle`** (`bin/zlayer/src/commands/serve.rs`). The bundle opens
  11 `SQLite` databases under `--data-dir` — `users.db`, `environments.db`,
  `projects.db`, `variables.db`, `tasks.db` (+ task runs), `workflows.db`
  (+ workflow runs), `notifiers.db`, `groups.db` (+ group memberships),
  `permissions.db`, `audit.db`, `syncs.db` — and feeds each handler its
  corresponding store. Prior to this change `zlayer serve` constructed
  `InMemory*Store::new()` for every resource class, so notifier configs,
  variables, tasks, workflows, groups, permissions, audit entries, and
  syncs were wiped on every daemon restart. Users, environments, and
  projects had no routes mounted at all. All of that now lives on disk
  and survives restart. The deployment database (`deployments.db`) stays
  in `init_daemon` because the S3 `SqliteReplicator` is bound to that
  exact path and the deployment-restore path depends on the concrete
  `Arc<SqlxStorage>` type — documented inline on `StorageBundle`. A
  `StorageBundle::open(None)` in-memory fallback is preserved for tests
  and exercises the same sqlx schema / query path as the file-backed
  variant (just with `:memory:`). Two new `#[tokio::test]` cases cover
  a full restart round-trip (write one record per store, drop the
  bundle, reopen at the same path, assert every record is still there)
  and the in-memory fallback. `--data-dir` defaults to `~/.zlayer` on
  macOS and `/var/lib/zlayer` (root) or `~/.zlayer` (user) on Linux.
- **`ApiConfig` grows a `user_store` handle** (`crates/zlayer-api/src/config.rs`).
  The three router builders that derive `AuthState` internally
  (`build_router_with_storage`, `build_router_with_services`,
  `build_router_with_deployment_state`) now propagate it, so
  `/api/v1/users` and `/auth/bootstrap`/`/auth/login` finally have a
  backing store when the daemon wires one in. The old behaviour
  (`user_store: None` hardcoded in the three builders) was the reason
  the Manager UI saw 500s on user management despite the store being
  implemented. `ApiConfig` drops its `#[derive(Debug)]` in favour of a
  manual impl that elides the `jwt_secret` and reports the two store
  handles as `bool`s — trait objects can't be derived-Debug, and we
  didn't want to slap a `Debug` supertrait on every storage trait.
- **Re-exports for the seven `Sqlx*Store` types that were hidden inside
  `zlayer_api::storage`** (`crates/zlayer-api/src/lib.rs`): `SqlxAuditStore`,
  `SqlxGroupStore`, `SqlxNotifierStore`, `SqlxPermissionStore`,
  `SqlxTaskStore`, `SqlxVariableStore`, `SqlxWorkflowStore`. Consumers that
  wanted to open one of these from outside the crate previously had to
  reach into `storage::...` by full path; now they're flat under
  `zlayer_api::` like the other three Sqlx stores. No API change beyond
  the re-exports.

## [0.10.98]

### Added
- **Persistent `SQLite` backend for notifier storage (`SqlxNotifierStore`).**
  Notifier configurations (Slack, Discord, webhook, SMTP) now survive daemon
  restarts. The new store lives in
  `crates/zlayer-api/src/storage/notifiers.rs` alongside the existing
  `InMemoryNotifierStore` (kept for unit tests); both back the same
  `NotifierStorage` trait so callers are backend-agnostic.
- **Generic `SqlxJsonStore<T>` adapter**
  (`crates/zlayer-api/src/storage/sqlx_json.rs`). A reusable blob-and-indexes
  persistence engine for resources with the "opaque JSON body + zero or more
  peeled-out unique/indexed columns" shape. Callers supply a `JsonTable<T>`
  describing the table name and a static list of `IndexSpec<T>` extractor
  functions; the adapter handles schema creation (`CREATE TABLE`, secondary
  indexes, table-level `UNIQUE(...)`), upserts with `created_at` preservation,
  `get`/`list`/`delete`/`count`, and `get_by_unique` lookups. This is the
  foundation for the remaining persistent stores (variables, tasks, workflows,
  groups, syncs) that will follow in subsequent patches.
- Two new `StorageError` variants: `AlreadyExists` (surfaced on `SQLite`
  2067 / 1555 constraint violations so callers can distinguish 409 from 500)
  and `Other` (generic miscellaneous error, e.g. unknown column in
  `get_by_unique`).

### Changed
- `NotifierStorage` implementations are now re-exported from
  `storage::mod` as `{InMemoryNotifierStore, NotifierStorage,
  SqlxNotifierStore}` (previously only the in-memory variant was exposed).
  No call-site changes required; the new type is additive.

## [0.10.97]

### Added
- **SMTP notifier sending is now fully implemented.** The
  `POST /api/v1/notifiers/{id}/test` endpoint no longer returns
  `501 Not Implemented` for `NotifierConfig::Smtp`; it connects to the
  configured SMTP server, authenticates, and delivers a real email via
  [`lettre`](https://docs.rs/lettre/0.11/). Upstream failures (bad
  credentials, unreachable host, invalid `from`/`to` addresses) are
  surfaced as HTTP 200 with `success: false` and a descriptive message,
  matching the convention already used by the Slack/Discord/Webhook arms.
  - Internals: a new `send_smtp_message(&SmtpParams, subject, body)`
    helper in `crates/zlayer-api/src/handlers/notifiers.rs` handles
    mailbox parsing, message construction, and transport setup, so any
    future caller that needs to send email can reuse it.
  - Transport features: `AsyncSmtpTransport::<Tokio1Executor>` with
    `lettre`'s `relay()` constructor (implicit TLS on submission ports,
    or STARTTLS as auto-negotiated). No OpenSSL linkage — TLS goes
    through `rustls`, matching the rest of the workspace.
  - New workspace dependency: `lettre = "0.11"` with
    `default-features = false` and features `smtp-transport`,
    `tokio1-rustls-tls`, `builder`, `hostname`. See the inline comment
    in `Cargo.toml` for the rationale.
  - Tests: four new `#[tokio::test]` cases cover the empty-recipient,
    invalid-`from`, unreachable-host, and end-to-end-via-`send_test_notification`
    failure paths, all without requiring a live SMTP server.
  - Refactor: the Slack/Discord/Webhook arms of `send_test_notification`
    have been extracted into `send_slack_test`, `send_discord_test`, and
    `send_webhook_test` helpers. Behaviour is unchanged; the dispatcher
    is now a thin `match` over `NotifierConfig`.

## [0.10.96]

### Changed
- **`zlayer-git` now uses `gix` (gitoxide) instead of shelling out to the
  `git` CLI.** All public operations (`clone_repo`, `fetch`, `pull_ff`,
  `rev_parse`, `current_sha`, `checkout`, `ls_remote`) are now implemented
  in-process via the pure-Rust `gix = "0.81"` crate. Blocking gix work is
  dispatched through `tokio::task::spawn_blocking` so the async API is
  preserved byte-for-byte and none of the 9 callers in `zlayer-api` had to
  change.
  - HTTPS PAT authentication is installed via
    `gix::remote::Connection::with_credentials`, returning an `Outcome`
    that carries the username/token directly. Tokens never touch the
    filesystem and never appear on a command line.
  - SSH authentication writes the PEM key to a mode-`0600` `TempDir` and
    sets `GIT_SSH_COMMAND` for the duration of the operation, guarded by
    an internal `Mutex` to serialise concurrent SSH operations.
  - `ls_remote` uses a throwaway bare repo (`gix::init_bare`) as a scratch
    context for the remote listing, then inspects `ref_map.remote_refs`.
  - `checkout` and `pull_ff` rebuild the worktree by calling
    `gix::worktree::state::checkout` with the target tree's index, then
    delete worktree files that existed previously but are absent from the
    new tree so that going backwards in history no longer leaves stale
    files behind.
  - New tests `ls_remote_against_local_bare` and
    `gix_native_clone_reads_gix_init_bare` exercise the gix-native code
    paths directly.
  - Runtime dependency: `gix = "0.81"` (workspace dep) with features
    `blocking-network-client`, `blocking-http-transport-reqwest-rust-tls`,
    `credentials`, `worktree-mutation`, `revision`, `sha1`. The `tempfile`
    runtime dependency is retained in `zlayer-git` because the SSH path
    still needs an on-disk key with a stable lifetime.

## [0.10.95]

### Added
- **Groups, permissions, and audit** (Phase 8.2): resource-level access
  control and audit logging for the REST API and CLI.
  - **Groups**: user group CRUD and membership management.
    - `GroupsState` handler state backed by `Arc<dyn GroupStorage>`.
    - Seven REST endpoints:
      - `GET    /api/v1/groups` -- list (any auth)
      - `POST   /api/v1/groups` -- create (admin)
      - `GET    /api/v1/groups/{id}` -- get (any auth)
      - `PATCH  /api/v1/groups/{id}` -- update name/description (admin)
      - `DELETE /api/v1/groups/{id}` -- delete (admin)
      - `POST   /api/v1/groups/{id}/members` -- add member (admin)
      - `DELETE /api/v1/groups/{id}/members/{user_id}` -- remove member (admin)
    - `zlayer group` CLI subcommands: `list`, `create`, `delete`,
      `member add`, `member remove`.
    - Daemon client methods: `list_groups`, `create_group`, `delete_group`,
      `add_group_member`, `remove_group_member`.
  - **Permissions**: resource-level permission grant/revoke/list.
    - `PermissionsState` handler state backed by `Arc<dyn PermissionStorage>`
      and `Arc<dyn GroupStorage>` (validates group existence on grant).
    - Three REST endpoints:
      - `GET    /api/v1/permissions?user={id}|group={id}` -- list (any auth)
      - `POST   /api/v1/permissions` -- grant (admin)
      - `DELETE /api/v1/permissions/{id}` -- revoke (admin)
    - `zlayer permission` CLI subcommands: `list`, `grant`, `revoke`.
    - Daemon client methods: `list_permissions`, `grant_permission`,
      `revoke_permission`.
  - **Audit**: audit log query endpoint and recording middleware.
    - `AuditState` handler state backed by `Arc<dyn AuditStorage>`.
    - One REST endpoint:
      - `GET /api/v1/audit?user=&resource_kind=&since=&until=&limit=`
        -- list (admin)
    - `zlayer audit tail` CLI subcommand with `--user`, `--resource`,
      `--limit`, `--output` options.
    - Daemon client method: `list_audit`.
    - `audit_middleware`: Axum middleware that records an `AuditEntry` for
      every mutating request (POST/PUT/PATCH/DELETE) that succeeds (2xx).
      Extracts actor from `AuthActor` extension, derives resource kind and
      id from the URL path.
  - OpenAPI documentation for all new endpoints, schemas, and tags
    (Groups, Permissions, Audit).

## [0.10.94]

### Added
- **Notifiers resource** (Phase 7d): configurable notification channels that
  fire alerts to Slack, Discord, generic webhooks, or SMTP endpoints.
  - `StoredNotifier` type in `zlayer-api::storage`: UUID id, name, kind,
    config, enabled flag, timestamps.
  - `NotifierKind` enum: `Slack`, `Discord`, `Webhook`, `Smtp`.
  - `NotifierConfig` tagged enum: channel-specific settings (webhook URL,
    SMTP host/port/credentials/recipients, custom HTTP method/headers).
  - `NotifierStorage` trait + `InMemoryNotifierStore` implementation.
  - Six REST endpoints:
    - `GET    /api/v1/notifiers` -- list (any auth)
    - `POST   /api/v1/notifiers` -- create (admin)
    - `GET    /api/v1/notifiers/{id}` -- get (any auth)
    - `PATCH  /api/v1/notifiers/{id}` -- update (admin)
    - `DELETE /api/v1/notifiers/{id}` -- delete (admin)
    - `POST   /api/v1/notifiers/{id}/test` -- send test notification (admin)
  - Test endpoint actually fires HTTP requests to Slack/Discord/Webhook
    URLs via `reqwest`. SMTP returns 501 (deferred).
  - `zlayer notifier` CLI subcommands: `list`, `create`, `test`, `delete`
    -- dispatched to the daemon via Unix socket.
  - Daemon client notifier methods: `list_notifiers`, `create_notifier`,
    `test_notifier`, `delete_notifier`.
  - OpenAPI documentation for all notifier endpoints and schemas.

## [0.10.93]

### Added
- **Workflows resource** (Phase 7c): named DAGs of steps that compose tasks,
  project builds, deploys, and sync applies.
  - `StoredWorkflow` type in `zlayer-api::storage`: UUID id, name, ordered
    steps list, optional project_id, timestamps.
  - `WorkflowStep` struct: step name, action, optional `on_failure` handler.
  - `WorkflowAction` tagged enum: `RunTask`, `BuildProject`, `DeployProject`,
    `ApplySync`.
  - `WorkflowRun` type: recorded execution with per-step results and overall
    status.
  - `WorkflowRunStatus` enum: `Pending`, `Running`, `Completed`, `Failed`.
  - `StepResult` type: step name, status (`ok`/`failed`/`skipped`), output.
  - `WorkflowStorage` trait + `InMemoryWorkflowStore` implementation.
  - Six REST endpoints:
    - `GET    /api/v1/workflows` -- list (any auth)
    - `POST   /api/v1/workflows` -- create (admin)
    - `GET    /api/v1/workflows/{id}` -- get (any auth)
    - `DELETE /api/v1/workflows/{id}` -- delete (admin)
    - `POST   /api/v1/workflows/{id}/run` -- execute sequentially (admin)
    - `GET    /api/v1/workflows/{id}/runs` -- list past runs (any auth)
  - Sequential execution: `RunTask` steps execute tasks via `sh -c`;
    `BuildProject`, `DeployProject`, `ApplySync` log but do not execute
    real operations (v1 limitation).
  - On-failure handling: if a step fails and defines `on_failure`, the
    referenced task runs before the workflow aborts; remaining steps are
    marked as `skipped`.
  - `zlayer workflow` CLI subcommands: `list`, `create`, `run`, `logs`,
    `delete` -- dispatched to the daemon via Unix socket.
  - Daemon client workflow methods: `list_workflows`, `create_workflow`,
    `run_workflow`, `list_workflow_runs`, `delete_workflow`.
  - OpenAPI documentation for all workflow endpoints and schemas.

## [0.10.92]

### Added
- **Tasks resource** (Phase 7b): named runnable scripts that can be executed
  on demand with output streaming.
  - `StoredTask` type in `zlayer-api::storage`: UUID id, name, kind (bash),
    body, project_id (optional), timestamps.
  - `TaskKind` enum (`Bash`).
  - `TaskRun` type: recorded execution with exit code, stdout, stderr,
    start/end timestamps.
  - `TaskStorage` trait + `InMemoryTaskStore` implementation.
  - Six REST endpoints:
    - `GET    /api/v1/tasks[?project_id=...]` -- list (any auth)
    - `POST   /api/v1/tasks` -- create (admin)
    - `GET    /api/v1/tasks/{id}` -- get (any auth)
    - `DELETE /api/v1/tasks/{id}` -- delete (admin)
    - `POST   /api/v1/tasks/{id}/run` -- execute synchronously (admin)
    - `GET    /api/v1/tasks/{id}/runs` -- list past runs (any auth)
  - `zlayer task` CLI subcommands: `list`, `create`, `run`, `logs`, `delete` --
    dispatched to the daemon via Unix socket.
  - Daemon client task methods: `list_tasks`, `create_task`, `get_task`,
    `run_task`, `list_task_runs`, `delete_task`.
  - OpenAPI documentation for all task endpoints and schemas.
  - `ApiError::NotImplemented` variant for 501 responses on non-Unix
    platforms.

## [0.10.91]

### Added
- **Variables resource** (Phase 7a): plaintext shared key-value pairs for
  template substitution in deployment specs. Variables are NOT encrypted
  (unlike secrets) and are fully visible in API responses.
  - `StoredVariable` type in `zlayer-api::storage`: UUID id, name, value,
    scope (project id or global), timestamps.
  - `VariableStorage` trait + `InMemoryVariableStore` implementation.
  - Five REST endpoints:
    - `GET    /api/v1/variables?scope={project_id}` -- list (any auth)
    - `POST   /api/v1/variables` -- create (admin)
    - `GET    /api/v1/variables/{id}` -- get (any auth)
    - `PATCH  /api/v1/variables/{id}` -- update (admin)
    - `DELETE /api/v1/variables/{id}` -- delete (admin)
  - `zlayer variable` CLI subcommands: `list`, `set`, `get`, `unset` --
    dispatched to the daemon via Unix socket.
  - Daemon client variable methods: `list_variables`, `create_variable`,
    `get_variable`, `update_variable`, `delete_variable`.
  - OpenAPI documentation for all variable endpoints and schemas.

## [0.10.90]

### Added
- **GitOps sync module** (`zlayer_git::sync`): `scan_resources()` walks a
  directory for YAML files and parses each into a `SyncResource` (kind +
  name + raw content).  `compute_diff()` compares local resources against
  remote names to produce a `SyncDiff` (to_create / to_update / to_delete).
- **Sync REST API** (`handlers/syncs.rs`): five endpoints for sync CRUD +
  diff + apply:
  - `GET  /api/v1/syncs` -- list all syncs
  - `POST /api/v1/syncs` -- create a sync
  - `GET  /api/v1/syncs/{id}/diff` -- compute diff
  - `POST /api/v1/syncs/{id}/apply` -- dry-run apply (v1)
  - `DELETE /api/v1/syncs/{id}` -- delete a sync
- **In-memory sync store** (`InMemorySyncStore`): `SyncStorage` trait +
  `Arc<RwLock<HashMap>>` implementation for v1 persistence.
- **`StoredSync` type** in `zlayer-api::storage`: UUID id, name,
  project_id, git_path, auto_apply, last_applied_sha, timestamps.
- **`zlayer sync` CLI subcommands**: `ls`, `create`, `diff`, `apply`,
  `delete` -- dispatched to the daemon via Unix socket.
- **Daemon client sync methods**: `list_syncs`, `create_sync`,
  `diff_sync`, `apply_sync`, `delete_sync`.
- **OpenAPI documentation** for all sync endpoints and schemas.

## [0.10.89]

### Added
- **Per-project git poller** (`GitPoller` in `zlayer-api::poller`).
  A background task that periodically checks `git ls-remote` for new
  commits on projects with `poll_interval_secs` set.  When new commits
  are detected the local working copy is fast-forward pulled; when
  `auto_deploy` is also true, the intent to rebuild/redeploy is logged
  (actual build trigger integration comes in a later phase).
- **`auto_deploy` and `poll_interval_secs` fields on `StoredProject`**
  (`#[serde(default)]` for backward-compatible deserialization of
  existing records).  Both fields are exposed on `CreateProjectRequest`
  and `UpdateProjectRequest`.
- **`git ls-remote` helper** (`zlayer_git::ls_remote`) queries the
  remote for a branch's HEAD SHA without cloning or fetching.
- **`zlayer project auto-deploy <id> --enabled true|false`** CLI
  subcommand to toggle the auto-deploy flag via PATCH.
- **`zlayer project poll-interval <id> --seconds N`** CLI subcommand
  to set (or clear with `--seconds 0`) the polling interval via PATCH.

## [0.10.88]

### Added
- **Git push webhook receiver** (`POST /webhooks/{provider}/{project_id}`)
  -- unauthenticated endpoint that verifies HMAC signatures (GitHub,
  Gitea, Forgejo) or constant-time token comparison (GitLab) against a
  per-project webhook secret stored in `PersistentSecretsStore` under
  scope `"webhook_secrets"`. On success, triggers the same clone /
  fast-forward-pull logic as `POST /api/v1/projects/{id}/pull`. Returns
  `{ "status": "ok", "sha": "..." }`.
- **`GET /api/v1/projects/{id}/webhook`** (auth-required) returns the
  webhook URL template and HMAC secret. Generates the secret on first
  call if it does not exist.
- **`POST /api/v1/projects/{id}/webhook/rotate`** (admin-only)
  regenerates the webhook secret, invalidating the old one.
- **`WebhookState`** struct and `build_project_webhook_routes()` /
  `build_webhook_receiver_routes()` router builders in `zlayer-api`.
- **`DaemonClient::get_project_webhook()` and `rotate_project_webhook()`**
  methods for the CLI to talk to the new endpoints.
- **`zlayer project webhook show <id>` / `zlayer project webhook rotate <id>`**
  CLI subcommands.
- **`WebhookResponse` and `WebhookInfoResponse` schemas** registered in
  the OpenAPI doc under a new "Webhooks" tag.

### Changed
- `zlayer-api` now depends on `hmac` (workspace dep) for HMAC-SHA256
  signature verification.

## [0.10.87]

### Added
- **`POST /api/v1/projects/{id}/pull` endpoint** (admin-only) that wires
  the `zlayer-git` crate into the Projects handler. Clones into
  `{clone_root}/{project_id}` on first call, then fast-forward pulls on
  subsequent calls. Resolves authentication from the project's
  `git_credential_id` via `GitCredentialStore` (PAT or SSH key), falling
  back to anonymous when unset or the store is not configured. Returns
  `{ project_id, git_url, branch, sha, path }`.
- **`ProjectState::with_git(store, git_creds, clone_root)`** constructor on
  `zlayer-api`'s project state. The existing `ProjectState::new(store)`
  constructor still works and leaves `git_creds` unset / defaults
  `clone_root` to `$TMPDIR/zlayer-projects`, so callers that don't need
  pulls are unchanged.
- **`ProjectPullResponse` schema** registered in the OpenAPI doc.
- **`DaemonClient::project_pull(&id)` method** posting to the new endpoint
  and returning the parsed JSON response.
- **`zlayer project pull <id>` CLI command** that triggers a pull and
  prints the repo, branch, SHA, and working-copy path.

### Changed
- `zlayer-api` now depends on `zlayer-git` so the Projects handler can
  clone and pull directly without going through a separate service.

## [0.10.86]

### Added
- **`zlayer project` CLI command group** with full CRUD: `ls`, `create`,
  `show`, `update`, `delete`, plus deployment linking (`link-deployment`,
  `unlink-deployment`, `list-deployments`). 8 new Clap variants total.
- **`zlayer credential` CLI command group** with two nested subcommands:
  `registry` (ls, add, delete) and `git` (ls, add, delete). 6 new Clap
  variants total. Passwords/tokens are prompted interactively via
  `dialoguer` when omitted from the command line.
- **14 `DaemonClient` methods** for projects and credentials, using
  `serde_json::Value` to avoid pulling server-only handler types into the
  CLI binary.

## [0.10.85]

### Added
- **`AuthSource::SecretStore` variant** in `zlayer-core` auth resolver.
  Allows per-registry auth configs to reference a stored credential by id.
  The sync resolver returns `Anonymous` with a warning; callers needing
  the credential must use the new async resolver.
- **`resolve_registry_auth_async()` free function** in `zlayer-api::auth`.
  Resolves all `AuthSource` variants including `SecretStore`, performing
  an async lookup against a `RegistryCredentialStore<S>`.
- **`AuthResolver::source_for_registry()` and public `resolve_source()`**
  accessors on the sync resolver, enabling external async resolution
  without duplicating per-registry lookup logic.

## [0.10.84]

### Added
- **Project CRUD endpoints** at `/api/v1/projects`. List, create, fetch,
  update (PATCH), and delete projects. Mutating endpoints require `admin`.
  Deletion cascade-removes deployment links. Sub-routes for deployment
  linking: `GET/POST /{id}/deployments`, `DELETE /{id}/deployments/{name}`.
  8 endpoints total.
- **Credential management endpoints** at `/api/v1/credentials`. Registry
  credentials (list, create, delete) and git credentials (list, create,
  delete) share a combined handler. All mutation requires `admin`.
  6 endpoints total. Secrets are stored encrypted; only metadata is
  returned in responses.
- **`build_project_routes(project_state)` and
  `build_credential_routes(credential_state)` router helpers** for
  composing project and credential routes into existing routers.
- `ProjectState` and `CredentialState` re-exported from `lib.rs`.
- OpenAPI schemas for `StoredProject`, `BuildKind`, project request types,
  and credential request/response types registered in the API doc.

## [0.10.83]

### Added
- **Environment CRUD endpoints** at `/api/v1/environments`. List, create,
  fetch, rename/re-describe (PATCH), and delete deployment/runtime
  environments. Project-scoped or global. List filter: `?project={id}` for
  one project, `?project=*` for everything (currently returns globals only
  and surfaces a 503 when project rows exist that the storage trait cannot
  enumerate — see follow-up below), or no param for globals only.
  Mutating endpoints require the `admin` role; deletion refuses if the
  environment still owns secrets.
- **Environment-aware secrets routing.** `POST/GET/DELETE /api/v1/secrets`
  now accept `?environment={env_id}` to resolve the secret scope from the
  environment store (`env:{id}` for globals, `project:{pid}:env:{id}` for
  project-scoped). Body `scope` and `?scope=` continue to work for legacy
  / API-key-style clients. The two forms are mutually exclusive — sending
  both returns `400`. New helper `env_scope(&StoredEnvironment) -> String`
  centralises the scope-string convention.
- **`POST /api/v1/secrets/bulk-import?environment={id}`** parses a
  dotenv-style payload (`KEY=value` per line, with `#` comments and
  optional surrounding quotes) and writes each entry under the env's
  scope. Returns `{ created, updated, errors }`. Admin-only.
- **`GET /api/v1/secrets/{name}?reveal=true`** returns the plaintext
  value alongside metadata. Admin-only.
- **`build_environment_routes(env_state, secrets_state) -> Router<()>`**
  helper plus reworked `build_router_with_secrets` /
  `build_router_with_services_and_secrets` /
  `build_router_with_internal_and_secrets` builders that now take an
  `Arc<dyn EnvironmentStorage>` and mount `/api/v1/environments` alongside
  `/api/v1/secrets`. `SecretsState::with_environments(...)` is the new
  constructor for env-aware secrets state.
- **Extended `SecretRef` parser** in `zlayer-secrets` with environment-scoped
  references: `$S::env/name` (global environment) and
  `$S:project:env/name` (project-scoped). Legacy `$S:name` and
  `$S:name/field` forms are preserved. The leading `::` disambiguates from
  the legacy JSON-field extraction syntax (`$S:name/field`).
- **CLI `zlayer env` command group** — `zlayer env ls [--project ID]`,
  `zlayer env create <name> [--project ID]`, `zlayer env show <id>`,
  `zlayer env update <id>`, `zlayer env delete <id>`. Full environment CRUD
  from the terminal.
- **CLI `zlayer secret` extended with `--env` flag** — all existing secret
  subcommands (list/get/set/unset) now accept `--env <env-id-or-name>` to
  scope operations to an environment. The `--env` flag resolves by name when
  a non-UUID is passed. Import and export also support `--env`. All existing
  scope-based behaviour (without `--env`) is preserved.

### Changed
- Secrets handlers (`create_secret`, `list_secrets`, `get_secret_metadata`,
  `delete_secret`) now use the `AuthActor` extractor (admin-gated for
  mutations) instead of `AuthUser`. The previous implementation scoped
  every secret to the caller's user id; new code defaults to a literal
  `"default"` scope when no `environment` / `scope` is provided.
- `SecretMetadataResponse` gained an optional `value` field, populated
  only on the `?reveal=true` admin path. The field is omitted from JSON
  when unset, so existing clients see no change.

### Follow-ups
- `?reveal=true` is admin-only but does not yet require a re-authentication
  challenge within the last 60 seconds. A future iteration should add that
  guard.
- `?project=*` listing of environments cannot enumerate distinct project
  ids through the current `EnvironmentStorage` trait. The handler returns
  `503 Service Unavailable` (with the count of unenumerable rows) when
  project-scoped environments exist; once the trait grows a proper
  `list_all` accessor we will switch to true cross-project listing.

## [0.10.82]

### Added
- **User accounts with email/password login.** ZLayer now has a real user
  system instead of raw API keys. First-run setup via
  `POST /auth/bootstrap` creates the initial admin; subsequent users via
  `POST /api/v1/users` (admin-gated). Passwords are hashed with Argon2id in
  the existing encrypted `zlayer-secrets` credential store. New
  `storage/users.rs` with `UserStorage` trait, `SqlxUserStore`
  (SQLite/sqlx), and `InMemoryUserStore`.
- **Cookie + CSRF browser sessions.** `POST /auth/login` returns an HttpOnly
  `zlayer_session` JWT cookie plus a JS-readable `zlayer_csrf` double-submit
  token. A new `crates/zlayer-api/src/middleware/csrf.rs` enforces the
  double-submit on mutating cookie-authed requests; Bearer-authed API
  clients (including local CLI over the Unix-socket auto-bearer path) pass
  through untouched. Standard production flow — HttpOnly / Secure / SameSite=Lax /
  constant-time token compare via `subtle`.
- **New auth endpoints** on `zlayer-api`: `POST /auth/bootstrap`,
  `POST /auth/login`, `POST /auth/logout`, `GET /auth/me`,
  `GET /auth/csrf`. Existing `POST /auth/token` (Bearer-JWT exchange) is
  preserved for API clients and for CLI session persistence.
- **Users CRUD** at `/api/v1/users`: list, create, get, update (PATCH),
  delete, and `POST /{id}/password`. Self-service password change when the
  caller provides the current password; admins can reset for any user.
- **`AuthActor` extractor** accepts both `Authorization: Bearer …` and the
  `zlayer_session` cookie, so every endpoint works for both API clients and
  browsers. `SessionAuthUser` is also available when cookie-only is required.
- **CLI auth and user command groups.** `zlayer auth bootstrap | login |
  logout | whoami` and `zlayer user ls | create | set-role | set-password |
  delete`. Interactive password prompts via `dialoguer` when flags are
  omitted. Session persisted to `~/.zlayer/session.json` (mode 0600 on Unix)
  with atomic temp-file-plus-rename writes.
- **OpenAPI coverage.** All new endpoints and request/response schemas are
  registered in the generated spec served at `/swagger-ui`.

### Changed
- **`AuthState` gained `user_store: Option<Arc<dyn UserStorage>>` and
  `cookie_secure: bool` fields.** All construction sites were updated to
  default both (`None` and `false`). Production deployments should set
  `cookie_secure = true` behind TLS.
- **`Claims` now carries an optional `email` field** (serde `default` for
  back-compat with tokens minted before this release). `create_token_with_email`
  is the new canonical minting function; the existing `create_token` forwards
  to it with `email = None` so callers don't need to change.
- **CLI `zlayer auth` command surface replaced.** The previous remote-server
  API-key flow (`zlayer auth login <URL> --api-key … --api-secret …`) is
  gone. Email/password is the new primary flow against the local daemon;
  remote-server + `--url` support can return as a follow-up.

### Security
- Argon2id hashing for user passwords (reused from the existing encrypted
  credential store; no downgrade).
- HttpOnly session cookies; CSRF double-submit with constant-time compare
  (`subtle`); short-circuit on Bearer so API clients aren't CSRF-gated.
- Session file mode 0600 on Unix; atomic write via temp file + rename.

## [0.10.81]

### Fixed
- **`ZImagefile`/Dockerfile `WORKDIR` now materialises the directory in the
  built image's rootfs**, matching Docker's WORKDIR semantics. Previously,
  the builder only ran `buildah config --workingdir` (metadata-only), so
  images built from a `ZImagefile` declaring e.g. `workdir: /workspace`
  shipped without `/workspace` in the rootfs. Containers started with
  `cwd=/workspace` then failed at init time with `chdir(/workspace): ENOENT`,
  surfaced as the opaque upstream message "failed to unix syscall" from
  youki's init process. `crates/zlayer-builder/src/buildah/mod.rs` now emits
  a `buildah run -- mkdir -p <dir>` alongside the `--workingdir` config.
- **`zlayer build` now hard-errors with actionable remediation** when
  `/var/lib/zlayer/{cache,registry}` isn't writable by the current UID,
  instead of silently dropping the local registry and producing an image the
  daemon can't find (which then falls through to `docker.io/library/*` with a
  cryptic 401).

### Changed
- **`zlayer daemon install` chowns the build-facing data dirs**
  (`registry`, `cache`, `bundles`) to `$SUDO_USER:zlayer` (mode 2775) instead
  of only `chgrp zlayer`. The installing user can run `zlayer build`
  immediately in their current shell — no `newgrp zlayer` or re-login
  required. Group membership is still provisioned so additional users added
  later can share the same dirs (after their own re-login).
- **`install.sh` and `install.ps1` warn when a stale `zlayer` binary earlier
  on `PATH` shadows the one just installed**, which otherwise produces
  mystifying "image built but daemon can't find it" symptoms (the stale
  binary skips the local-registry import step that the current binary
  performs).

## [0.10.77]

### Changed
- **`--deployment` is now optional on `zlayer logs`, `zlayer stop`, `zlayer exec`,
  `zlayer job trigger`, `zlayer job status`, and `zlayer cron status`.**
  A new shared resolver at `bin/zlayer/src/commands/resolver.rs` auto-picks the
  deployment when the service (or deployment) name is unambiguous. When multiple
  candidates match and stdout is a TTY, an interactive `dialoguer::Select` prompt
  disambiguates; when non-TTY, the command errors with the list of candidates and
  a hint to pass `--deployment` explicitly. Example: `zlayer logs my-api` now
  works directly whenever `my-api` appears in exactly one deployment.
- **Removed duplicated deployment-detection in `commands/exec.rs`.** The old
  `auto_detect_deployment` helper is gone; `exec` now uses the shared resolver
  (which additionally validates the service name and supports the TTY prompt path,
  neither of which `exec` did before).
- **`CLAUDE.md` aligned with the current tree.** Removed stale references to
  `bin/runtime/` and `bin/zlayer-build/` (both consolidated into `bin/zlayer` long
  ago), documented `bin/zlayer-desktop`, and corrected the `cargo run` examples.
  Also updated the CHANGELOG guidance to use real version numbers (never
  `[Unreleased]`).

## [0.10.76]

### Added
- **`zlayer` system group provisioned during `daemon install` (Linux).**
  `zlayer daemon install` now creates a `zlayer` system group (via `groupadd
  --system`), adds `$SUDO_USER` to it (via `usermod -aG`), and chgrps +
  chmods the shared build-facing data directories — `registry`, `cache`, and
  `bundles` — so unprivileged users can run `zlayer build` against the
  system-wide data dir without sudo. Secrets, raft, and live container
  runtime state (containers/rootfs/volumes) stay root-only. Applied before
  the `systemctl daemon-reload` call so group setup still takes effect on
  WSL distros with systemd disabled. Membership in `zlayer` is
  root-equivalent on the host (same trust model as the `docker` group). No
  change on macOS (single-user data dir) or Windows.

### Fixed
- **`zlayer-manager` dashboard failed with `error sending request for url
  (http://0.0.0.0:3669/...)`.** Two compounding issues. First, `zlayer serve`
  was baking the bind address directly into the `ZLAYER_API_URL` env var it
  injects into every container, producing `http://0.0.0.0:3669` when the
  default wildcard bind was used — wildcard IPs can't be dialed.
  `bin/zlayer/src/commands/serve.rs` now substitutes `127.0.0.1` / `[::1]`
  for wildcard binds when constructing the per-container URL. Second, even
  with a valid TCP URL the manager can't reach the host's loopback from
  inside an overlay-networked container. `crates/zlayer-manager/src/api_client.rs`
  now supports HTTP-over-Unix-Domain-Socket via `ZLayerClient::new_unix`,
  backed by a `hyper_util::client::legacy::Client` + `tower::Service<Uri>`
  UnixConnector (pattern copied from `bin/zlayer/src/daemon_client.rs`).
  `get_api_client()` in `server_fns.rs` now prefers `ZLAYER_SOCKET` when
  set + file exists, falling back to `ZLAYER_API_URL` TCP. Works regardless
  of container network mode. All ~40 existing public API methods were
  refactored to route through a transport-aware `execute()` helper; the
  reqwest-backed TCP path is preserved for external/remote manager
  deployments.
- **CI "Install system dependencies" step hung indefinitely.** `sudo apt-get
  install -y` was blocking on `needrestart`'s whiptail prompt on Ubuntu
  22.04+ runners despite `-y`. All four Linux install steps in
  `.forgejo/workflows/ci.yaml` now run with `DEBIAN_FRONTEND=noninteractive`,
  `NEEDRESTART_MODE=a`, `NEEDRESTART_SUSPEND=1`, `sudo -E`, and
  `Dpkg::Options::--force-confnew` so no prompt can stall the job.
- **`zlayer build` silently skipped local-registry import on permission
  errors.** When the builder's local-registry import step hit EACCES —
  typically because an unprivileged user ran `zlayer build` against the
  root-owned `/var/lib/zlayer/registry` — it logged a warning and returned
  success. The daemon then couldn't find the just-built image locally and
  fell through to Docker Hub, producing confusing 401s for images that were
  never supposed to leave the box. The import step now returns a hard error
  that names the registry path and the underlying cause.
- **`zlayer-manager` image build failed copying `hash.txt`.** The 0.10.75
  fix copied from `target/hash.txt`, but cargo-leptos 0.3.x writes `hash.txt`
  next to the compiled binary (`target/release/hash.txt`). Both
  `ZImagefile.zlayer-manager` and `Dockerfile.zlayer-manager` now copy from
  the correct path, verified locally by running `cargo leptos build
  --release` and inspecting the output.
- **`cargo install cargo-leptos` in zlayer-manager/zlayer-web images broke
  when `core2 0.4.0` was yanked.** Fresh semver resolution picked the
  yanked version transitively via `libflate`. Both Dockerfiles and
  ZImagefiles now use `cargo install cargo-leptos --locked`, which reuses
  the lockfile shipped with cargo-leptos and avoids the yank entirely.

## [0.10.75]

### Added
- **Interactive deploy TUI.** `zlayer deploy` and `zlayer up` now render the
  existing ratatui-based deploy TUI by default on a TTY. The TUI animates the
  deployment plan, infra status panel, per-service lifecycle (Registering →
  Scaling → Running or Failed), and a scrollable log pane. SSE events from
  the daemon are translated into structured `DeployEvent`s so the panels
  populate in real time. Ctrl+C inside the TUI detaches cleanly (the daemon
  keeps running). `--no-tui` (already a global flag) falls back to the plain
  logger; dry-run and non-TTY stdout (CI/piped) also stay plain.

### Fixed
- **zlayer-manager container panic at startup.** `cargo-leptos` emits
  `hash.txt` to the workspace target directory when `hash-files = true`, but
  the runtime images didn't copy it, so Leptos 0.8.17 panicked at startup
  with `failed to read hash file` before serving the first request. Both
  `ZImagefile.zlayer-manager` and `Dockerfile.zlayer-manager` now copy
  `hash.txt` into `/app/hash.txt` alongside the binary.

## [0.10.74]

### Added
- **`zlayer volume` CLI commands.** New subcommand group for volume
  management: `ls` (list volumes with name, path, and size) and `rm`
  (remove a volume, with `--force` for non-empty volumes). Includes new
  REST API endpoints (`GET /api/v1/volumes`, `DELETE /api/v1/volumes/{name}`)
  backed by filesystem-level volume directory enumeration, plus
  `DaemonClient` methods and route registration in the daemon server.

## [0.10.73]

### Added
- **`zlayer network` CLI commands.** New subcommand group for network
  management: `ls`, `inspect`, `create`, `rm` for network CRUD, plus
  `status`, `peers`, and `dns` for overlay network introspection. All
  commands communicate with the daemon via the existing REST API endpoints.

### Fixed
- **zlayer-manager white screen in production.** Container images were missing
  `LEPTOS_OUTPUT_NAME`, `LEPTOS_SITE_PKG_DIR`, and `LEPTOS_HASH_FILES` runtime
  env vars, so Leptos SSR generated unhashed script paths that didn't match the
  hashed files built by cargo-leptos. Also added `assets-dir` to `Leptos.toml`
  so the logo and other static assets are copied into the site directory.
- **zlayer-web same missing env vars.** Applied the same runtime env var fix to
  the zlayer-web container images.

### Changed
- **zlayer-manager default port 9120 → 6677.** Port 9120 conflicted with
  Komodo. Updated CLI default, Leptos config, container images, deployment
  spec generator, and documentation.

## [0.10.72]

### Fixed
- **`zlayer status` reports "not running" when daemon runs as root via
  systemd.** The Unix socket at `/var/run/zlayer.sock` was created with
  mode `0o660` (owner+group only). When the daemon runs as root, the
  socket is owned by `root:root`, so non-root users cannot stat it and
  `zlayer status` thinks nothing is there. Changed to `0o666`; access
  control is already handled by the local auth token.
- **`zlayer daemon status` reports "not installed" on SELinux/Fedora
  Atomic systems.** The `detect_service_level()` function tried to stat
  `/etc/systemd/system/zlayer.service` to decide system vs user service,
  but on SELinux-confined systems non-root users cannot stat files in
  that directory. The fallback returned `User`, causing all `systemctl`
  calls to use `--user` (wrong namespace). Removed the dead
  `ServiceLevel::User` code path entirely — on Linux, zlayer is always
  installed as a system service.

## [0.10.71]

### Fixed
- **`zlayer deploy` keeps serving stale `:latest` images after a new
  release is pushed.** The end-to-end pull path had three independent
  cache bugs that all had to be fixed together. First, the
  `zlayer-registry` manifest cache was keyed by tag (`manifest:<image>`)
  in a persistent SQLite store with no TTL and no revalidation, so any
  cached mutable-tag manifest was served forever across daemon restarts.
  `pull_manifest` now revalidates mutable tags (`:latest`, `:dev`,
  `:edge`, `:main`, `:master`, empty/missing tag) via
  `HEAD /v2/{repo}/manifests/{tag}` and compares the upstream digest
  against a new `manifest-digest:<image>` sidecar entry; on mismatch the
  stale cache is invalidated before the refetch. Registry-unreachable
  errors fall back to the cached copy with a warning so offline deploys
  still work. Pinned tags and digest refs keep their fast-path behavior.
  Second, `YoukiRuntime::pull_image` hard-coded `PullPolicy::IfNotPresent`,
  dropping any `pull_policy: always` the user set in their spec before it
  reached the puller; `pull_image_with_policy` and `pull_image_layers` now
  thread the spec's policy all the way through to the registry client via
  the new `ImagePuller::pull_image_with_policy(..., force_refresh)` entry
  point, which invalidates the manifest cache when `force_refresh` is
  true. Third, `DockerRuntime::pull_image_with_policy` trusted
  `docker inspect_image` alone to short-circuit `IfNotPresent` pulls,
  never consulting the registry; it now compares the local image's
  `repo_digests` entry against the upstream digest returned by
  bollard's `inspect_registry_image` (the Docker distribution API) and
  re-pulls on mismatch.
- **`zlayer build` / `zlayer-build` reuse stale base images.**
  `buildah from` was invoked with no `--pull` flag, so any upstream
  republish of the base image was invisible to subsequent local builds
  until `buildah rmi` was run manually. Builds now default to
  `--pull=newer`, matching modern build-tool conventions: fast when
  nothing has changed, correct when the registry has a newer copy.
  Controlled via the new `--pull=<newer|always|never>` and `--no-pull`
  flags on `zlayer build`.

### Added
- **`zlayer image ls`, `zlayer image rm <image>`, `zlayer system prune`
  subcommands.** First-class image management from the CLI via new
  `/api/v1/images`, `/api/v1/images/{image}`, and `/api/v1/system/prune`
  REST endpoints on the daemon, backed by new
  `Runtime::list_images`, `Runtime::remove_image`, and
  `Runtime::prune_images` trait methods. Implemented for the Docker
  runtime (via bollard's image-management APIs) and the Youki runtime
  (walks the persistent `zlayer-registry` cache to remove manifest
  entries and their referenced layer blobs, and prunes orphaned
  content-addressed blobs). Other runtimes inherit a default
  "unsupported" error. Users can now force a fresh pull of a stale
  `:latest` image with `zlayer image rm <image>` + redeploy instead of
  manually wiping `{data_dir}/cache/blobs.redb`.

## [0.10.70]

### Fixed
- **Deploy "Stabilization timed out" no longer masks the real container
  failure.** When a container's init process crashed during startup (bad
  image, missing libs, failed mount), the overlay attach code would run
  against the now-dead PID and emit misleading `RTNETLINK answers: No such
  process` / `File exists` / `Invalid "netns" value` errors. The user then
  saw a generic `Stabilization timed out: N/N replicas, healthy=false`
  with no pointer to the root cause. The agent now (a) checks the
  container's state between `start_container` and overlay attach, (b)
  returns the container's log tail when the init already exited, and
  (c) includes each failing service's recent log lines in the
  stabilization timeout error itself.
- **Veth leak from failed overlay attaches.** Each failed
  `attach_to_interface` used to leave a `veth-<pid>` pair (one end named
  `eth0`) orphaned in the host network namespace. The next deploy's
  `ip link add ... peer name eth0` then failed with RTNETLINK "File
  exists", cascading across redeploys. The overlay manager now (a) creates
  the container-side veth with a unique name (`vc-<pid>`) and renames it
  to `eth0` only after moving into the container's netns, (b) deletes
  the pair on any attach-path failure, and (c) sweeps orphan veth pairs
  whose owning PID is no longer alive before each new attach.
- **`zlayer-manager` and `zlayer-web` images fail at runtime with
  `version 'GLIBC_2.38' not found`.** The cargo-chef builder
  (`lukemathwalker/cargo-chef:latest-rust-1.90`) runs on Debian trixie
  (glibc 2.41), but the runtime stage was pinned to `debian:bookworm-slim`
  (glibc 2.36). Binaries referencing GLIBC_2.38+ symbols would not start.
  Both Dockerfile and ZImagefile runtime stages now use `debian:trixie-slim`
  to match the builder glibc.

## [0.10.69]

### Fixed
- **`zlayer daemon status` and `zlayer status` report "not installed" / "not running"
  after installing with sudo.** The daemon commands used `geteuid()` to decide
  between system-level and user-level systemd paths. Since `install.sh` runs
  `sudo zlayer daemon install` (system-level), but users query without sudo
  (user-level), the status commands looked in the wrong place. Now probes the
  filesystem for the actual unit file location and data directory, falling back
  to euid only during fresh installs.

## [0.10.66]

### Fixed
- **Container creation "failed to prepare rootfs" on Linux.** libcontainer 0.5.7
  used `is_file()` to decide bind mount destination type, which returned false
  for Unix sockets — causing the ZLayer API socket mount to create a directory
  instead of a file, failing with EINVAL. Upgraded to libcontainer git rev
  `a68a38c4` (youki-dev/youki#3484) which uses `!is_dir()` instead.
- **Improved container creation error reporting.** libcontainer errors now use
  Debug formatting to preserve the full error chain, so mount failures show
  the actual syscall error (e.g., EINVAL, EPERM) instead of just
  "failed to prepare rootfs".

### Changed
- **Socket bind mount uses `typ("bind")`** instead of `typ("none")` in the OCI
  spec, ensuring libcontainer's bind mount code path handles the source
  correctly.
- **Set `rootfsPropagation` to `"private"`** in OCI spec (matches Docker default).

### Added
- **`scripts/install-dev.sh`** — builds from source and installs like `install.sh`
  but reports `0.0.0-dev`. Run `install.sh` to go back to a release build.

## [0.10.65]

### Fixed
- **SELinux 203/EXEC on Fedora/RHEL.** Binaries installed under
  `/var/lib/zlayer/bin` inherited `var_lib_t`, which systemd's `init_t` cannot
  exec as a service entrypoint, so `systemctl start zlayer` failed with
  `status=203/EXEC`. `install.sh` now relabels the binary to `bin_t` via
  `semanage fcontext` + `restorecon` (persistent) with `chcon` as a fallback
  for systems without `policycoreutils-python-utils` (e.g., Fedora Silverblue).
- **Daemon startup failure now shows current error, not stale logs.** Daemon log
  files are truncated before each start so failure diagnostics only contain
  output from the current attempt. Switched from `RotationStrategy::Never` to
  `Daily` rotation with 7-day retention. Added `--since=-2min` to the
  journalctl fallback query on Linux. Previously, `zlayer daemon install/start`
  could print week-old tracing entries from previous attempts.
- **Systemd unit upgraded to `Type=notify`** with `sd_notify(READY=1)` via the
  pure-Rust `sd-notify` crate, matching Docker/containerd/CRI-O. `systemctl
  start zlayer` now blocks until the daemon is truly ready (all init phases
  complete, API socket bound) instead of returning immediately.
- **Fixed daemon.json / socket bind race condition.** `daemon.json` was written
  ~300 lines before the API socket bound, so the CLI could think the daemon was
  ready while the socket wasn't listening yet.

### Changed
- **Install script now auto-configures PATH.** When the install directory is not
  already on PATH, the user is prompted (Y/n) and the script writes
  `/etc/profile.d/zlayer.sh` (Linux) or `/etc/paths.d/zlayer` (macOS).
- **Install script reports installed vs target version.** Shows both versions
  before downloading and prompts before reinstalling the same version.

### Added
- **Networks management page** in ZLayer Manager UI at `/networks`: full CRUD
  interface for network access-control policies. Displays stats row (total
  networks, members, rules, active policies), networks table with View/Delete
  actions, detail modal showing CIDRs as badges, members table with kind badges,
  and access rules table with allow/deny badges. Create modal accepts name,
  description, and comma/newline-separated CIDRs. Delete confirmation modal
  included. API client methods (`list_networks`, `get_network`,
  `create_network`, `delete_network`) and Leptos server functions added. Sidebar
  updated with "Networks" link in the Network section.
- **Network policy enforcement in reverse proxy**: the proxy now evaluates
  `NetworkPolicySpec` access rules against incoming requests. When a source IP
  belongs to a network with defined policies, access is allowed or denied based
  on service/deployment/port rules (deny takes priority, default deny when
  governed). The `NetworkPolicyChecker` is shared between the API layer and
  the proxy so that policy changes via the Networks API take effect immediately.
- **Networks API** (`/api/v1/networks`): CRUD endpoints for network
  access-control groups. Supports creating, listing, getting, updating, and
  deleting `NetworkPolicySpec` objects that define membership (users, groups,
  nodes, CIDRs) and service access rules. Includes `ip_matches_network()`
  helper for CIDR-based access matching.
- **`ServiceNetworkSpec` rename**: the per-service overlay/join-policy config
  formerly named `NetworkSpec` is now `ServiceNetworkSpec` to avoid collision
  with the new standalone `NetworkPolicySpec` type.
- **Reverse Proxy management page** in ZLayer Manager UI at `/proxy`: displays
  proxy stats (total routes, healthy/total backends, TLS certificates, active
  streams), routes table with expandable backend details and health badges, TLS
  certificates table with expiry warnings for certificates expiring within 7
  days, and stream proxies table with TCP/UDP protocol badges. Sidebar updated
  with "Reverse Proxy" and "SSH Tunnels" links in the Network section.
- **Proxy management API endpoints** (`GET /api/v1/proxy/{routes,backends,tls,streams}`):
  read-only REST endpoints for inspecting reverse proxy state. Includes L7
  route listing, load-balancer backend groups with health status, TLS
  certificate inventory with expiry/renewal info, and L4 TCP/UDP stream
  proxies. New getters added to `ServiceRegistry::list_routes()`,
  `LoadBalancer::{list_service_names, group_snapshot}`,
  `CertManager::list_cached_domains()`, and
  `StreamRegistry::{list_tcp_services, list_udp_services}`.
- **Proxy management API client and server functions** in ZLayer Manager:
  adds `ProxyRoute`, `ProxyBackendGroup`, `TlsCertificate`, and `StreamProxy`
  types to the API client with methods `list_proxy_routes()`,
  `list_proxy_backends()`, `list_tls_certificates()`, and
  `list_stream_proxies()`. Corresponding Leptos server functions
  (`get_proxy_routes`, `get_proxy_backends`, `get_tls_certificates`,
  `get_stream_proxies`) provide data access for the proxy management UI page.
- **Service endpoint display** in deployment detail modal: each service row now
  shows its configured endpoints (protocol, port, URL) in an expandable sub-table.
  Protocol badges use DaisyUI color coding (HTTP=info, HTTPS=success,
  WebSocket=secondary, gRPC=accent, others=ghost). Services with no endpoints
  show "No endpoints configured".
- `ServiceEndpoint` type in Manager server functions for passing endpoint data
  from the API to the UI.
- `get_services()` now fetches per-service details to include endpoint
  information alongside the service summary data.

## [2026-04-03]

### Added
- **Nodes management page** in ZLayer Manager: replaces the "Coming Soon" stub
  with a live cluster node table sourced from the Raft cluster state. Shows
  stats row (total nodes, healthy count, leader, cluster health), a table with
  node ID, address, role (leader/voter/learner badge), overlay IP, status
  (ready/draining/dead badge), CPU, and memory. Includes a "Generate Join
  Token" card when a leader is present.
- `ClusterNodeSummary` in `/api/v1/cluster/nodes` now returns enriched data:
  `advertise_addr`, `overlay_ip`, `cpu_total`, `cpu_used`, `memory_total`,
  `memory_used`, `registered_at`, `last_heartbeat`, `role`, and `mode`.
- `list_cluster_nodes()` method on `ZLayerClient` API client for calling
  `GET /api/v1/cluster/nodes`.
- Dashboard `get_system_stats()` now reports real node counts and aggregate
  CPU/memory usage from cluster state instead of hardcoded zeros.

### Changed
- Overlay API endpoints (`/api/v1/overlay/status`, `/peers`, `/ip-alloc`, `/dns`)
  now return real data from the `OverlayManager` and `DnsServer` instead of stub
  503 responses. `OverlayApiState` holds `Option<Arc<RwLock<OverlayManager>>>` and
  `Option<Arc<DnsServer>>`; the daemon's `serve` command wires them in from
  `DaemonState`. When the overlay is unavailable (host networking mode), endpoints
  return appropriate "unavailable" responses.
- Added getters on `OverlayManager`: `deployment()`, `global_interface()`,
  `overlay_port()`, `has_global_transport()`, `service_transport_count()`,
  `overlay_cidr()`, `ip_alloc_stats()`.

### Added
- Theme toggle button in Manager navbar: toggles between "dark" and "zlayer"
  (light) DaisyUI themes with localStorage persistence across page refreshes.
  Sun icon in dark mode, moon icon in light mode.
- Structured logging types: `LogEntry`, `LogStream`, `LogSource`, `LogQuery` in
  `zlayer-observability::logs` — unified type for all log sources (containers,
  jobs, builds, daemon) with timestamps and stream identification.
- `FileLogWriter` / `MemoryLogWriter` for writing structured JSONL log entries
  to disk or in-memory ring buffer.
- `FileLogReader` / `apply_query()` in `zlayer-observability::log_reader` for
  reading JSONL files with filtering (stream, source, time range, tail limit)
  and legacy `stdout.log`/`stderr.log` backward compatibility.
- `LogOutputConfig` / `LogDestination` types for configurable log destination
  (disk/memory), max size, and retention.
- `LogsConfig` in `zlayer-spec` with `logs: Option<LogsConfig>` on `ServiceSpec`
  so deployment specs can configure per-service log output.
- `max_files: Option<usize>` on `FileLoggingConfig` — old rotated log files
  beyond this limit are cleaned up at startup (default: 7).
- Daemon log rotation via `tracing-appender` with daily rotation, replacing
  the unbounded `dup2`-to-file approach.

### Changed
- Default WireGuard overlay port moved from `zlayer-overlay` to `zlayer-core`
  (`DEFAULT_WG_PORT`) for cross-platform availability (Windows thin CLI).
- `Runtime` trait: `container_logs()` returns `Vec<LogEntry>` (was `String`),
  `get_logs()` returns `Vec<LogEntry>` (was `Vec<String>`). All 4 runtimes
  (youki, docker, macOS sandbox, WASM) updated.
- `ServiceManager::get_service_logs()` returns `Vec<LogEntry>` with populated
  service/deployment fields.
- Daemon systemd unit uses `StandardOutput=journal` instead of appending to
  `/var/log/zlayer/daemon.log`. The daemon manages its own files via
  `tracing-appender`; journald captures pre-init crashes.
- `rotate_daemon_log()` renames the current day's log before every start so
  failure output is always fresh.
- `wait_for_daemon_ready()` finds the newest `daemon.*` tracing-appender file
  instead of hardcoded `daemon.log`, with journalctl fallback.
- Log rotation loop now handles `.jsonl` files alongside `.log`, and cleans up
  the `executions/` subdirectory.
- Raft bootstrap checks metrics before calling `initialize()` to suppress
  noisy openraft ERROR log on already-initialized nodes.
- `install.sh` cleanup now removes stale `zl-*` network interfaces, WireGuard
  UAPI sockets, and kills stale port-holding processes.

## [2026-04-02]

### Changed
- Default WireGuard overlay port changed from 51820 to 51420 to avoid
  conflicts with system WireGuard VPNs.
- Replaced all hardcoded `51820` port literals with `DEFAULT_WG_PORT` constant
  and `node_config.overlay_port` configuration throughout the codebase.
- Daemon startup WireGuard port cleanup: replaced passive polling with active
  process identification and termination. Reads overlay port from
  `node_config.json` instead of hardcoding, identifies the port holder via
  `ss`/`lsof`, and sends SIGTERM/SIGKILL to stale zlayer/boringtun processes.
- `OverlayManager` now accepts a configurable overlay port via
  `with_overlay_port()` builder method instead of hardcoding.
- Raft bootstrap on restart now checks metrics before calling `initialize()`,
  suppressing the noisy openraft ERROR log on already-initialized nodes.
- `install.sh`: expanded cleanup on upgrade — removes stale `zl-*` network
  interfaces, WireGuard UAPI sockets, and kills stale port-holding processes.

## [2026-04-01]

### Added
- New `zlayer-docker` crate: Docker CLI, Compose, and API socket compatibility layer.
  - `compose` module: Parse `docker-compose.yaml` files and convert to ZLayer `DeploymentSpec`.
    Handles ports (short/long syntax), volumes (bind/named/tmpfs), environment (map/list),
    depends_on (with conditions), healthchecks, deploy resources/replicas, and more.
  - `cli` module: Docker-compatible CLI subcommands (`zlayer docker run/ps/stop/build/compose/etc.`)
    with full argument parsing mirroring Docker CLI flags.
  - `socket` module: Docker Engine API v1.43 socket emulation over Unix domain socket.
    Stub endpoints for containers, images, volumes, networks, and system info.
  - 104 unit tests + 5 integration tests covering compose parsing, conversion, and round-trips.
  - Example `docker-compose.yaml` in `examples/` (nginx + API + postgres + redis stack).
  - Wired into `bin/zlayer` as `zlayer docker` subcommand (feature-gated: `docker-compat`).
- `zlayer daemon install --docker-socket`: opt-in Docker API socket at `/var/run/docker.sock`
  and Docker CLI shim at `/usr/local/bin/docker` → `zlayer docker`.
- `zlayer serve --docker-socket`: spawns Docker API socket server alongside the daemon.

## [2026-03-31]

### Fixed
- `install.sh` falling back to `~/.local/bin` which breaks `daemon install` on
  immutable distros (Fedora Atomic/Silverblue/uBlue). Now write-probes
  `/usr/local/bin` (k3s pattern), falls back to `/opt/bin`. Binary always lands
  in a system path accessible to systemd.
- `pick_system_binary_path()` last-resort fallback returning `/opt/zlayer/bin/zlayer`
  without creating the directory, causing ENOENT on `std::fs::copy`.
- Sandbox `copy_dir_recursive` failing on symlinks (common in macOS Homebrew images),
  breaking secondary tag creation (`:latest`). Symlinks are now preserved instead of
  falling through to `tokio::fs::copy`.
- Crate publishing failing for all crates: `zlayer-wsl` had a hardcoded `version = "0.0.0-dev"`
  instead of `version.workspace = true`, causing workspace resolution to fail after the
  release sed version bump.
- Sandbox backend push failure for multi-tag images ("rootfs not found"): secondary
  tags now get on-disk directories via `tag_image()` before the push phase runs.
- Sandbox backend tar archive failure on macOS ("No such file or directory"): tar
  builder now preserves symlinks instead of following them, fixing broken Homebrew
  symlinks and producing correct OCI layers.
- Build workflow `upload-temp-packages` output: `package_base_url` is now set
  unconditionally so re-runs don't pass an empty artifact URL to the release workflow.

## [2026-03-30]

### Added
- `host` field on `EndpointSpec` for host-based/subdomain proxy routing
  (e.g. `host: "api.example.com"` or `host: "*.example.com"`), wired into
  `RouteEntry::from_endpoint()` so specs can declare host patterns directly.
- GPU model affinity: `model` field on `GpuSpec` pins workloads to nodes with
  a matching GPU model (substring match, e.g. `model: "A100"`).
- GPU device index tracking: scheduler now allocates specific GPU indices per
  container, preventing device overlap when multiple containers share a node.
  `PlacementDecision` carries `gpu_indices` for downstream device injection.
- GPU environment variable injection: containers automatically receive
  `NVIDIA_VISIBLE_DEVICES`/`CUDA_VISIBLE_DEVICES` (NVIDIA),
  `ROCR_VISIBLE_DEVICES`/`HIP_VISIBLE_DEVICES` (AMD), or
  `ZE_AFFINITY_MASK` (Intel) based on vendor and allocated indices.
- Gang scheduling (`scheduling: gang` on `GpuSpec`): all-or-nothing placement
  for distributed GPU jobs — if any replica cannot be placed, all are rolled back.
- Distributed job coordination (`distributed` on `GpuSpec`): injects
  `MASTER_ADDR`, `MASTER_PORT`, `WORLD_SIZE`, `RANK`, `LOCAL_RANK`, and
  backend-specific env vars (`NCCL_SOCKET_IFNAME`/`GLOO_SOCKET_IFNAME`).
- GPU spread scheduling (`scheduling: spread`): distributes GPU workloads across
  nodes instead of bin-packing them onto the fewest nodes.
- GPU sharing modes (`sharing` on `GpuSpec`): `mps` for NVIDIA Multi-Process
  Service (up to 8 containers per GPU) and `time-slice` for round-robin sharing
  (up to 4 containers per GPU). Fractional GPU tracking in scheduler.
- `MpsDaemonManager` in `zlayer-agent` for reference-counted NVIDIA MPS daemon
  lifecycle management (auto-start on first MPS container, auto-stop on last).
- GPU utilization in heartbeat: `GpuUtilizationReport` struct with per-GPU
  utilization %, memory, temperature, and power reported via Raft heartbeat.
- GPU metrics collection module (`gpu_metrics`): portable metrics via
  `nvidia-smi` (NVIDIA), sysfs (AMD/Intel) — no hard driver dependencies.
- GPU health monitoring: detects thermal throttling, ECC errors, and
  unresponsive GPUs via `check_gpu_health()`.
- GPU metrics in `zlayer-observability`: utilization (percent), memory used/total
  (bytes), temperature (celsius), and power draw (watts). Each metric is a
  Prometheus `GaugeVec` with `gpu_index` and `node` labels.
- CDI (Container Device Interface) support: discovers specs from `/etc/cdi/`
  and `/var/run/cdi/`, resolves fully-qualified device names, merges container
  edits, and can generate NVIDIA specs via `nvidia-ctk`.
- macOS sandbox backend now supports `push_image`, `tag_image`, `manifest_create`,
  `manifest_add`, and `manifest_push` operations. Previously, pipeline builds on
  macOS succeeded but push/manifest operations always failed with "Operation 'push'
  is not supported by this backend". The sandbox backend now tars the rootfs, builds
  OCI manifests, and pushes via the registry client (requires the `cache` feature,
  which the `zlayer` CLI already enables). Multi-platform manifest lists are stored
  on disk and pushed as OCI image indexes.
- `ImagePuller::push_image_index_to_registry` method for pushing OCI image indexes
  (manifest lists) to remote registries.

## [2026-03-29]

### Fixed
- `zlayer-paths` crate now publishable (removed `publish = false`) and added to
  CI release publish order so crates depending on it can resolve it from the registry.
- `daemon install` no longer fails with "Text file busy" (ETXTBSY) when the old
  binary is still running via systemd. The install function now stops the service
  and unlinks the destination before copying.
- `admin_password` file permissions are now enforced to 0o644 on every daemon
  startup, not just on initial creation. Previously, files created with the old
  0o600 permissions stayed unreadable to non-root tools (E2E tests, CLI) forever.
- `DockerConfigAuth` now respects the `DOCKER_CONFIG` environment variable
  (standard Docker convention) for locating `config.json`.

## [2026-03-28]

### Fixed
- Daemon startup failures now produce diagnostic output instead of
  "no log output found at /var/log/zlayer/daemon.log". The systemd unit
  template now redirects stdout/stderr to the daemon log file via
  `StandardOutput=append:` and `StandardError=append:` directives. The log
  directory is pre-created before starting the service. On timeout,
  `wait_for_daemon_ready()` falls back to querying journalctl and provides
  a diagnostic hint.

## [2026-03-27]

### Fixed
- Install scripts (`install.sh`, `install.py`) now install `libseccomp` runtime
  library, verify cgroups v2, and create `/var/lib/zlayer/` state directories.
  Previously, fresh installs would fail to start containers because the bundled
  libcontainer runtime requires libseccomp at runtime but nothing installed it.
- Container, job, cron, and build API routes were missing the `AuthState`
  extension because it was applied inside `build_router_with_deployment_state()`
  before routes were nested in `serve.rs`. The `Extension(auth_state)` layer is
  now re-applied after all `.nest()` calls so every route receives auth context.
- `install.sh`: daemon install output was silently discarded (`>/dev/null 2>&1`).
  Now shows output so users can see what happened. Linux installs use `sudo`,
  macOS installs do not. Removed redundant post-install status checks that
  duplicated the daemon's own readiness reporting.
- Linux `daemon install`: `systemctl daemon-reload` and `systemctl enable` exit
  codes were silently discarded (`let _ = ...`). They now propagate errors and
  bail with the stderr message on failure.
- Non-deterministic sandbox builder cache hash. `compute_dockerfile_hash()` used
  `format!("{:?}", dockerfile)` which produced different hashes across runs due to
  `HashMap` randomized iteration order. Replaced with deterministic per-instruction
  `cache_key()` hashing. Also fixed `SandboxBackend::build_image()` not forwarding
  `options.source_hash` to `SandboxImageBuilder`, so pipeline-provided hashes were
  being silently ignored.

### Added
- Content-based cache invalidation for the sandbox builder. `SandboxImageConfig`
  now carries a `source_hash` field (SHA-256 of the Dockerfile/ZImagefile). When
  a cached image exists with a matching hash, the rebuild is skipped entirely.
  Pipeline builds (`build_single_image`) also compute a file hash up front and
  short-circuit before even creating an `ImageBuilder` when the output image is
  unchanged. `BuildOptions` and `ImageBuilder` gain a `source_hash` setter.
  `build_toolchain_as_image()` and `build_base_image()` in `macos_image_resolver`
  now embed source hashes in their cached configs.
- Multi-version builds in `scripts/build-macos-images.sh` — e.g.
  `./scripts/build-macos-images.sh node 18 20 22 24` builds Node 18, 20, 22, 24.
  Partial versions auto-resolve to latest patch via upstream APIs. Environment
  variable overrides (`GO_VERSION`, `NODE_VERSION`, `PYTHON_VERSION`, etc.) take
  precedence over API resolution. Output directories are now versioned
  (`$BUILD_DIR/golang-1.23.6/rootfs/`). OCI config.json includes version labels.
- `BuildBackend` trait in `zlayer-builder::backend` providing a pluggable
  abstraction over container build tooling. Includes `BuildahBackend` (wraps
  buildah CLI) and `SandboxBackend` (macOS Seatbelt, cfg-gated). Added
  `detect_backend()` for runtime auto-detection with `ZLAYER_BACKEND` env
  override support. Added `BuildError::NotSupported` variant for unsupported
  backend operations.
- `ImageBuilder::with_backend()` constructor for creating a builder with an
  explicit `BuildBackend`. `ImageBuilder::new()` now auto-detects the backend
  via `detect_backend()`. `with_executor()` wraps the executor in a
  `BuildahBackend` for trait-based dispatch.
- `PipelineExecutor::with_backend()` constructor for creating a pipeline
  executor with an explicit `BuildBackend`. Push, manifest, and per-image
  build operations delegate to the backend when set.

### Refactored
- Refactored package installer in `macos_image_resolver` to use a `ResolvedPackage`
  enum (`HomebrewBottle`, `DirectRelease`, `Tap`, `UvPython`) instead of
  shoehorning everything into `BrewFormulaInfo`. Renamed `fetch_formula_info()`
  to `resolve_package()`, `install_single_bottle()` to `install_package()`, and
  `fetch_and_extract_bottle()` to `install_with_deps()`. Added
  `install_direct_release()` for downloading and extracting forge release assets,
  `install_uv_python()` for Python provisioning via uv, and `DiscoveryResponse`
  for structured RepoSourceSyncer discovery. Dependency BFS walk now only recurses
  for `HomebrewBottle` variants. Added `"golang"` -> `("go", false)` to
  `map_single_package_hardcoded()`.
- Extracted the buildah build orchestration loop (stage walking, container
  creation, instruction execution, COPY --from resolution, cache tracking,
  commit, cleanup) from `ImageBuilder::build()` into
  `BuildahBackend::build_image()`. `ImageBuilder::build()` is now a thin
  coordinator: parse Dockerfile/ZImagefile/template, handle WASM early return,
  delegate to the backend. `LayerCacheTracker` moved to `backend/buildah.rs`.
  Removed inline `build_with_sandbox()`, `resolve_stages()`,
  `resolve_base_image()`, `create_container()`, `commit_container()`,
  `tag_image()`, `push_image()`, and `generate_build_id()` from `builder.rs`.
- Pipeline executor (`pipeline/executor.rs`) now always uses
  `ImageBuilder::with_backend()` instead of falling back to
  `ImageBuilder::with_executor()`, wrapping a bare executor in a
  `BuildahBackend` when no explicit backend is provided.

### Changed
- CLI `pipeline` command now uses `detect_backend()` + `PipelineExecutor::with_backend()`
  instead of directly creating a `BuildahExecutor`, enabling automatic sandbox
  fallback on macOS.
- Register container lifecycle routes (`/api/v1/containers`) in the daemon API
  server, enabling direct container creation, inspection, logs, exec, and stats
  endpoints independent of the deployment/service abstraction.

### Fixed
- macOS sandbox runtime default data directory changed from `~/.local/share/zlayer`
  to `~/.zlayer` to match the builder and other components.
- macOS images CI workflow (`macos-images.yml`): replaced `uses: ./` action
  reference (which tried to download from GitHub and hung) with building zlayer
  from source and running the pipeline directly.

### Changed
- `map_linux_packages()` in `macos_image_resolver` is now async and fetches
  package mappings from RepoSources (`zachhandley.github.io/RepoSources/maps/`)
  with a 7-day local cache at `{data_dir}/cache/package-maps/{distro}.json`.
  Resolution order: cached/fetched RepoSources map, name transformation
  heuristics (strip `-dev`, `lib` prefix, version digits), then hardcoded
  fallback. The original hardcoded mapping is preserved as `map_single_package_hardcoded()`.
- Homebrew bottle install success log now includes the formula version
  (`versions.stable`) from the Homebrew API.

- Refactored `fetch_and_extract_bottle()` in `macos_image_resolver` to resolve
  the full Homebrew dependency tree (BFS) before installing a formula. Split into
  three functions: `fetch_formula_info()`, `install_single_bottle()`, and the
  public `fetch_and_extract_bottle()` entry point. Added `dependencies` and
  `versions` fields to `BrewFormulaInfo`. Formulas already present in the rootfs
  Cellar are skipped, and individual dependency failures are logged as warnings
  without aborting the overall installation.
- Refactored `sandbox_builder::setup_base_image()` to use the 3-tier macOS image
  resolution from `macos_image_resolver`. Old inline toolchain provisioning block
  replaced with Tier 1 (local cache), Tier 2 (GHCR pull), Tier 3 (local build)
  pipeline. Non-rewritable images retain existing cache/pull logic.
- `sandbox_builder::load_base_image_config()` now checks for rewritten macOS image
  `config.json` before falling through to the existing `image_config.json` / registry
  pull path.
- Toolchain env injection in `build()` now guards against duplicating env vars that
  are already present in the config (e.g., when loaded from a cached image config).
- `sandbox_builder::execute_run()` now intercepts Linux package manager install
  commands (`apt-get install`, `apk add`, `yum install`, `dnf install`) and fetches
  Homebrew bottles into the sandbox rootfs before the translated (no-op) command runs.

### Added
- `extract_package_install_packages()` helper: parses package names from apt-get,
  apk, yum, and dnf install commands, handling flags, sudo, and `&&`-compound
  commands.
- `find_after_subcommand()` helper: locates the argument tail after a subcommand
  keyword, skipping leading flags.
- 3-tier macOS image resolution system (`macos_image_resolver`) — rewrites Docker Hub
  image references to macOS-native equivalents. Tier 1: pulls pre-built sandbox images
  from `ghcr.io/blackleafdigital/zlayer`. Tier 2: builds toolchain images locally via
  `macos_toolchain`. Tier 3: creates minimal base rootfs for distro images. Includes
  GHCR auth resolution (GHCR_TOKEN, GITHUB_TOKEN, Docker config), Homebrew bottle
  fetching for installing packages into sandbox rootfs, and Linux-to-brew package
  name mapping.
- GraalVM CE toolchain resolver for macOS sandbox builds — resolves exact versions
  (`21.0.5`), partial/major versions (`21` -> latest 21.x.y), and `latest` by scanning
  GitHub releases from `graalvm/graalvm-ce-builds`. Downloads macOS tarballs and
  extracts with `--strip-components=3` (same JDK structure as Adoptium). Sets both
  `JAVA_HOME` and `GRAALVM_HOME` environment variables. Detects base images containing
  "graalvm" in the name (e.g., `graalvm-ce`, `graalvm/graalvm-ce`).
- Swift toolchain resolver for macOS sandbox builds — provisions Swift from the host
  system's Xcode Command Line Tools rather than downloading. Locates the toolchain via
  `xcrun --find swiftc`, copies it into the cache keyed by the detected version, and
  symlinks into the build rootfs at `/usr/local/swift`. Detects `swift` base images.
- Java (Adoptium/Temurin) toolchain resolver for macOS sandbox builds — resolves `latest`
  (fetches most recent LTS from Adoptium API), exact feature versions (`21`, `17`, `8`),
  and dotted versions (`21.0.5` strips to major `21`). Uses the Adoptium binary API which
  redirects to `.tar.gz` downloads. Extracts with `--strip-components=3` to handle the
  macOS `jdk-X/Contents/Home/` tarball structure. Detects `eclipse-temurin`, `amazoncorretto`,
  and `openjdk` base images.
- Bun toolchain resolver for macOS sandbox builds — resolves exact versions (`1.2.3`),
  partial versions (`1` -> latest 1.x.y), and `latest` by scanning GitHub releases from
  `oven-sh/bun`. Downloads `bun-darwin-{arch}.zip` assets and extracts the binary into
  the `bin/` directory structure for proper PATH integration.
- Python toolchain resolver for macOS sandbox builds — uses standalone builds from
  `astral-sh/python-build-standalone` on GitHub. Resolves exact versions (`3.12.1`),
  partial versions (`3.12` -> latest 3.12.x), and `latest` by scanning GitHub release
  assets. Downloads `install_only_stripped` tarballs for the host architecture.
- Rust toolchain resolver for macOS sandbox builds — resolves exact versions (e.g. `1.82.0`),
  partial versions (`1.82` -> `1.82.0`), and `latest` (fetches stable channel TOML). Downloads
  standalone installer tarballs from `static.rust-lang.org`, extracts, and runs `install.sh`
  with `--prefix` to provision rustc + cargo into the build cache.

### Changed
- Sandbox builder: macOS toolchain provisioning now symlinks cached toolchains into the
  build rootfs instead of copying. The cache at `~/.zlayer/toolchains/{lang}-{version}-{arch}/`
  is the immutable source; each build gets a symlink to it, saving disk space and build time.

### Fixed
- Sandbox builder: macOS toolchain provisioning (`macos_toolchain.rs`) — auto-detects
  language from base image (e.g. `golang:1.23-alpine` → Go 1.23), downloads self-contained
  macOS binaries from official APIs (go.dev, nodejs.org, etc.), and provisions them directly
  into the build rootfs. No brew, no global state, parallel-build safe. Supports Go + Node
  initially, with version resolution for any published version.
- Sandbox builder: `macos_compat.rs` package manager commands (`apk add`, `apt-get install`)
  are now no-ops — toolchains are provisioned in rootfs, so Linux package installs are skipped
- Sandbox builder: PATH augmentation now always includes rootfs-prefixed paths and Homebrew
  paths, even when the base image already defines its own PATH (e.g. golang images)
- Sandbox builder: broadened Seatbelt mach-lookup to allow all services during build-time,
  fixing brew/ruby/git failures caused by restricted XPC access
- Sandbox builder: RunFailed errors now include stderr output for better diagnostics
- action.yml: added curl timeouts (`--connect-timeout 30 --max-time 120`) to prevent
  indefinite hangs when downloading ZLayer binaries from GitHub releases
- install.sh: added curl timeouts to binary download
- Sandbox builder: per-build HOME directory (`{image_dir}/home/`) instead of hardcoded
  `/root` — fixes `Read-only file system @ dir_s_mkdir - /root` errors from brew/git
- Sandbox builder: base image config (ENV, WORKDIR, USER, ENTRYPOINT, CMD, Shell,
  Healthcheck) is now loaded from OCI image metadata and merged into the build config —
  previously all base image config was silently discarded
- Sandbox builder: base image config is cached alongside rootfs as `image_config.json`
- Registry: `ImageConfig` now parses Shell and Healthcheck fields from OCI image config

### Changed
- Moved `docker/` directory to `images/` and updated all references across the codebase
  (ZPipeline.yaml, CLAUDE.md, Dockerfiles, ZImagefiles, pipeline module docs, builder README)
- Buildah "not found" warning suppressed on macOS (now debug-level); non-macOS unchanged
- Sandbox builder: broadened Seatbelt FS rules for build-time (allows RUN commands to
  write to absolute host paths since there is no chroot on macOS)

### Added
- macOS-native base images defined as ZImagefiles (`images/macos/`):
  `zlayer/base`, `zlayer/golang`, `zlayer/rust`, `zlayer/node`, `zlayer/python` (with uv),
  `zlayer/deno`, `zlayer/bun` — containing macOS Mach-O binaries for sandbox builds
- macOS base image ZPipeline (`images/macos/ZPipeline.yaml`) orchestrating all 7 images
  with dependency ordering, tagged for `ghcr.io/blackleafdigital/zlayer/`
- macOS image builder script (`scripts/build-macos-images.sh`) for bootstrapping native
  rootfs images with architecture detection (arm64/x86_64)
- Registry client: two-pass platform resolver on macOS — prefers `darwin/{arch}` for
  native ZLayer sandbox images, falls back to `linux/{arch}` for Docker Hub images
- Sandbox builder: ELF binary detection on macOS — when a RUN command fails with exit
  126/127, checks if the binary is a Linux ELF and produces a clear error message
  suggesting zlayer/ base images instead of Alpine/Debian
- Sandbox builder: PATH includes rootfs bin dirs + Homebrew paths (`/opt/homebrew/bin`)
  so that macOS-native binaries installed in the image are found
- Sandbox builder: end-to-end integration tests (`sandbox_build_e2e.rs`) covering
  manifest pull, layer pull, simple build, and ENV/WORKDIR build verification
- Sandbox builder: registry image pull via `zlayer-registry` (ImagePuller + LayerUnpacker)
  when the `cache` feature is enabled, instead of requiring pre-pulled base images
- Sandbox builder: multi-stage build support -- all stages are built sequentially and
  `COPY --from=stage` resolves files from previously-built stage rootfs directories
- Sandbox builder: ARG/ENV variable substitution (`${VAR}`, `${VAR:-default}`, `$VAR`)
  applied to RUN commands, COPY/ADD sources and destinations, ENV values, WORKDIR,
  LABEL values, and USER instructions
- Sandbox builder: ADD URL sources -- downloads via `reqwest` with automatic archive
  extraction for `.tar`, `.tar.gz`, `.tgz`, `.tar.bz2`, `.tar.xz`, and `.zip` files
- Sandbox builder: ADD local archive auto-extraction for tar/gzip/bzip2/xz/zip formats
- Sandbox builder: SHELL instruction support -- custom shell stored in config and used
  for subsequent RUN shell-form commands
- Sandbox builder: HEALTHCHECK instruction -- stores command, interval, timeout,
  start_period, and retries in `SandboxImageConfig`
- Sandbox builder: USER instruction now sets the USER environment variable for RUN
  commands and resolves usernames from the rootfs `/etc/passwd`
- Sandbox builder: COPY/ADD `--chown` and `--chmod` flags applied after file operations
  using `nix::unistd::chown` and `std::os::unix::fs::PermissionsExt`

### Fixed
- Dockerfile COPY parser: fixed source/destination extraction to use the external
  parser's separated `destination` field instead of incorrectly splitting the
  `sources` vector (which already excluded the destination)
- Sandbox builder: COPY/ADD destination path handling now correctly distinguishes
  file targets from directory targets (`.`, trailing `/`, multiple sources)

## [2026-03-17]

### Added
- macOS-native image builder (`SandboxImageBuilder`) that uses the Seatbelt sandbox
  instead of buildah, enabling `zlayer build` on macOS without requiring buildah or
  a Linux VM. Supports FROM, RUN, COPY, ADD, ENV, WORKDIR, ENTRYPOINT, CMD, EXPOSE,
  ARG, LABEL, USER, VOLUME, and STOPSIGNAL instructions (single-stage builds).
- `ImageBuilder` automatically falls back to the sandbox builder on macOS when buildah
  is not installed, instead of failing with a "buildah not found" error.
- `examples/zlayer-web.zlayer.yml` deployment spec for the Leptos web frontend

### Fixed
- Daemon now regenerates the admin password if the `admin_password` file was
  deleted while the credential store still has the (Argon2id-hashed) entry,
  preventing permanently unrecoverable admin credentials
- `zlayer daemon install` and `zlayer daemon start` now write a `spawner.pid`
  marker file before launching the daemon via launchctl/systemctl, so the new
  daemon's `cleanup_stale_daemon()` skips killing the spawning CLI process
  (previously caused SIGTERM / exit 143)
- `cleanup_stale_daemon()` now reads `{data_dir}/spawner.pid` as a fallback when
  `ZLAYER_SPAWNER_PID` env var is not set (e.g., when launched via launchd/systemd),
  preventing the daemon from killing the `zlayer daemon install/start` CLI process
- GitHub Actions E2E workflow aligned with Forgejo: added `CARGO_BUILD_JOBS: "4"`,
  `--test-threads=1` for Youki tests, and post-sudo permission fix to resolve
  SQLite "database is locked" contention and test timeouts
- `daemon install` placed `--data-dir` after the `serve` subcommand, but it's a
  top-level CLI arg — clap rejected it, causing the daemon to crash-loop on start
  (both macOS launchd and Linux systemd)

### Previously added
- Containers automatically receive `ZLAYER_API_URL`, `ZLAYER_TOKEN`, and
  `ZLAYER_SOCKET` environment variables for authenticated API access
- Unix socket bind-mounted into containers for local auth bypass (Linux/Docker)
- macOS sandbox containers get socket path in writable dirs for local access
- `zlayer token show` command to retrieve admin API credentials
- Admin password persisted to `{data_dir}/admin_password` (mode 0600) on first bootstrap

### Fixed
- Overlay networking now auto-starts reliably on daemon restart by waiting for
  the WireGuard UDP port (51820) to be freed after killing the old daemon
  (fixes `errno=48` / EADDRINUSE on restart).
- Stale network interface cleanup uses `ifconfig` on macOS instead of Linux-only
  `ip` command, properly destroying orphaned utun devices with matching WireGuard
  sockets.
- Raft data directory now uses platform-specific data dir instead of hardcoded
  `/var/lib/zlayer/raft` (fixes "Permission denied" on macOS without root).
- Raft bootstrap "already bootstrapped" message downgraded to `info!` (expected
  on every restart with persistent storage).
- Overlay warning messages no longer reference Linux-only concepts (veth pairs)
  on macOS. macOS hint now directs users to `sudo` or `zlayer daemon install`.

## [0.9.990]

### Added
- Resource-aware node join: `NodeInfo` now tracks CPU cores, memory, disk, GPU resources, and
  node status (ready/draining/dead). Join flow (`POST /api/v1/cluster/join`) includes hardware
  resource information from the joining node.
- Cross-platform system resource detection (`bin/zlayer/src/resources.rs`): detects CPU, memory,
  disk, and GPU resources using Linux `/proc/meminfo`, macOS `sysctl`, and disk via `statvfs`.
- Leader self-registration: the leader node registers itself with real hardware specs on bootstrap
  instead of placeholder values.
- Worker heartbeat endpoint (`POST /api/v1/cluster/heartbeat`): worker nodes report resource
  usage every 5 seconds. Dead-node detection loop marks stale nodes as "dead" after 30 seconds
  of missed heartbeats.
- Distributed scheduling: `Scheduler::build_node_states()` converts Raft cluster state into a
  placement-ready node list. `Scheduler::compute_placement()` feeds it through the existing
  bin-packing/dedicated/exclusive placement algorithms.
- Remote container dispatch: `Scheduler::execute_distributed_scaling()` dispatches container
  creation to remote nodes via internal API calls. `apply_scaling()` now chooses distributed
  multi-node or local-only scheduling based on cluster size.
- Service assignment tracking: service-to-node assignments are replicated through Raft via
  `UpdateServiceAssignment` entries.
- Container rescheduling on node death: when the dead-node detection loop marks a node as "dead",
  the scheduler's `handle_node_death()` method automatically reschedules affected services to
  remaining live nodes using the placement algorithm.
- `Scheduler::with_raft()` constructor: accepts a pre-existing `Arc<RaftCoordinator>` so the
  daemon can share its Raft coordinator with the scheduler.
- `PlacementState::remove_node()` and `PlacementState::remove_service_from_node()`: cleanup
  methods for clearing stale container placements when a node dies.
- Node recovery: when a previously "dead" node resumes sending heartbeats, the heartbeat handler
  automatically proposes `UpdateNodeStatus { status: "ready" }` to recover it.
- Internal authentication token is now generated during daemon init and shared between the
  scheduler and API InternalState, ensuring consistent auth across the system.
- `Runtime::get_container_port_override()` trait method: allows runtimes to report a
  dynamically assigned port for containers that share the host network. Default returns
  `None` (no override). macOS sandbox runtime returns the assigned port.
- `Container.port_override` field: stores the runtime-assigned port for proxy backend
  address construction.
- Seatbelt profiles now include the dynamically assigned port in their network bind rules.
- Stabilization and endpoint reporting (Phase 11):
  - `wait_for_stabilization()` in daemon.rs: polls ServiceManager for replica count convergence and health checks with configurable timeout
  - `ServiceHealthSummary` / `StabilizationResult` types for structured stabilization outcomes
  - `DeploymentDetails` response now includes `service_health` array with per-service replica counts, health status, and endpoint URLs
  - `create_deployment` handler orchestrates when wired: parses spec, stores as Deploying, spawns async task to register services / setup overlay / configure proxy / scale, updates to Running or Failed
  - `delete_deployment` handler tears down services (scale to 0, remove) before deleting from storage
  - `DeploymentState.with_orchestration()` constructor wires ServiceManager, OverlayManager, and ProxyManager into the handler
  - `build_router_with_deployment_state()` router builder accepts pre-built DeploymentState for orchestration
  - Enhanced CLI deploy output: success shows per-service endpoints and replica counts from daemon, failure shows per-service status with health info
  - CLI poll loop uses live `service_health` data from daemon instead of spec-based estimates
  - Serve command wires orchestration handles into DeploymentState so API deployments are fully orchestrated
- Raft distributed scheduler integration (Phase 10):
  - `RaftCoordinator` integration in daemon init with `PersistentRaftStorage` (SQLite-backed)
  - `RaftService` background Raft RPC server for cluster communication
  - `add_member()` for dynamic cluster membership changes
  - `POST /api/v1/cluster/join` and `GET /api/v1/cluster/nodes` API endpoints with `ClusterApiState`
  - Join token validation with HMAC-based authentication
  - CIDR-aware IP allocation via `IpAllocator` for overlay addresses (collision-safe)
- Secrets wiring into container environment (Phase 8):
  - `BundleBuilder.with_secrets_provider()` / `with_deployment_scope()`: thread secrets provider through OCI bundle creation
  - `$S:secret-name` env vars resolved at container start time via `resolve_env_with_secrets`
  - Falls back to `$E:` (host env) resolution when no secrets provider is configured
  - `CredentialStore`: Argon2id-based API key authentication built on `PersistentSecretsStore`
  - `CredentialStore.validate()`, `create_api_key()`, `ensure_admin()` for credential lifecycle
  - Blanket `SecretsProvider` / `SecretsStore` impls for `Arc<T>` enabling shared ownership
  - Auth handler (`/auth/token`) validates against `CredentialStore` instead of hardcoded dev credentials
  - `AuthState.credential_store` / `ApiConfig.credential_store` for injecting credential store into API
  - Admin credential bootstrapped on first daemon start (random password logged once)
  - Unix socket connections auto-injected with admin JWT via `run_dual_with_local_auth()`
  - Local CLI-to-daemon IPC requires no explicit authentication
- TCP/UDP L4 proxy wiring (Phase 6):
  - `TcpStreamService.serve()`: standalone accept loop for TCP proxying without Pingora infrastructure
  - `UdpStreamService.serve()`: accepts externally-bound `UdpSocket` for UDP proxying
  - `ProxyManager` now handles TCP/UDP protocols in `ensure_ports_for_service()`: binds listeners, spawns stream proxy tasks
  - `ProxyManager.set_stream_registry()` / `with_stream_registry()`: wires L4 stream registry
  - `StreamService` health-aware backend selection: `BackendHealth` enum (Healthy/Unhealthy/Unknown), skips unhealthy backends in round-robin
  - `StreamRegistry.spawn_health_checker()`: background TCP connect probe (every 5s, 2s timeout)
  - Daemon wires `StreamRegistry` into both `ProxyManager` and `ServiceManager`
  - Health checker task integrated into daemon lifecycle (started on init, aborted on shutdown)
- Public vs Internal endpoint enforcement (Phase 7):
  - Public endpoints (`expose: public`) bind to `0.0.0.0` (all interfaces)
  - Internal endpoints (`expose: internal`) bind to the overlay IP, or `127.0.0.1` if no overlay is available
  - Defense-in-depth: proxy rejects non-overlay sources (outside `10.200.0.0/16`) for internal routes with HTTP 403
  - `OverlayManager.node_ip()` getter exposes the node's global overlay IP
  - Removed duplicate `ExposeType` enum from `zlayer-tunnel`; now imports from `zlayer-spec`
- Container logging (Phase 5):
  - Structured log paths: container stdout/stderr written to `/var/log/zlayer/{deployment}/{service}/{container_id}.{stdout,stderr}.log`
  - `YoukiConfig.log_base_dir` / `YoukiConfig.deployment_name`: configurable log directory hierarchy
  - Bundle symlinks: `{bundle}/logs/{stdout,stderr}.log` symlink to structured paths for backward compatibility
  - Daemon auto-configures Youki runtime with log directory and deployment name on Linux
  - Structured log rotation: hourly task rotates oversized files in `/var/log/zlayer/` (keeps last 10%)
  - Old log cleanup removes `.log` files older than 7 days from the structured log directory
- GPU inventory detection module (`zlayer-agent::gpu_detector`) that scans sysfs PCI devices for GPUs
  - Auto-detects vendor (NVIDIA, AMD, Intel) via PCI vendor IDs
  - Reads VRAM from PCI BAR regions, AMD `mem_info_vram_total`, or nvidia-smi
  - Reads GPU model names from DRM subsystem or nvidia-smi
  - Resolves device paths (`/dev/nvidiaN`, `/dev/dri/cardN`, `/dev/dri/renderDN`)
- GPU fields on `NodeResources` in scheduler placement: `gpu_total`, `gpu_used`, `gpu_models`, `gpu_memory_mb`, `gpu_vendor`, and `gpu_available()` helper
- `GpuInfoSummary` struct and `gpus` field on `NodeInfo` in Raft cluster state for distributed GPU awareness
- GPU detection runs at node init and node join, with results logged and displayed
- `zlayer exec` command: batch command execution in running containers via the daemon API
- `zlayer ps` command: lists deployments, services, and containers via the daemon API
  - Supports `--deployment` filter, `--containers` flag for replica detail, `--format` (table/json/yaml)
  - Container listing API endpoint: `GET /api/v1/deployments/{name}/services/{service}/containers`
- Deploy TUI with ratatui-based interactive progress display for `zlayer deploy` and `zlayer up`
  - Phase-aware layout: deploying, running dashboard, and shutdown views
  - Infrastructure phase progress indicators (spinners, checkmarks)
  - Per-service deployment status with replica counts and health indicators
  - Scrollable log pane capturing warnings and errors
  - `--no-tui` flag for CI/non-interactive environments with clean PlainDeployLogger fallback
- `restore_deployments()`: automatic deployment recovery on daemon restart from SQLite storage
- `DaemonClient`: HTTP-over-Unix-socket client with auto-start daemon and exponential backoff retry
- `load_or_init_node_config()`: node configuration auto-initialization with WireGuard key generation on first run
- Overlay tunnel setup on node join: configures WireGuard peers, TUN interface, and persists bootstrap state for daemon reload
- Shared stabilization module: `wait_for_stabilization()` moved to `zlayer-agent` crate for reuse across daemon and API
- Deployment delete cleanup: `delete_deployment` handler tears down overlay, proxy routes, and service replicas before removing storage
- DNS handle lifecycle: `DnsHandle` kept alive in `DaemonState` for runtime record management
- Glob-based spec file discovery: `zlayer deploy` now finds `*.zlayer.yml` and `*.zlayer.yaml` files
- Pipeline file auto-discovery: accepts `ZPipeline.yaml` or `zlayer-pipeline.yaml` (no longer requires `-f` flag)
- Per-crate log filtering: default verbosity reduced to WARN for internal crates, use `-v` for old behavior

### Changed
- zlayer-consensus: replaced serde_json round-trip in state machine apply with direct
  `EntryPayload` matching (~10x faster apply). Switched to consistent bincode serialization
  for snapshots. Removed unnecessary serde_json dependency.
- Replaced kernel WireGuard with boringtun (Cloudflare's Rust userspace WireGuard implementation) for overlay networking
  - No longer requires WireGuard kernel module (`modprobe wireguard` / kernel 5.6+)
  - No longer requires `wireguard-tools` package (`wg` binary)
  - Uses TUN device via `/dev/net/tun` (universally available on Linux)
  - Still requires CAP_NET_ADMIN or root for TUN device creation
  - Renamed `WireGuardManager` to `OverlayTransport`
  - All configuration now done via UAPI protocol (no external binary dependencies)
- `zlayer manager init` now creates `manager.zlayer.yml` instead of `.zlayer.yml`
- Deploy output uses event-driven architecture (DeployEvent channel) instead of mixed println/tracing
- API server default port changed from 8080 to 3669
- Overlay network failure is now fatal when services require networking (use `--host-network` to bypass)

### Fixed
- zlayer-consensus: `handle_full_snapshot` was a no-op (snapshot data was received but never
  installed into the state machine). Now correctly deserializes and installs snapshot state.
- zlayer-consensus: `snapshot_logs_since_last` and `enable_prevote` config settings were silently
  ignored during Raft node initialization. Both are now applied to the openraft `Config`.
- Overlay container attachment: added platform guard (`#[cfg(not(target_os = "linux"))]`) to
  `OverlayManager::attach_container()` so non-Linux platforms skip veth/nsenter commands and
  return the node's overlay IP directly, eliminating noisy error logs on macOS.
- macOS sandbox networking: multiple replicas of the same service no longer conflict on ports.
  Each sandbox container is assigned a unique dynamic port (via OS port-0 allocation), passed
  to the process as `PORT` / `ZLAYER_PORT` environment variables. The proxy routes to each
  replica's unique `127.0.0.1:{assigned_port}` backend address instead of all replicas sharing
  the same port. Port guard listeners prevent TOCTOU races between `create_container()` and
  `start_container()`.
- Proxy backend registration: containers are now registered with the load balancer on startup (previously backends were never added, causing "No healthy backends" errors)
- Health check target address: TCP and HTTP health checks now connect to the container's overlay IP instead of 127.0.0.1
- Error reporting now includes service names and error details in deploy output
- Youki runtime reads image CMD, ENTRYPOINT, Env, WorkingDir, and User from OCI image config
- Docker runtime cleans up stale containers before re-deploy
- Integration tests un-ignored in `youki_e2e.rs`
