# RAFT_TODO — `raft-e2e` ship-blocker, for the Raft agent

Goal: `raft-e2e` GREEN — all 5 suites: `cluster_3node`, `cluster_failover`, `cluster_node_upgrade`,
`cluster_scaling`, `cluster_upgrade`. After any change: `cargo fmt --all` → `clippy --workspace
--all-targets -- -D warnings` → `test --workspace` → `build --workspace`, all green (never `-p`).
Mirror shared-crate changes to `/Users/zach/GitHub/zlayer-zql` + bump its CHANGELOG. Commit markers
`[np]`/`[fast]`; NEVER `[skip ci]`.

---

## CURRENT STATE (2026-06-03) — the root cause was a DEADLOCK, now fixed; verifying

### THE FIX (already shipped: `cfd18bff` on dev, mirrored to zql)
Every prior `raft-e2e` run failed with **all 5 suites → "node1 never came up on 127.0.0.1:19110"**: the
node ran `node init` fine, then `zlayer serve` NEVER bound its API listener, so the harness's 60s
`/health/ready` poll timed out. `readiness()` (`crates/zlayer-api/src/handlers/health.rs:34`) is an
unconditional 200, so the listener simply never came up.

**Root cause: a stderr feedback DEADLOCK, not anything raft-specific.** Commit `51383c54` moved the
observability **console** tracing layer stdout→stderr. But `serve` installs
`install_stderr_redirect_to_tracing()` (`bin/zlayer/src/commands/serve.rs:1263`) which `dup2`'s fd 2
onto a pipe whose reader re-emits each line as `tracing::error!`. With the console layer ALSO writing
to fd 2, every event looped back through that pipe; the pipe filled and `write_all` blocked **while
holding the global stderr mutex** → the daemon deadlocked mid-`init` (proven locally with `sample`:
main thread parked in `Stderr::lock` from `StorageBundle::open`, serve.rs:3066). Only triggers when
stderr is non-TTY = the CI nodes (serve stderr → file). Before `51383c54`, console went to stdout (fd
1), disjoint from the fd-2 capture — which is why it used to work.

Fix in `crates/zlayer-observability/src/logging.rs`: the **daemon** arms of `init_logging` (the 3 arms
with a `file_writer` — only `zlayer serve` configures file logging) route the console layer to
**stdout** (disjoint from the fd-2 capture); the **CLI** arms (no file writer) keep **stderr** so
`ps --containers --format json` stdout stays clean (preserving the original `51383c54` intent — that
JSON-parse fix is still satisfied because the e2e's `ps` is a separate CLI process). Also bounded the
overlayd dial (`OverlaydClient::connect_with_attempts`) so a dead overlayd degrades in ~2.5s not ~35s.

### RESULT of run #1701 (cfd18bff): deadlock fix took raft from 0/5 → **3/5**. Two real bugs left.
The nodes now bind (deadlock gone). Suite verdicts:
- `cluster_3node` ✅ OK
- `cluster_failover` ✅ OK
- `cluster_node_upgrade` ✅ OK
- **`cluster_scaling` ❌ FAIL** — `RuntimeError("expected replicas spread across at least 2 nodes; got
  distinct={'1'} from ['1','1','1']")`. The scheduler places ALL 3 replicas on node 1 — no spread
  across the cluster. This is a REAL **scheduler placement** bug (spread/anti-affinity across raft
  nodes), not a test relax. Investigate `crates/zlayer-scheduler` placement + how the raft leader
  assigns replicas to nodes; the e2e asserts replicas land on ≥2 distinct `node_id`s. Fix the placement
  so replicas spread.
- **`cluster_upgrade` ❌ FAIL** — `RuntimeError("deployment e2e-cluster-app never converged to 3
  containers on image docker.io/library/nginx:1.29-alpine within 180s; last_running=[(…1.28-alpine,
  running)x3]")`. A rolling **image upgrade** (1.28→1.29) never replaces the running containers — they
  stay on the old image past the 180s window. REAL **reconcile/rollout** bug: the deploy-update path
  isn't tearing down old-image containers and bringing up new-image ones across the cluster.
  Investigate the deployment-update/reconcile path (`zlayer-scheduler` + `zlayer-agent` service
  reconcile + how an image change is propagated to worker nodes and triggers container replacement).

### REMAINING = fix those 2 scheduler/reconcile bugs, then re-dispatch raft-e2e to 5/5.
Both are genuine orchestration bugs surfaced now that nodes actually come up. Use the forgejo-actions
sidecar to read logs. After fixing, dispatch a fresh `raft-e2e` on the latest dev and confirm 5/5.
(Old note: run #1701 reached 984 log lines — the 3 passing suites + the 2 failures above.)
- Read CI logs via the **forgejo-actions sidecar** (REST logs 404 on this Forgejo): streamable-HTTP
  JSON-RPC at `https://forge-actions.blackleafdigital.com/mcp`, bearer = vault item "Claude Code MCP:
  forgejo-actions" (`FORGEJO_ACTIONS_MCP_API_KEY`). initialize→capture `mcp-session-id`→
  notifications/initialized→tools/call. Tools: `list_runs` (repo MUST be `owner/name`, single param),
  `get_run_detail{run_id,include_steps}` (the sidecar's job `id` ≠ forgejo task_id — get it from
  get_run_detail), `tail_job_log{run_id,job_id,grep,tail_lines,strip_ansi,strip_timestamps}` (live log
  is unreadable until Forgejo flushes chunks; works once steps complete). Dispatch:
  `mcp__forgejo__dispatch_workflow {owner:BlackLeafDigital, repo:ZLayer, workflow_filename:e2e.yml,
  ref:dev, inputs:{test:raft-e2e}}`. e2e.yml is `workflow_dispatch`-only; raft job pinned
  `runs-on: nvidia` (privileged GPU box).

### IF A LATER LAYER SURFACES
- **`cluster_scaling` spread:** the handoff flagged that in run #1697 all 3 replicas landed on
  `node_id:"1"` (no spread across nodes). If `cluster_scaling` asserts replicas spread across ≥2 nodes
  and fails, the scheduler placement needs attention (real fix, not a test relax).
- **Per-node logs not captured:** run #1699's artifact upload found NOTHING (`target/zlayer-e2e/**`
  empty at upload time) — the host-process suites' per-node stderr (`<THROWAWAY_ROOT>/logs/node{i}.err`,
  `THROWAWAY_ROOT = REPO_ROOT/target/zlayer-e2e`) is cleaned before upload or written elsewhere. If you
  need a node's serve stderr, FIX the artifact capture first (ensure node logs survive to the upload
  step; check the suite `finally:`/cleanup in `run-suite.py` and whether `KEEP_E2E_ARTIFACTS` gates
  retention) then re-dispatch.

