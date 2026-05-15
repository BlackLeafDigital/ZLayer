#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# ///
"""Hermetic runner for the zlayer-manager Intellitester suite.

Default mode connects to the user's locally-installed daemon over its
Unix socket (auto-admin). `--throwaway` spins up an isolated daemon
under `target/zlayer-e2e/<suite-id>/` instead.

Either way the harness creates a scoped fixture admin via
`zlayer user create`, boots the manager pointed at the resolved daemon,
drives login + nav-smoke + stale-session through intellitester, and
tears the fixture down on exit. Daemon-side `delete_user` is idempotent
(204 in both cases) so cleanup never special-cases the missing-user
error.

Stdlib only. Invoke via `uv run`.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import secrets
import shutil
import signal
import stat
import subprocess
import sys
import time
import urllib.error
import urllib.request
from contextlib import suppress
from pathlib import Path
from typing import Optional


# Script lives at:
#   crates/zlayer-manager/tests/e2e/scripts/run-suite.py
# parents[0]=scripts, [1]=e2e, [2]=tests, [3]=zlayer-manager,
# parents[4]=crates, [5]=repo root.
REPO_ROOT = Path(__file__).resolve().parents[5]
E2E_DIR = REPO_ROOT / "crates" / "zlayer-manager" / "tests" / "e2e"
THROWAWAY_ROOT = REPO_ROOT / "target" / "zlayer-e2e"

INTELLITESTER_PIN = "intellitester@^0.4.5"

# Throwaway-mode daemon ports (match the bash version's defaults).
API_PORT = int(os.environ.get("ZLAYER_E2E_API_PORT", "13669"))
WG_PORT = int(os.environ.get("ZLAYER_E2E_WG_PORT", "51421"))
DNS_PORT = int(os.environ.get("ZLAYER_E2E_DNS_PORT", "15354"))
# Manager bind port: 16677 historically to dodge a canonical install on 6677.
MANAGER_PORT = int(os.environ.get("ZLAYER_E2E_MANAGER_PORT", "16677"))

HEALTHCHECK_TIMEOUT_S = 60
SOCKET_WAIT_S = 30
TERM_GRACE_S = 5


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def log(msg: str) -> None:
    print(f"==> {msg}", flush=True)


def die(msg: str, code: int = 1) -> "Optional[int]":
    print(msg, file=sys.stderr, flush=True)
    sys.exit(code)


def is_socket(path: Path) -> bool:
    try:
        return stat.S_ISSOCK(path.stat().st_mode)
    except OSError:
        return False


def _wait_http_ok(url: str, timeout_s: int) -> bool:
    """Poll a URL once a second until any non-5xx response returns."""
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=2) as resp:
                if resp.status < 500:
                    return True
        except urllib.error.HTTPError as e:
            if e.code < 500:
                return True
        except (urllib.error.URLError, ConnectionError, TimeoutError, OSError):
            pass
        time.sleep(1)
    return False


def _kill_pg(proc: subprocess.Popen, name: str, *, sudo: bool = False) -> None:
    """SIGTERM the process group, wait, SIGKILL if alive.

    When `sudo=True` and we're not already root, signals are delivered via
    `sudo kill` so that root-owned processes (the daemon under
    `--sudo-daemon`) get killed despite the harness running unprivileged.
    """
    if proc.poll() is not None:
        return
    try:
        pgid = os.getpgid(proc.pid)
    except ProcessLookupError:
        return
    log(f"Killing {name} (pgid={pgid})")
    need_sudo = sudo and os.geteuid() != 0

    def _send(sig: int) -> None:
        if need_sudo:
            subprocess.run(
                ["sudo", "kill", f"-{int(sig)}", f"-{pgid}"],
                check=False,
            )
        else:
            with suppress(ProcessLookupError, PermissionError):
                os.killpg(pgid, sig)

    _send(signal.SIGTERM)
    try:
        proc.wait(timeout=TERM_GRACE_S)
        return
    except subprocess.TimeoutExpired:
        pass
    _send(signal.SIGKILL)
    with suppress(subprocess.TimeoutExpired):
        proc.wait(timeout=TERM_GRACE_S)


def _maybe_sudo(argv: list[str], *, sudo: bool) -> list[str]:
    """Prefix argv with `sudo -E env PATH=... HOME=...` iff requested and not root."""
    if sudo and os.geteuid() != 0:
        return [
            "sudo", "-E", "env",
            f"PATH={os.environ.get('PATH', '')}",
            f"HOME={os.environ.get('HOME', '')}",
            *argv,
        ]
    return argv


# ---------------------------------------------------------------------------
# Resolution
# ---------------------------------------------------------------------------

def resolve_zlayer_bin(*, throwaway: bool) -> Path:
    built = REPO_ROOT / "target" / "release" / "zlayer"
    if throwaway:
        if not built.exists():
            die(
                f"zlayer binary not found at {built}; "
                "build it first (drop --no-build) or place it on PATH."
            )
        return built
    on_path = shutil.which("zlayer")
    if on_path:
        return Path(on_path)
    if not built.exists():
        die(f"no `zlayer` on PATH and no built binary at {built}")
    return built


def resolve_host_socket() -> Optional[Path]:
    candidates: list[Path] = []
    env_sock = os.environ.get("ZLAYER_SOCKET")
    if env_sock:
        candidates.append(Path(env_sock))
    candidates.append(Path("/var/run/zlayer.sock"))
    candidates.append(Path.home() / ".zlayer" / "run" / "zlayer.sock")
    for p in candidates:
        if is_socket(p):
            return p
    return None


# ---------------------------------------------------------------------------
# Phases
# ---------------------------------------------------------------------------

def build_phase(*, throwaway: bool, with_manager: bool = True) -> None:
    on_path = shutil.which("zlayer") is not None
    if throwaway or not on_path:
        log("Building zlayer (release)")
        subprocess.run(
            ["cargo", "build", "--release", "-p", "zlayer"],
            cwd=REPO_ROOT, check=True,
        )
    else:
        log(f"Skipping zlayer build (using {shutil.which('zlayer')})")
    if with_manager:
        log("Building zlayer-manager (release, cargo-leptos)")
        subprocess.run(
            ["cargo", "leptos", "build", "--release"],
            cwd=REPO_ROOT / "crates" / "zlayer-manager", check=True,
        )


def start_throwaway_daemon(
    data_dir: Path, zlayer_bin: Path, *, sudo: bool,
) -> tuple[subprocess.Popen, Path]:
    log(f"Starting throwaway daemon on 127.0.0.1:{API_PORT} (data-dir: {data_dir})")
    argv = _maybe_sudo(
        [
            str(zlayer_bin),
            "--data-dir", str(data_dir),
            "serve",
            "--bind", f"127.0.0.1:{API_PORT}",
            "--deployment-name", "zlayer-e2e",
            "--wg-port", str(WG_PORT),
            "--dns-port", str(DNS_PORT),
        ],
        sudo=sudo,
    )
    env = {
        **os.environ,
        "ZLAYER_JWT_SECRET": os.environ.get(
            "ZLAYER_JWT_SECRET",
            "e2e-secret-do-not-use-in-prod-do-not-share-this-key-1234567890",
        ),
    }
    proc = subprocess.Popen(argv, cwd=REPO_ROOT, env=env, start_new_session=True)

    ready_url = f"http://127.0.0.1:{API_PORT}/health/ready"
    if not _wait_http_ok(ready_url, HEALTHCHECK_TIMEOUT_S):
        _kill_pg(proc, "daemon (failed startup)", sudo=sudo)
        die(f"throwaway daemon never came up on 127.0.0.1:{API_PORT}")

    # Read the bound socket path from daemon.json rather than synthesising
    # it. The daemon's path resolver substitutes `/tmp/zlayer-daemon-<hash>.sock`
    # when `{data_dir}/run/zlayer.sock` would overflow `sockaddr_un.sun_path`
    # (Linux 108 / macOS 104); only the daemon knows which path actually got
    # bound, so we trust daemon.json (struct DaemonMetadata in
    # bin/zlayer/src/commands/serve.rs, field `socket_path`).
    meta_path = data_dir / "daemon.json"
    deadline = time.monotonic() + SOCKET_WAIT_S
    sock: Optional[Path] = None
    while time.monotonic() < deadline:
        if meta_path.is_file():
            try:
                meta = json.loads(meta_path.read_text())
            except (OSError, json.JSONDecodeError):
                meta = None
            if isinstance(meta, dict):
                sp = meta.get("socket_path")
                if isinstance(sp, str) and sp:
                    candidate = Path(sp)
                    if is_socket(candidate):
                        sock = candidate
                        break
        time.sleep(1)
    if sock is None:
        _kill_pg(proc, "daemon (no socket)", sudo=sudo)
        die(
            "throwaway daemon socket never appeared "
            f"(daemon.json at {meta_path} missing or socket_path stale; "
            "see daemon stderr above)"
        )
        raise RuntimeError("unreachable")  # for type checkers
    return proc, sock


def create_fixture_user(
    zlayer_bin: Path, cli_data_dir: Optional[Path],
    email: str, password: str, display: str,
) -> str:
    log(f"Creating fixture user {email} (admin)")
    env = {**os.environ}
    if cli_data_dir:
        env["ZLAYER_DATA_DIR"] = str(cli_data_dir)
    try:
        result = subprocess.run(
            [
                str(zlayer_bin), "user", "create",
                "--email", email,
                "--password", password,
                "--role", "admin",
                "--display-name", display,
            ],
            env=env, capture_output=True, text=True, check=True,
        )
    except subprocess.CalledProcessError as exc:
        if exc.stdout:
            sys.stderr.write(f"--- user create stdout ---\n{exc.stdout}\n")
        if exc.stderr:
            sys.stderr.write(f"--- user create stderr ---\n{exc.stderr}\n")
        raise
    # Stdout: `Created user {display} <{email}> (role: {role}, id: {uuid})`
    match = re.search(r"id:\s*([0-9a-fA-F-]{36})\)", result.stdout)
    if not match:
        die(
            "could not parse fixture user id from CLI output:\n"
            f"{result.stdout}"
        )
        raise RuntimeError("unreachable")
    user_id = match.group(1)
    log(f"    id={user_id}")
    return user_id


def delete_fixture_user(
    zlayer_bin: Path, cli_data_dir: Optional[Path], user_id: str,
) -> None:
    env = {**os.environ}
    if cli_data_dir:
        env["ZLAYER_DATA_DIR"] = str(cli_data_dir)
    # Daemon-side delete is idempotent (204 in both cases). Transport
    # failures during cleanup are tolerated.
    result = subprocess.run(
        [str(zlayer_bin), "user", "delete", user_id, "--yes"],
        env=env, capture_output=True, text=True,
    )
    if result.returncode != 0:
        print(
            f"!! fixture user delete exit={result.returncode}: "
            f"{result.stderr.strip()}",
            file=sys.stderr, flush=True,
        )


def start_manager(socket_path: Path) -> subprocess.Popen:
    log(f"Starting manager on 127.0.0.1:{MANAGER_PORT} (talking to {socket_path})")
    env = {
        **os.environ,
        "ZLAYER_SOCKET": str(socket_path),
        "LEPTOS_SITE_ADDR": f"127.0.0.1:{MANAGER_PORT}",
        # leptos_options.site_root defaults to a relative path; pin it
        # absolute so the binary serves /pkg/* whatever the cwd.
        "LEPTOS_SITE_ROOT": str(REPO_ROOT / "target" / "site"),
        # LEPTOS_HASH_FILES is a runtime flag; cargo-leptos's compile-time
        # setting doesn't carry into a plain binary invocation.
        "LEPTOS_HASH_FILES": "true",
        "RUST_LOG": os.environ.get("RUST_LOG", "info,zlayer_manager=debug"),
    }
    binary = REPO_ROOT / "target" / "release" / "zlayer-manager"
    if not binary.exists():
        die(f"zlayer-manager binary not found at {binary}; drop --no-build")
    proc = subprocess.Popen(
        [str(binary)], cwd=REPO_ROOT, env=env, start_new_session=True,
    )
    if not _wait_http_ok(
        f"http://127.0.0.1:{MANAGER_PORT}/login", HEALTHCHECK_TIMEOUT_S,
    ):
        _kill_pg(proc, "manager (failed startup)")
        die(f"manager never came up on 127.0.0.1:{MANAGER_PORT}")
    return proc


def run_intellitester(
    yaml_path: Path, *,
    extra_args: Optional[list[str]] = None,
    env_extras: Optional[dict[str, str]] = None,
) -> None:
    argv = ["pnpm", "dlx", "--silent", INTELLITESTER_PIN, "run"]
    if extra_args:
        argv.extend(extra_args)
    argv.append(str(yaml_path))
    env = {**os.environ, **(env_extras or {})}
    subprocess.run(argv, cwd=E2E_DIR, env=env, check=True)


# ---------------------------------------------------------------------------
# Cluster suites (cluster_3node, cluster_failover)
# ---------------------------------------------------------------------------
#
# These suites exercise the multi-node consensus + failover paths backed by
# the README §385-387 heartbeat-health-monitoring claims. They share the
# 3-node bootstrap path via `_bootstrap_3node_cluster`.

# Per-node ports for the local 3-node throwaway cluster. Picked above the
# manager-suite range so the two cannot collide.
CLUSTER_NODES: list[dict[str, int]] = [
    {"api": 19110, "raft": 19111, "overlay": 51410},
    {"api": 19120, "raft": 19121, "overlay": 51420},
    {"api": 19130, "raft": 19131, "overlay": 51430},
]
CLUSTER_DEPLOYMENT = "zlayer-e2e-cluster"
CLUSTER_NODES_READY_TIMEOUT_S = 60
CLUSTER_NODE_DEAD_TIMEOUT_S = 45
CLUSTER_NODE_RECOVER_TIMEOUT_S = 60


def _parse_join_token(stdout: str) -> str:
    """Extract the join token from `zlayer node generate-join-token` stdout.

    The CLI prints a labeled block (see
    `bin/zlayer/src/commands/node.rs::handle_node_generate_join_token`):

        Join Token Generated
        ====================

        Deployment: <name>
        API: <endpoint>

        Token:
        <base64-url-no-pad-token>

        Usage:
          zlayer node join <leader-addr> --token <same-token>

    Strategy: find the line that is exactly `Token:`, return the next
    non-empty line. Fall back to scanning the `--token <X>` usage line.
    Fall back further to picking the longest base64url-shaped line.
    """
    lines = [ln.rstrip() for ln in stdout.splitlines()]
    for i, ln in enumerate(lines):
        if ln.strip() == "Token:":
            for j in range(i + 1, len(lines)):
                cand = lines[j].strip()
                if cand:
                    return cand
    for ln in lines:
        m = re.search(r"--token\s+(\S+)", ln)
        if m:
            return m.group(1)
    # Last-ditch: longest base64url-looking token on any line.
    best = ""
    for ln in lines:
        for tok in ln.split():
            if len(tok) >= 32 and re.fullmatch(r"[A-Za-z0-9_\-]+", tok):
                if len(tok) > len(best):
                    best = tok
    if best:
        return best
    raise RuntimeError(
        "could not parse join token from `node generate-join-token` "
        f"stdout:\n{stdout}"
    )


def _cluster_nodes_url(api_port: int) -> str:
    return f"http://127.0.0.1:{api_port}/api/v1/cluster/nodes"


def _fetch_cluster_nodes(api_port: int) -> list[dict]:
    """GET the leader's `/api/v1/cluster/nodes` and return the parsed list.

    The endpoint may be unauthenticated on loopback or may require a JWT —
    we honor ZLAYER_E2E_CLUSTER_TOKEN as an opt-in bearer override. On any
    transport/HTTP failure we raise so the caller can poll.
    """
    req = urllib.request.Request(_cluster_nodes_url(api_port))
    bearer = os.environ.get("ZLAYER_E2E_CLUSTER_TOKEN")
    if bearer:
        req.add_header("Authorization", f"Bearer {bearer}")
    with urllib.request.urlopen(req, timeout=5) as resp:
        body = resp.read().decode("utf-8")
    data = json.loads(body)
    # Endpoint historically returns either a bare array or {"nodes": [...]}.
    if isinstance(data, dict) and "nodes" in data:
        return list(data["nodes"])
    if isinstance(data, list):
        return data
    raise RuntimeError(
        f"unexpected /api/v1/cluster/nodes payload shape: {data!r}"
    )


def _spawn_node_serve(
    zlayer_bin: Path, data_dir: Path, api_port: int,
) -> subprocess.Popen:
    """Spawn `zlayer serve` for a cluster node (no --daemon, killable group)."""
    env = {
        **os.environ,
        "ZLAYER_JWT_SECRET": os.environ.get(
            "ZLAYER_JWT_SECRET",
            "e2e-secret-do-not-use-in-prod-do-not-share-this-key-1234567890",
        ),
    }
    argv = [
        str(zlayer_bin),
        "--data-dir", str(data_dir),
        "serve",
        "--bind", f"127.0.0.1:{api_port}",
        "--deployment-name", CLUSTER_DEPLOYMENT,
    ]
    return subprocess.Popen(argv, cwd=REPO_ROOT, env=env, start_new_session=True)


def _bootstrap_3node_cluster(
    zlayer_bin: Path, root_dir: Path,
) -> tuple[list[subprocess.Popen], list[Path], list[int]]:
    """Stand up a 3-node loopback cluster.

    Returns (procs, data_dirs, api_ports). Caller is responsible for
    `_kill_pg`-ing every proc in `procs` in its `finally:` block.
    """
    data_dirs: list[Path] = []
    for i in range(3):
        d = root_dir / f"node{i + 1}" / "data"
        d.mkdir(parents=True, exist_ok=True)
        data_dirs.append(d)

    api_ports = [CLUSTER_NODES[i]["api"] for i in range(3)]
    procs: list[subprocess.Popen] = []

    # --- Node 1: init (bootstrap leader) -----------------------------------
    log(f"cluster: initializing node1 (api={CLUSTER_NODES[0]['api']})")
    subprocess.run(
        [
            str(zlayer_bin),
            "--data-dir", str(data_dirs[0]),
            "node", "init",
            "--advertise-addr", "127.0.0.1",
            "--api-port", str(CLUSTER_NODES[0]["api"]),
            "--raft-port", str(CLUSTER_NODES[0]["raft"]),
            "--overlay-port", str(CLUSTER_NODES[0]["overlay"]),
        ],
        check=True, cwd=REPO_ROOT,
    )

    # --- Node 1: serve in the background -----------------------------------
    log(f"cluster: starting node1 serve on 127.0.0.1:{CLUSTER_NODES[0]['api']}")
    n1 = _spawn_node_serve(zlayer_bin, data_dirs[0], CLUSTER_NODES[0]["api"])
    procs.append(n1)
    if not _wait_http_ok(
        f"http://127.0.0.1:{CLUSTER_NODES[0]['api']}/health/ready", 30,
    ):
        die(f"node1 never came up on 127.0.0.1:{CLUSTER_NODES[0]['api']}")

    # --- Generate join token from the leader -------------------------------
    log("cluster: generating join token on node1")
    try:
        gen = subprocess.run(
            [
                str(zlayer_bin),
                "--data-dir", str(data_dirs[0]),
                "node", "generate-join-token",
                CLUSTER_DEPLOYMENT,
                "-a", f"http://127.0.0.1:{CLUSTER_NODES[0]['api']}",
            ],
            capture_output=True, text=True, check=True, cwd=REPO_ROOT,
        )
    except subprocess.CalledProcessError as exc:
        if exc.stdout:
            sys.stderr.write(
                f"--- node generate-join-token stdout ---\n{exc.stdout}\n"
            )
        if exc.stderr:
            sys.stderr.write(
                f"--- node generate-join-token stderr ---\n{exc.stderr}\n"
            )
        raise
    token = _parse_join_token(gen.stdout)
    log(f"cluster: join token ({len(token)} chars) acquired")

    # --- Nodes 2 & 3: join + serve -----------------------------------------
    for i in (1, 2):
        cfg = CLUSTER_NODES[i]
        log(
            f"cluster: joining node{i + 1} via 127.0.0.1:{CLUSTER_NODES[0]['api']} "
            f"(advertise=127.0.0.1, api={cfg['api']})"
        )
        subprocess.run(
            [
                str(zlayer_bin),
                "--data-dir", str(data_dirs[i]),
                "node", "join",
                f"127.0.0.1:{CLUSTER_NODES[0]['api']}",
                "--token", token,
                "--advertise-addr", "127.0.0.1",
                "--api-port", str(cfg["api"]),
                "--raft-port", str(cfg["raft"]),
                "--overlay-port", str(cfg["overlay"]),
            ],
            check=True, cwd=REPO_ROOT,
        )
        log(f"cluster: starting node{i + 1} serve on 127.0.0.1:{cfg['api']}")
        p = _spawn_node_serve(zlayer_bin, data_dirs[i], cfg["api"])
        procs.append(p)
        if not _wait_http_ok(
            f"http://127.0.0.1:{cfg['api']}/health/ready", 30,
        ):
            die(f"node{i + 1} never came up on 127.0.0.1:{cfg['api']}")

    return procs, data_dirs, api_ports


def _wait_for_ready_cluster(
    leader_api_port: int, expected_nodes: int, timeout_s: int,
) -> list[dict]:
    """Poll the leader until `expected_nodes` show status='ready' and >=1
    has role='leader'. Returns the final node list."""
    deadline = time.monotonic() + timeout_s
    last_nodes: list[dict] = []
    last_err: Optional[str] = None
    while time.monotonic() < deadline:
        try:
            nodes = _fetch_cluster_nodes(leader_api_port)
        except (urllib.error.URLError, urllib.error.HTTPError,
                ConnectionError, TimeoutError, OSError, json.JSONDecodeError,
                RuntimeError) as e:
            last_err = repr(e)
            time.sleep(2)
            continue
        last_nodes = nodes
        ready = [n for n in nodes if str(n.get("status", "")).lower() == "ready"]
        leaders = [n for n in nodes if str(n.get("role", "")).lower() == "leader"]
        if len(ready) >= expected_nodes and len(leaders) >= 1:
            return nodes
        time.sleep(2)
    detail = (
        f"; last_err={last_err}" if last_err else
        f"; last_nodes={last_nodes!r}"
    )
    raise RuntimeError(
        f"cluster never reached {expected_nodes} ready nodes with a leader "
        f"within {timeout_s}s{detail}"
    )


def _cleanup_cluster(
    procs: list[subprocess.Popen], root_dir: Path,
) -> None:
    for i, p in enumerate(procs):
        _kill_pg(p, f"cluster-node{i + 1}")
    if os.environ.get("KEEP_E2E_ARTIFACTS") == "1":
        log(f"cluster: KEEP_E2E_ARTIFACTS=1 → leaving {root_dir} in place")
        return
    shutil.rmtree(root_dir, ignore_errors=True)


def run_cluster_3node(args: argparse.Namespace) -> int:
    """Stand up a 3-node cluster, assert it forms with a leader + 3 ready."""
    if not args.no_build:
        build_phase(throwaway=args.throwaway, with_manager=False)

    zlayer_bin = resolve_zlayer_bin(throwaway=args.throwaway)
    log(f"cluster_3node: using zlayer binary {zlayer_bin}")

    root_dir = THROWAWAY_ROOT / "cluster_3node"
    # Fresh slate: this suite is destructive to its target directory.
    shutil.rmtree(root_dir, ignore_errors=True)
    root_dir.mkdir(parents=True, exist_ok=True)

    procs: list[subprocess.Popen] = []
    try:
        procs, _data_dirs, api_ports = _bootstrap_3node_cluster(
            zlayer_bin, root_dir,
        )
        log("cluster_3node: waiting for cluster to converge")
        nodes = _wait_for_ready_cluster(
            api_ports[0], expected_nodes=3,
            timeout_s=CLUSTER_NODES_READY_TIMEOUT_S,
        )
        for n in nodes:
            log(
                f"cluster_3node: node id={n.get('id', '?')} "
                f"role={n.get('role', '?')} status={n.get('status', '?')}"
            )
        log("cluster_3node: PASS — 3 ready nodes with leader elected")
        return 0
    except Exception as e:  # noqa: BLE001
        print(f"cluster_3node: FAIL — {e!r}", file=sys.stderr, flush=True)
        return 1
    finally:
        _cleanup_cluster(procs, root_dir)


def run_cluster_failover(args: argparse.Namespace) -> int:
    """Stand up 3 nodes, kill a non-leader, assert dead→ready transition.

    Reschedule of dead-node replicas requires deploying a service first; that
    assertion belongs in a separate suite. This suite scope is heartbeat
    transition (ready -> dead -> ready) per README §385-387.
    """
    if not args.no_build:
        build_phase(throwaway=args.throwaway, with_manager=False)

    zlayer_bin = resolve_zlayer_bin(throwaway=args.throwaway)
    log(f"cluster_failover: using zlayer binary {zlayer_bin}")

    root_dir = THROWAWAY_ROOT / "cluster_failover"
    shutil.rmtree(root_dir, ignore_errors=True)
    root_dir.mkdir(parents=True, exist_ok=True)

    procs: list[subprocess.Popen] = []
    try:
        procs, data_dirs, api_ports = _bootstrap_3node_cluster(
            zlayer_bin, root_dir,
        )
        leader_api = api_ports[0]
        log("cluster_failover: waiting for cluster to converge")
        nodes = _wait_for_ready_cluster(
            leader_api, expected_nodes=3,
            timeout_s=CLUSTER_NODES_READY_TIMEOUT_S,
        )

        # Pick a worker (non-leader). The leader's API endpoint contains
        # node1's port; we use that as a fallback identifier when the
        # `role` field is missing on a worker payload.
        worker_idx: Optional[int] = None
        for n in nodes:
            role = str(n.get("role", "")).lower()
            api_endpoint = str(n.get("api_endpoint") or n.get("address") or "")
            if role and role != "leader":
                # Map endpoint port → our index.
                for i, port in enumerate(api_ports):
                    if str(port) in api_endpoint:
                        worker_idx = i
                        break
                if worker_idx is None:
                    # Fall back to "anything but node 1".
                    worker_idx = 1
                break
        if worker_idx is None:
            # Cluster reported no explicit leader/worker split; pick node 2.
            worker_idx = 1
        log(
            f"cluster_failover: selected worker = node{worker_idx + 1} "
            f"(api={api_ports[worker_idx]})"
        )

        # 1. Kill the worker.
        _kill_pg(procs[worker_idx], f"cluster-node{worker_idx + 1} (kill)")

        # 2. Poll for `dead`.
        log(
            f"cluster_failover: waiting up to {CLUSTER_NODE_DEAD_TIMEOUT_S}s "
            f"for node{worker_idx + 1} → dead"
        )
        deadline = time.monotonic() + CLUSTER_NODE_DEAD_TIMEOUT_S
        saw_dead = False
        while time.monotonic() < deadline:
            try:
                current = _fetch_cluster_nodes(leader_api)
            except Exception:  # noqa: BLE001
                time.sleep(2)
                continue
            for n in current:
                api_endpoint = str(
                    n.get("api_endpoint") or n.get("address") or ""
                )
                if str(api_ports[worker_idx]) not in api_endpoint:
                    continue
                if str(n.get("status", "")).lower() == "dead":
                    saw_dead = True
                    break
            if saw_dead:
                break
            time.sleep(2)
        if not saw_dead:
            raise RuntimeError(
                f"node{worker_idx + 1} never transitioned to 'dead' within "
                f"{CLUSTER_NODE_DEAD_TIMEOUT_S}s"
            )
        log(f"cluster_failover: node{worker_idx + 1} → dead (OK)")

        # 3. Restart the killed worker. It already joined → just `serve`.
        log(f"cluster_failover: restarting node{worker_idx + 1}")
        procs[worker_idx] = _spawn_node_serve(
            zlayer_bin, data_dirs[worker_idx], api_ports[worker_idx],
        )
        if not _wait_http_ok(
            f"http://127.0.0.1:{api_ports[worker_idx]}/health/ready", 30,
        ):
            raise RuntimeError(
                f"restarted node{worker_idx + 1} never came up on "
                f"127.0.0.1:{api_ports[worker_idx]}"
            )

        # 4. Poll for `ready` again.
        log(
            f"cluster_failover: waiting up to {CLUSTER_NODE_RECOVER_TIMEOUT_S}s "
            f"for node{worker_idx + 1} → ready"
        )
        deadline = time.monotonic() + CLUSTER_NODE_RECOVER_TIMEOUT_S
        recovered = False
        while time.monotonic() < deadline:
            try:
                current = _fetch_cluster_nodes(leader_api)
            except Exception:  # noqa: BLE001
                time.sleep(2)
                continue
            for n in current:
                api_endpoint = str(
                    n.get("api_endpoint") or n.get("address") or ""
                )
                if str(api_ports[worker_idx]) not in api_endpoint:
                    continue
                if str(n.get("status", "")).lower() == "ready":
                    recovered = True
                    break
            if recovered:
                break
            time.sleep(2)
        if not recovered:
            raise RuntimeError(
                f"node{worker_idx + 1} never recovered to 'ready' within "
                f"{CLUSTER_NODE_RECOVER_TIMEOUT_S}s"
            )
        log(
            f"cluster_failover: PASS — node{worker_idx + 1} ready→dead→ready"
        )
        return 0
    except Exception as e:  # noqa: BLE001
        print(f"cluster_failover: FAIL — {e!r}", file=sys.stderr, flush=True)
        return 1
    finally:
        _cleanup_cluster(procs, root_dir)


# ---------------------------------------------------------------------------
# Cluster scaling / upgrade suites
# ---------------------------------------------------------------------------
#
# These suites build on `_bootstrap_3node_cluster` and exercise the
# deploy → scale and rolling-upgrade paths. The spec YAMLs live under
# `crates/zlayer-manager/tests/e2e/cluster-specs/`.

CLUSTER_SPECS_DIR = (
    REPO_ROOT / "crates" / "zlayer-manager" / "tests" / "e2e" / "cluster-specs"
)
CLUSTER_APP_DEPLOYMENT = "e2e-cluster-app"

# `ps --containers` field names vary across CLI versions; probe in order.
_IMAGE_FIELDS = ("image", "image_name", "image_ref")
_NODE_FIELDS = ("node_id", "node", "host_node", "placed_on", "host")


def _container_status(entry: dict) -> str:
    """Best-effort extraction of the running-state for a `ps` entry."""
    for field in ("status", "state", "phase"):
        val = entry.get(field)
        if val:
            return str(val)
    return ""


def _container_image(entry: dict) -> str:
    for field in _IMAGE_FIELDS:
        val = entry.get(field)
        if val:
            return str(val)
    return ""


def _container_node(entry: dict) -> str:
    for field in _NODE_FIELDS:
        val = entry.get(field)
        if val:
            return str(val)
    return ""


def _count_running_containers(
    zlayer_bin: Path, data_dir: Path, deployment: str,
    expected_count: int, timeout_s: int = 120,
) -> list[dict]:
    """Poll `zlayer ps --containers` until `expected_count` are running.

    Returns the list of running entries. Raises RuntimeError on timeout.
    Transient `CalledProcessError`s (cluster still reconciling) are
    swallowed and retried.
    """
    deadline = time.monotonic() + timeout_s
    last_entries: list[dict] = []
    last_err: Optional[str] = None
    while time.monotonic() < deadline:
        try:
            result = subprocess.run(
                [
                    str(zlayer_bin),
                    "--data-dir", str(data_dir),
                    "ps",
                    "--deployment", deployment,
                    "--containers",
                    "--format", "json",
                ],
                capture_output=True, text=True, check=True, cwd=REPO_ROOT,
            )
        except subprocess.CalledProcessError as exc:
            last_err = (
                f"exit={exc.returncode} stderr={(exc.stderr or '').strip()!r}"
            )
            if exc.stderr:
                sys.stderr.write(
                    f"--- ps --containers stderr (retrying) ---\n{exc.stderr}\n"
                )
            time.sleep(2)
            continue

        raw = (result.stdout or "").strip()
        if not raw:
            last_err = "empty stdout"
            time.sleep(2)
            continue
        try:
            parsed = json.loads(raw)
        except json.JSONDecodeError as e:
            last_err = f"json decode error: {e!r}; stdout={raw!r}"
            time.sleep(2)
            continue

        if isinstance(parsed, dict) and "containers" in parsed:
            entries = list(parsed["containers"])
        elif isinstance(parsed, list):
            entries = parsed
        else:
            last_err = f"unexpected ps payload shape: {parsed!r}"
            time.sleep(2)
            continue

        running = [
            e for e in entries
            if "running" in _container_status(e).lower()
        ]
        last_entries = running
        if len(running) == expected_count:
            return running
        time.sleep(2)

    detail = (
        f"; last_err={last_err}" if last_err else
        f"; last_running={last_entries!r}"
    )
    raise RuntimeError(
        f"deployment {deployment} never reached {expected_count} running "
        f"containers within {timeout_s}s{detail}"
    )


def _wait_image_transition(
    zlayer_bin: Path, data_dir: Path, deployment: str,
    expected_image: str, expected_count: int, timeout_s: int = 180,
) -> list[dict]:
    """Poll until all `expected_count` running containers report
    `image == expected_image`. Returns the converged list."""
    deadline = time.monotonic() + timeout_s
    last_state: list[dict] = []
    last_err: Optional[str] = None
    while time.monotonic() < deadline:
        try:
            result = subprocess.run(
                [
                    str(zlayer_bin),
                    "--data-dir", str(data_dir),
                    "ps",
                    "--deployment", deployment,
                    "--containers",
                    "--format", "json",
                ],
                capture_output=True, text=True, check=True, cwd=REPO_ROOT,
            )
        except subprocess.CalledProcessError as exc:
            last_err = (
                f"exit={exc.returncode} stderr={(exc.stderr or '').strip()!r}"
            )
            if exc.stderr:
                sys.stderr.write(
                    f"--- ps --containers stderr (retrying) ---\n{exc.stderr}\n"
                )
            time.sleep(2)
            continue

        raw = (result.stdout or "").strip()
        if not raw:
            last_err = "empty stdout"
            time.sleep(2)
            continue
        try:
            parsed = json.loads(raw)
        except json.JSONDecodeError as e:
            last_err = f"json decode error: {e!r}; stdout={raw!r}"
            time.sleep(2)
            continue

        if isinstance(parsed, dict) and "containers" in parsed:
            entries = list(parsed["containers"])
        elif isinstance(parsed, list):
            entries = parsed
        else:
            last_err = f"unexpected ps payload shape: {parsed!r}"
            time.sleep(2)
            continue

        running = [
            e for e in entries
            if "running" in _container_status(e).lower()
        ]
        last_state = running
        if len(running) == expected_count and all(
            _container_image(e) == expected_image for e in running
        ):
            return running
        time.sleep(2)

    detail = (
        f"; last_err={last_err}" if last_err else
        f"; last_running={[(_container_image(e), _container_status(e)) for e in last_state]!r}"
    )
    raise RuntimeError(
        f"deployment {deployment} never converged to {expected_count} "
        f"containers on image {expected_image} within {timeout_s}s{detail}"
    )


def run_cluster_scaling(args: argparse.Namespace) -> int:
    """3-node cluster, deploy nginx-v1 at 1→3→1 replicas via spec swaps."""
    if not args.no_build:
        build_phase(throwaway=args.throwaway, with_manager=False)

    zlayer_bin = resolve_zlayer_bin(throwaway=args.throwaway)
    log(f"cluster_scaling: using zlayer binary {zlayer_bin}")

    root_dir = THROWAWAY_ROOT / "cluster_scaling"
    shutil.rmtree(root_dir, ignore_errors=True)
    root_dir.mkdir(parents=True, exist_ok=True)

    procs: list[subprocess.Popen] = []
    try:
        procs, data_dirs, api_ports = _bootstrap_3node_cluster(
            zlayer_bin, root_dir,
        )
        log("cluster_scaling: waiting for cluster to converge")
        _wait_for_ready_cluster(
            api_ports[0], expected_nodes=3,
            timeout_s=CLUSTER_NODES_READY_TIMEOUT_S,
        )

        leader_data_dir = data_dirs[0]
        spec_1r = CLUSTER_SPECS_DIR / "nginx-v1-1r.yaml"
        spec_3r = CLUSTER_SPECS_DIR / "nginx-v1-3r.yaml"

        # --- 1 replica ----------------------------------------------------
        log(f"cluster_scaling: deploying {spec_1r.name} (1 replica)")
        try:
            subprocess.run(
                [
                    str(zlayer_bin),
                    "--data-dir", str(leader_data_dir),
                    "deploy", str(spec_1r),
                ],
                check=True, cwd=REPO_ROOT,
            )
        except subprocess.CalledProcessError as exc:
            if exc.stderr:
                sys.stderr.write(f"--- deploy stderr ---\n{exc.stderr}\n")
            raise
        log("cluster_scaling: waiting for 1 running container")
        _count_running_containers(
            zlayer_bin, leader_data_dir,
            CLUSTER_APP_DEPLOYMENT, expected_count=1,
        )
        log("cluster_scaling: 1 replica running (OK)")

        # --- 3 replicas ---------------------------------------------------
        log(f"cluster_scaling: deploying {spec_3r.name} (3 replicas)")
        try:
            subprocess.run(
                [
                    str(zlayer_bin),
                    "--data-dir", str(leader_data_dir),
                    "deploy", str(spec_3r),
                ],
                check=True, cwd=REPO_ROOT,
            )
        except subprocess.CalledProcessError as exc:
            if exc.stderr:
                sys.stderr.write(f"--- deploy stderr ---\n{exc.stderr}\n")
            raise
        log("cluster_scaling: waiting for 3 running containers")
        running = _count_running_containers(
            zlayer_bin, leader_data_dir,
            CLUSTER_APP_DEPLOYMENT, expected_count=3,
        )

        node_ids = [_container_node(e) for e in running]
        distinct = {nid for nid in node_ids if nid}
        log(f"cluster_scaling: replica node distribution = {node_ids!r}")
        if len(distinct) < 2:
            raise RuntimeError(
                f"expected replicas spread across at least 2 nodes; "
                f"got distinct={distinct!r} from {node_ids!r}"
            )
        log(
            f"cluster_scaling: replicas spread across {len(distinct)} nodes "
            f"(OK)"
        )

        # --- back to 1 replica -------------------------------------------
        log(f"cluster_scaling: redeploying {spec_1r.name} (scale-down 3→1)")
        try:
            subprocess.run(
                [
                    str(zlayer_bin),
                    "--data-dir", str(leader_data_dir),
                    "deploy", str(spec_1r),
                ],
                check=True, cwd=REPO_ROOT,
            )
        except subprocess.CalledProcessError as exc:
            if exc.stderr:
                sys.stderr.write(f"--- deploy stderr ---\n{exc.stderr}\n")
            raise
        log("cluster_scaling: waiting for 1 running container (scale-down)")
        _count_running_containers(
            zlayer_bin, leader_data_dir,
            CLUSTER_APP_DEPLOYMENT, expected_count=1,
        )

        log("cluster_scaling: PASS — 1 → 3 → 1 replicas across cluster")
        return 0
    except Exception as e:  # noqa: BLE001
        print(f"cluster_scaling: FAIL — {e!r}", file=sys.stderr, flush=True)
        return 1
    finally:
        _cleanup_cluster(procs, root_dir)


def run_cluster_upgrade(args: argparse.Namespace) -> int:
    """3-node cluster, rolling image upgrade v1.28 → v1.29."""
    if not args.no_build:
        build_phase(throwaway=args.throwaway, with_manager=False)

    zlayer_bin = resolve_zlayer_bin(throwaway=args.throwaway)
    log(f"cluster_upgrade: using zlayer binary {zlayer_bin}")

    root_dir = THROWAWAY_ROOT / "cluster_upgrade"
    shutil.rmtree(root_dir, ignore_errors=True)
    root_dir.mkdir(parents=True, exist_ok=True)

    procs: list[subprocess.Popen] = []
    try:
        procs, data_dirs, api_ports = _bootstrap_3node_cluster(
            zlayer_bin, root_dir,
        )
        log("cluster_upgrade: waiting for cluster to converge")
        _wait_for_ready_cluster(
            api_ports[0], expected_nodes=3,
            timeout_s=CLUSTER_NODES_READY_TIMEOUT_S,
        )

        leader_data_dir = data_dirs[0]
        spec_v1 = CLUSTER_SPECS_DIR / "nginx-v1-3r.yaml"
        spec_v2 = CLUSTER_SPECS_DIR / "nginx-v2-3r.yaml"

        # --- v1 (nginx:1.28-alpine) --------------------------------------
        log(f"cluster_upgrade: deploying {spec_v1.name} (v1, 3 replicas)")
        try:
            subprocess.run(
                [
                    str(zlayer_bin),
                    "--data-dir", str(leader_data_dir),
                    "deploy", str(spec_v1),
                ],
                check=True, cwd=REPO_ROOT,
            )
        except subprocess.CalledProcessError as exc:
            if exc.stderr:
                sys.stderr.write(f"--- deploy stderr ---\n{exc.stderr}\n")
            raise
        log("cluster_upgrade: waiting for 3 running v1 containers")
        v1_running = _count_running_containers(
            zlayer_bin, leader_data_dir,
            CLUSTER_APP_DEPLOYMENT, expected_count=3,
        )
        initial_images = [_container_image(e) for e in v1_running]
        log(f"cluster_upgrade: initial images = {initial_images!r}")

        # --- v2 (nginx:1.29-alpine), poll for image-field transition -----
        log(f"cluster_upgrade: deploying {spec_v2.name} (v2 rolling upgrade)")
        try:
            subprocess.run(
                [
                    str(zlayer_bin),
                    "--data-dir", str(leader_data_dir),
                    "deploy", str(spec_v2),
                ],
                check=True, cwd=REPO_ROOT,
            )
        except subprocess.CalledProcessError as exc:
            if exc.stderr:
                sys.stderr.write(f"--- deploy stderr ---\n{exc.stderr}\n")
            raise

        expected_image = "nginx:1.29-alpine"
        log(
            f"cluster_upgrade: waiting for 3 containers on {expected_image}"
        )
        _wait_image_transition(
            zlayer_bin, leader_data_dir,
            CLUSTER_APP_DEPLOYMENT, expected_image=expected_image,
            expected_count=3,
        )

        log(
            "cluster_upgrade: PASS — 3 replicas migrated v1.28 → v1.29"
        )
        return 0
    except Exception as e:  # noqa: BLE001
        print(f"cluster_upgrade: FAIL — {e!r}", file=sys.stderr, flush=True)
        return 1
    finally:
        _cleanup_cluster(procs, root_dir)


# ---------------------------------------------------------------------------
# Cluster node-upgrade orchestrator suite
# ---------------------------------------------------------------------------
#
# Exercises `POST /api/v1/cluster/upgrade` (added in Commit D) end-to-end
# against a real 3-node loopback cluster. We post a deliberately
# nonexistent target version so every follower's `zlayer self-update`
# shell-out fails, which lets us assert three independent pieces of
# behaviour without ever actually restarting a daemon:
#
#   1. A follower-targeted POST returns 421 + `X-Leader-Addr`.
#   2. The leader orchestrator walks every follower and records one
#      `errors[]` entry per follower (no `upgraded[]`, no daemon flap).
#   3. After the failed rollout, all 3 nodes are still `status=ready`.


def run_cluster_node_upgrade(args: argparse.Namespace) -> int:
    """`POST /api/v1/cluster/upgrade` with a bad version: assert 421
    leader-redirect, orchestrator follower-walk + error-recording, and
    that the cluster survives the failed rollout."""
    if not args.no_build:
        build_phase(throwaway=args.throwaway, with_manager=False)

    zlayer_bin = resolve_zlayer_bin(throwaway=args.throwaway)
    log(f"cluster_node_upgrade: using zlayer binary {zlayer_bin}")

    root_dir = THROWAWAY_ROOT / "cluster_node_upgrade"
    shutil.rmtree(root_dir, ignore_errors=True)
    root_dir.mkdir(parents=True, exist_ok=True)

    procs: list[subprocess.Popen] = []
    try:
        procs, _data_dirs, api_ports = _bootstrap_3node_cluster(
            zlayer_bin, root_dir,
        )
        log("cluster_node_upgrade: waiting for cluster to converge")
        nodes = _wait_for_ready_cluster(
            api_ports[0], expected_nodes=3,
            timeout_s=CLUSTER_NODES_READY_TIMEOUT_S,
        )

        # Identify leader. The bootstrap path makes node1 the leader, but
        # if the payload exposes an explicit `role=leader` we honour it.
        leader_port: Optional[int] = None
        for n in nodes:
            role = str(n.get("role", "")).lower()
            if role == "leader":
                api_endpoint = str(
                    n.get("api_endpoint") or n.get("address") or ""
                )
                for port in api_ports:
                    if str(port) in api_endpoint:
                        leader_port = port
                        break
                break
        if leader_port is None:
            # Fall back to the bootstrap leader (node1).
            leader_port = api_ports[0]
        log(f"cluster_node_upgrade: leader = 127.0.0.1:{leader_port}")

        # Pick a non-leader follower for the 421 redirect test. Use the
        # last node so we never accidentally collide with the leader.
        follower_port = api_ports[-1]
        if follower_port == leader_port:
            # Defensive fallback: pick any port != leader.
            follower_port = next(
                p for p in api_ports if p != leader_port
            )
        log(
            f"cluster_node_upgrade: follower for 421 test = "
            f"127.0.0.1:{follower_port}"
        )

        def _post_cluster_upgrade(
            api_port: int, body: dict, timeout_s: int = 60,
        ) -> tuple[int, dict, dict]:
            """POST /api/v1/cluster/upgrade. Returns (status, body_json, headers)."""
            url = f"http://127.0.0.1:{api_port}/api/v1/cluster/upgrade"
            req = urllib.request.Request(
                url, method="POST",
                headers={"Content-Type": "application/json"},
                data=json.dumps(body).encode("utf-8"),
            )
            bearer = os.environ.get("ZLAYER_E2E_CLUSTER_TOKEN")
            if bearer:
                req.add_header("Authorization", f"Bearer {bearer}")
            try:
                with urllib.request.urlopen(req, timeout=timeout_s) as resp:
                    return (
                        resp.status,
                        json.loads(resp.read().decode("utf-8")),
                        dict(resp.headers),
                    )
            except urllib.error.HTTPError as e:
                body_bytes = e.read() if hasattr(e, 'read') else b""
                body_text = body_bytes.decode("utf-8", errors="replace")
                try:
                    body_json = json.loads(body_text) if body_text else {}
                except json.JSONDecodeError:
                    body_json = {"raw": body_text}
                return (
                    e.code, body_json, dict(e.headers) if e.headers else {},
                )

        # --- Test 1: 421 leader-redirect ---------------------------------
        log(
            "cluster_node_upgrade: Test 1 — POST against non-leader, "
            "expect 421 + X-Leader-Addr"
        )
        bad_body = {
            "version": "v0.0.0-test-nonexistent",
            "cooldown_secs": 1,
            "strict": False,
        }
        status1, body1, headers1 = _post_cluster_upgrade(
            follower_port, bad_body, timeout_s=30,
        )
        if status1 != 421:
            raise RuntimeError(
                f"expected 421 from follower POST, got {status1}; "
                f"body={body1!r} headers={headers1!r}"
            )
        leader_addr_hdr = (
            headers1.get("X-Leader-Addr")
            or headers1.get("x-leader-addr")
        )
        if not leader_addr_hdr:
            raise RuntimeError(
                "follower 421 response missing X-Leader-Addr header; "
                f"status={status1} body={body1!r} headers={headers1!r}"
            )
        if str(body1.get("error", "")) != "not_leader":
            raise RuntimeError(
                "follower 421 response body missing `error: not_leader`; "
                f"status={status1} body={body1!r} headers={headers1!r}"
            )
        log(
            f"cluster_node_upgrade: 421 redirect OK "
            f"(leader_addr={leader_addr_hdr})"
        )

        # --- Test 2: orchestrator walks followers and records errors -----
        # Every follower's self-update will fail (nonexistent version → no
        # such GitHub release), so the leader's per-follower drop-deadline
        # (30s) elapses on each. With 2 followers + a 1s cooldown that's
        # comfortably under 2 minutes, but we budget a generous 10 minutes
        # of urllib timeout to keep the suite robust under load.
        log(
            "cluster_node_upgrade: Test 2 — POST against leader with bad "
            "version, expect orchestrator to record 2 errors (one per "
            "follower) and 0 upgrades"
        )
        status2, body2, _headers2 = _post_cluster_upgrade(
            leader_port, bad_body, timeout_s=600,
        )
        if status2 != 200:
            raise RuntimeError(
                f"expected 200 from leader POST, got {status2}; "
                f"body={body2!r}"
            )
        upgraded = body2.get("upgraded") or []
        errors_list = body2.get("errors") or []
        skipped = body2.get("skipped") or []
        if len(upgraded) != 0:
            raise RuntimeError(
                f"expected upgraded=[], got {upgraded!r}; "
                f"full body={body2!r}"
            )
        if len(errors_list) != 2:
            raise RuntimeError(
                f"expected 2 errors (one per follower), got "
                f"{len(errors_list)}; errors={errors_list!r} "
                f"upgraded={upgraded!r} skipped={skipped!r}"
            )
        log(
            f"cluster_node_upgrade: orchestrator-walk OK "
            f"(upgraded={upgraded!r} skipped={skipped!r} "
            f"errors={[e.get('node_id') for e in errors_list]!r})"
        )

        # --- Test 3: cluster survives the failed upgrade -----------------
        log(
            "cluster_node_upgrade: Test 3 — re-fetch cluster nodes, "
            "expect all 3 still ready"
        )
        after = _fetch_cluster_nodes(leader_port)
        ready_after = [
            n for n in after
            if str(n.get("status", "")).lower() == "ready"
        ]
        if len(ready_after) != 3:
            raise RuntimeError(
                f"expected 3 ready nodes after failed upgrade, got "
                f"{len(ready_after)}; nodes={after!r}"
            )
        log(
            "cluster_node_upgrade: PASS — 421-redirect, orchestrator-walk, "
            "error-recording, cluster-survival all verified"
        )
        return 0
    except Exception as e:  # noqa: BLE001
        print(
            f"cluster_node_upgrade: FAIL — {e!r}",
            file=sys.stderr, flush=True,
        )
        return 1
    finally:
        _cleanup_cluster(procs, root_dir)


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Hermetic runner for the zlayer-manager Intellitester suite."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Environment overrides:\n"
            "  ZLAYER_E2E_THROWAWAY=1    same as --throwaway\n"
            "  ZLAYER_E2E_SUDO_DAEMON=1  same as --sudo-daemon\n"
            "  ZLAYER_E2E_API_PORT       throwaway API port (default 13669)\n"
            "  ZLAYER_E2E_MANAGER_PORT   manager bind port (default 16677)\n"
            "  ZLAYER_E2E_WG_PORT        throwaway WG port (default 51421)\n"
            "  ZLAYER_E2E_DNS_PORT       throwaway DNS port (default 15354)\n"
            "  ZLAYER_SOCKET             override host daemon socket path\n"
            "  KEEP_E2E_ARTIFACTS=1      retain cluster_* throwaway dirs\n"
            "  ZLAYER_E2E_CLUSTER_TOKEN  bearer token for /api/v1/cluster/nodes\n"
            "\n"
            "Suites:\n"
            "  manager_auth      (default) login + nav + stale-session\n"
            "  cluster_3node     boot a local 3-node cluster + assert quorum\n"
            "  cluster_failover  3-node cluster, kill worker, assert recovery\n"
            "  cluster_scaling   3-node cluster, deploy 1→3→1 replicas\n"
            "  cluster_upgrade   3-node cluster, image v1→v2 rolling upgrade\n"
            "  cluster_node_upgrade  POST /api/v1/cluster/upgrade with bad version,\n"
            "                        assert 421 redirect + orchestrator-walk + cluster survival\n"
        ),
    )
    parser.add_argument(
        "suite", nargs="?",
        choices=[
            "manager_auth",
            "cluster_3node",
            "cluster_failover",
            "cluster_scaling",
            "cluster_upgrade",
            "cluster_node_upgrade",
        ],
        default=None,
        help=(
            "Which suite to run. Defaults to `manager_auth`. May also be "
            "supplied via `--suite`."
        ),
    )
    parser.add_argument(
        "--suite", dest="suite_flag",
        choices=[
            "manager_auth",
            "cluster_3node",
            "cluster_failover",
            "cluster_scaling",
            "cluster_upgrade",
            "cluster_node_upgrade",
        ],
        default=None,
        help="Alternative to the positional suite argument.",
    )
    parser.add_argument(
        "--throwaway", action="store_true",
        default=bool(os.environ.get("ZLAYER_E2E_THROWAWAY")),
        help=(
            "Spin up an isolated daemon under target/zlayer-e2e/<id>/ "
            "instead of connecting to the host daemon."
        ),
    )
    parser.add_argument(
        "--sudo-daemon", dest="sudo_daemon", action="store_true",
        default=bool(os.environ.get("ZLAYER_E2E_SUDO_DAEMON")),
        help=(
            "Wrap the throwaway daemon subprocess in `sudo -E env PATH=... "
            "HOME=...`. Only the daemon is elevated; the harness, manager, "
            "pnpm, and target/ files stay user-owned. No-op when EUID is "
            "already 0."
        ),
    )
    parser.add_argument(
        "--only", metavar="YAML",
        help=(
            "Run a single intellitester test file (relative to tests/e2e/). "
            "Skips the stale-session regression sequence."
        ),
    )
    parser.add_argument(
        "--no-build", dest="no_build", action="store_true",
        help="Skip cargo build + cargo leptos build.",
    )
    ns = parser.parse_args()
    # Resolve positional/flag suite name → ns.suite (positional wins).
    if ns.suite is None:
        ns.suite = ns.suite_flag if ns.suite_flag is not None else "manager_auth"
    return ns


def main() -> int:
    args = parse_args()

    # Cluster suites do not use the manager fixture pipeline below; dispatch
    # them up-front and return their exit code directly.
    if args.suite == "cluster_3node":
        return run_cluster_3node(args)
    if args.suite == "cluster_failover":
        return run_cluster_failover(args)
    if args.suite == "cluster_scaling":
        return run_cluster_scaling(args)
    if args.suite == "cluster_upgrade":
        return run_cluster_upgrade(args)
    if args.suite == "cluster_node_upgrade":
        return run_cluster_node_upgrade(args)

    # Default suite (`manager_auth`) falls through into the manager harness.

    daemon_proc: Optional[subprocess.Popen] = None
    manager_proc: Optional[subprocess.Popen] = None
    throwaway_dir: Optional[Path] = None
    cli_data_dir: Optional[Path] = None
    fixture_user_id: Optional[str] = None
    zlayer_bin: Optional[Path] = None
    daemon_sudo: bool = False

    # SIGTERM via a handler so the `finally:` block always runs.
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))

    try:
        if not args.no_build:
            build_phase(throwaway=args.throwaway)

        zlayer_bin = resolve_zlayer_bin(throwaway=args.throwaway)
        log(f"Using zlayer binary: {zlayer_bin}")

        if args.throwaway:
            suite_id = secrets.token_hex(4)
            throwaway_dir = THROWAWAY_ROOT / suite_id
            throwaway_dir.mkdir(parents=True, exist_ok=True)
            cli_data_dir = throwaway_dir
            daemon_sudo = args.sudo_daemon
            daemon_proc, socket_path = start_throwaway_daemon(
                throwaway_dir, zlayer_bin, sudo=daemon_sudo,
            )
            log(f"Throwaway daemon socket: {socket_path}")
        else:
            socket = resolve_host_socket()
            if not socket:
                die(
                    "no host daemon socket found.\n"
                    "  Probed:\n"
                    "    $ZLAYER_SOCKET (if set)\n"
                    "    /var/run/zlayer.sock\n"
                    "    $HOME/.zlayer/run/zlayer.sock\n"
                    "  Either start the daemon, or rerun with --throwaway."
                )
                raise RuntimeError("unreachable")
            socket_path = socket
            log(f"Host daemon socket: {socket_path}")

        suite_token = secrets.token_hex(4)
        fixture_email = f"e2e-{suite_token}@test.local"
        fixture_password = secrets.token_hex(16)
        fixture_display = "E2E Admin"

        fixture_user_id = create_fixture_user(
            zlayer_bin, cli_data_dir,
            fixture_email, fixture_password, fixture_display,
        )

        manager_proc = start_manager(socket_path)

        # Intellitester's collectMissingEnvVars scans every `${VAR}`
        # reference and prompts when one is missing from process.env —
        # even ones declared in the yaml's own `variables:` block.
        # Export the creds so the prompt never fires.
        intelli_env = {
            "ADMIN_EMAIL": fixture_email,
            "ADMIN_PASSWORD": fixture_password,
            "ADMIN_DISPLAY": fixture_display,
        }

        if args.only:
            target = E2E_DIR / args.only
            if not target.exists():
                die(f"--only target not found: {target}")
            log(f"Running single test: {args.only}")
            run_intellitester(target, env_extras=intelli_env)
            log(f"Single test passed: {args.only}")
            return 0

        log("Running login.test.yaml")
        run_intellitester(E2E_DIR / "login.test.yaml", env_extras=intelli_env)

        log("Running nav-smoke.test.yaml")
        run_intellitester(
            E2E_DIR / "nav-smoke.test.yaml", env_extras=intelli_env,
        )

        # --- Stale-session regression -----------------------------------
        #
        # 1. Setup logs in and saves storage state to disk.
        # 2. Harness deletes the fixture user — cookies now reference a
        #    non-existent user (the exact stale-session condition).
        # 3. Verify loads cookies, hits the manager, asserts that
        #    /api/manager/manager_me clears both cookies, browser jar
        #    is empty after, and we redirect to /login.
        state_file = E2E_DIR / ".stale-session.state.json"
        state_file.unlink(missing_ok=True)

        log("Stale-session: login + saveStorageState")
        run_intellitester(
            E2E_DIR / "stale-session-setup.test.yaml",
            env_extras=intelli_env,
        )

        log(f"Stale-session: deleting fixture user via CLI ({fixture_user_id})")
        delete_fixture_user(zlayer_bin, cli_data_dir, fixture_user_id)
        # Mark deleted so the `finally:` doesn't double-delete.
        fixture_user_id = None

        log("Stale-session: verify cookies cleared on next request")
        run_intellitester(
            E2E_DIR / "stale-session-verify.test.yaml",
            extra_args=["--storage-state", str(state_file)],
            env_extras=intelli_env,
        )

        log("All e2e checks passed.")
        return 0

    finally:
        # Order matters: manager → fixture user → daemon → tmpdir → state.
        if manager_proc is not None:
            _kill_pg(manager_proc, "manager")
        if fixture_user_id is not None and zlayer_bin is not None:
            delete_fixture_user(zlayer_bin, cli_data_dir, fixture_user_id)
        if daemon_proc is not None:
            _kill_pg(daemon_proc, "daemon", sudo=daemon_sudo)
        if throwaway_dir is not None:
            # If --sudo-daemon was used, the daemon may have written
            # root-owned files inside throwaway_dir. shutil.rmtree will
            # silently fail on those; retry under sudo. target/ is
            # gitignored so a residual dir is harmless either way.
            shutil.rmtree(throwaway_dir, ignore_errors=True)
            if throwaway_dir.exists() and daemon_sudo and os.geteuid() != 0:
                with suppress(subprocess.SubprocessError):
                    subprocess.run(
                        ["sudo", "rm", "-rf", str(throwaway_dir)],
                        check=False,
                    )
        with suppress(FileNotFoundError):
            (E2E_DIR / ".stale-session.state.json").unlink()


if __name__ == "__main__":
    sys.exit(main())