---

## FULL HISTORY (multi-layer onion already peeled — each fix on dev)
1. **overlayd not built in CI** → `serve` self-spawns `zlayer-overlayd` (separate binary); absent →
   dial stall > health timeout. Fix: build `-p zlayer -p zlayer-overlayd` in `.forgejo/workflows/e2e.yml`
   + `.github/workflows/e2e.yml` + `run-suite.py` build phase; added `crates/zlayer-overlayd` to the
   raft fingerprint; bumped per-node health wait 30→60s. (`530d2058`)
2. **Linux-only compile error** in `crates/zlayer-overlayd/src/server.rs`: `IpNet::contains(ip)` →
   `contains(&ip)` + unused import. (`95e1f04f`)
3. **Unqualified images** rejected by registry → qualified `cluster-specs/*.yaml` to
   `docker.io/library/nginx:…` + `_norm_image` tolerant compare in `run-suite.py`. (`530d2058`/`35c20e5e`)
4. **cgroups:** job container at cgroup-v2 root with `/sys/fs/cgroup` read-only when not privileged.
   Cgroup-prep step (`remount,rw` + delegate `/zlayer-e2e` + export `ZLAYER_CGROUP_PARENT`) (`a99d238f`)
   + pinned `runs-on: nvidia` (`a391ab41`).
5. **Stabilization `healthy=false`:** `crates/zlayer-agent/src/service.rs` (~615-695) only bridged the
   health monitor into `health_states` INSIDE `if let (Some(proxy), Some(ip))`; in CI there's no
   reachable `effective_ip` → no bridge → forever unhealthy. Fix: register the `health_states` bridge
   UNCONDITIONALLY (proxy backend update stays gated). Specs declare a host-side `command:"true"` health
   check. (`78c671ee`)
6. **JSON pollution:** `ps --containers --format json` stdout had INFO `{"timestamp":…}` lines → the
   suite's `json.loads` died `Extra data`. Fix attempt `51383c54` moved console→stderr — which
   introduced the DEADLOCK above. The CORRECT fix (`cfd18bff`) routes only the DAEMON console to stdout
   and keeps the CLI console on stderr.

## Critical files
`bin/zlayer/src/commands/serve.rs` (init order; `install_stderr_redirect_to_tracing` ~1263; API bind
via `serve_bound` ~2833 after `init_daemon` ~1495), `bin/zlayer/src/daemon.rs` (overlay ~697, raft
~1172-1370), `crates/zlayer-observability/src/logging.rs` (the fix), `crates/zlayer-agent/src/
service.rs` (health bridge), `…/stabilization.rs`, `…/health.rs`, `bin/zlayer/src/commands/ps.rs`,
`crates/zlayer-manager/tests/e2e/{scripts/run-suite.py, cluster-specs/*.yaml}`,
`.forgejo/workflows/e2e.yml` (raft job pinned `runs-on: nvidia`) + `.github/workflows/e2e.yml`.

## DONE = run #1701 (or a fresh dispatch on the latest dev) shows raft-e2e 5/5 green.
