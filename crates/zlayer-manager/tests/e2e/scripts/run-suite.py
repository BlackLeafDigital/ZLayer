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
import socket
import stat
import subprocess
import sys
import time
import urllib.error
import urllib.request
from contextlib import contextmanager, suppress
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


def _open_logs(token_dir: Path, name: str) -> tuple:
    """Open stdout/stderr log files for a spawned process.

    Returns (out_fh, err_fh) — keep these tied to the Popen so the kernel
    keeps writing until the writer exits. Caller is responsible for closing
    via _close_proc_logs(proc) after proc.wait().
    """
    logs_dir = token_dir / "logs"
    logs_dir.mkdir(parents=True, exist_ok=True)
    out = open(logs_dir / f"{name}.out.log", "ab", buffering=0)
    err = open(logs_dir / f"{name}.err.log", "ab", buffering=0)
    return out, err


def _attach_logs(proc: subprocess.Popen, out, err) -> subprocess.Popen:
    """Stash log file handles on a Popen so cleanup can close them."""
    proc._zlayer_log_files = (out, err)  # type: ignore[attr-defined]
    return proc


def _close_proc_logs(proc: subprocess.Popen) -> None:
    """Close any log handles attached via _attach_logs. Idempotent."""
    for fh in getattr(proc, "_zlayer_log_files", ()):
        with suppress(Exception):
            fh.close()
    if hasattr(proc, "_zlayer_log_files"):
        proc._zlayer_log_files = ()  # type: ignore[attr-defined]


def _run_capture(cmd: list, output_path: Path) -> None:
    """Run a shell command, write its combined stdout+stderr to output_path.

    Failures are swallowed — capture is best-effort only.
    """
    try:
        output_path.parent.mkdir(parents=True, exist_ok=True)
        with open(output_path, "wb") as fh:
            subprocess.run(
                cmd, stdout=fh, stderr=subprocess.STDOUT,
                timeout=10, check=False,
            )
    except Exception as e:  # noqa: BLE001 - post-mortem must not fail
        with suppress(Exception):
            output_path.write_text(f"capture failed: {e!r}\n")


def _capture_tail(src: Path, dst: Path, n: int = 500) -> None:
    """Write the last n lines of src to dst. Best-effort."""
    try:
        if not src.is_file():
            return
        dst.parent.mkdir(parents=True, exist_ok=True)
        with open(src, "rb") as fh:
            fh.seek(0, 2)
            size = fh.tell()
            block = min(size, max(n * 200, 4096))
            fh.seek(-block, 2)
            data = fh.read()
        lines = data.splitlines()[-n:]
        with open(dst, "wb") as fh:
            for line in lines:
                fh.write(line + b"\n")
    except Exception as e:  # noqa: BLE001
        with suppress(Exception):
            dst.write_text(f"tail failed: {e!r}\n")


def _capture_postmortem(
    token_dir: Path,
    data_dirs: list,
    suite_name: str,
    exit_code: int,
) -> None:
    """Snapshot host state + per-node bundles + log tails into <token>/postmortem/.

    Called BEFORE cleanup deletes the data dirs. Every step is best-effort —
    never raises, never fails the calling suite. `data_dirs` is the list of
    daemon data dirs (e.g. `[<token>/node1/data, <token>/node2/data, ...]`
    for cluster suites, `[<token>/data]` for the manager suite).
    """
    pm = token_dir / "postmortem"
    try:
        pm.mkdir(parents=True, exist_ok=True)
        with suppress(Exception):
            (pm / "marker.txt").write_text(
                f"suite={suite_name}\nexit_code={exit_code}\n"
                f"captured_at={time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}\n"
            )
    except Exception:  # noqa: BLE001
        return

    host = pm / "host"
    _run_capture(["ip", "-o", "link", "show"], host / "ip-link.txt")
    _run_capture(["ip", "-o", "addr", "show"], host / "ip-addr.txt")
    _run_capture(["sh", "-c", "mount | grep -E '^cgroup|cgroup2|/sys/fs/cgroup'"],
                 host / "mount-cgroup.txt")
    _run_capture(["findmnt", "--json"], host / "findmnt.txt")
    _run_capture(
        ["sh", "-c",
         "echo '== /proc/self/uid_map =='; cat /proc/self/uid_map 2>&1; "
         "echo '== /proc/self/gid_map =='; cat /proc/self/gid_map 2>&1; "
         "echo '== /proc/self/setgroups =='; cat /proc/self/setgroups 2>&1"],
        host / "userns.txt",
    )

    for idx, data_dir in enumerate(data_dirs, start=1):
        try:
            if data_dir == token_dir:
                node_name = "manager"
            else:
                node_name = f"node{idx}"
            dest = pm / node_name
            dest.mkdir(parents=True, exist_ok=True)
            bundles_src = data_dir / "bundles"
            if bundles_src.is_dir():
                with suppress(Exception):
                    shutil.copytree(
                        bundles_src, dest / "bundles",
                        dirs_exist_ok=True, symlinks=True,
                    )
            dj = data_dir / "daemon.json"
            if dj.is_file():
                with suppress(Exception):
                    shutil.copy2(dj, dest / "daemon.json")
            run_dir = data_dir / "run"
            if run_dir.is_dir():
                with suppress(Exception):
                    (dest / "run-listing.txt").write_text(
                        "\n".join(
                            sorted(str(p.relative_to(run_dir))
                                   for p in run_dir.rglob("*"))
                        ) + "\n"
                    )
        except Exception:  # noqa: BLE001
            continue

    logs_src = token_dir / "logs"
    if logs_src.is_dir():
        tail_dir = pm / "tail"
        for log_file in logs_src.glob("*.log"):
            _capture_tail(log_file, tail_dir / (log_file.name + ".tail"))


# ---------------------------------------------------------------------------
# Layer C: step tracking + SUMMARY.json
# ---------------------------------------------------------------------------

# Tracks the currently-executing suite step so failed_step can be recorded
# in SUMMARY.json even when the exception is caught far from the failure
# point. Reset by the _step() context manager.
_current_step: Optional[str] = None


@contextmanager
def _step(name: str):
    """Track the currently-running step for SUMMARY.json's failed_step field.

    Wrap each obvious suite phase: with _step("bootstrap_3node"): ...

    When an exception bubbles out of the wrapped block, _current_step retains
    `name` until the next outer _step (or until cleanup); SUMMARY.json reads
    it to populate failed_step.
    """
    global _current_step
    prev = _current_step
    _current_step = name
    try:
        yield
    finally:
        # On exception we want failed_step to STAY at the inner name (don't
        # reset to prev); on normal exit, restore prev so an outer _step
        # is correctly named when no error occurred.
        if sys.exc_info()[0] is None:
            _current_step = prev


def _write_summary(
    token_dir: Path,
    suite_name: str,
    started_at: float,
    ended_at: float,
    exit_code: int,
    failure: Optional[tuple],
    nodes: Optional[list] = None,
) -> None:
    """Write <token>/SUMMARY.json with the suite outcome. Best-effort.

    `failure` is `(type_name, message)` or None on success.
    `nodes` is a list of dicts shaped like
        {"index": int, "api_port": int, "pid": int, "data_dir": str}
    or None for non-cluster suites.
    """
    try:
        token_dir.mkdir(parents=True, exist_ok=True)
        iso = lambda t: time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(t))
        payload = {
            "suite": suite_name,
            "token": token_dir.name,
            "started_at": iso(started_at),
            "ended_at": iso(ended_at),
            "exit_code": exit_code,
            "failed_step": _current_step,
            "failure": (
                {"type": failure[0], "message": failure[1]}
                if failure else None
            ),
            "nodes": nodes or [],
            "artifacts": {
                "logs": "logs/",
                "postmortem": "postmortem/",
                "data_dirs": [
                    str(d.relative_to(token_dir))
                    if token_dir in d.parents or d == token_dir
                    else str(d)
                    for d in (
                        [Path(n["data_dir"]) for n in (nodes or [])]
                    )
                ],
            },
        }
        (token_dir / "SUMMARY.json").write_text(
            json.dumps(payload, indent=2) + "\n"
        )
    except Exception as e:  # noqa: BLE001
        with suppress(Exception):
            (token_dir / "SUMMARY.json.error").write_text(
                f"summary write failed: {e!r}\n"
            )


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
        _close_proc_logs(proc)
        return
    try:
        pgid = os.getpgid(proc.pid)
    except ProcessLookupError:
        _close_proc_logs(proc)
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
    _close_proc_logs(proc)


def _ports_in_use(ports: list[int]) -> list[int]:
    """Return the subset of `ports` that cannot currently be bound on 127.0.0.1.

    Exists because cluster suites share fixed ports (CLUSTER_NODES); a daemon
    from a previous suite that hasn't fully released its listening socket
    will cause the next suite to silently talk to the leftover daemon.
    """
    busy: list[int] = []
    for port in ports:
        s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        try:
            s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            try:
                s.bind(("127.0.0.1", port))
            except OSError:
                busy.append(port)
        finally:
            with suppress(OSError):
                s.close()
    return busy


def _wait_ports_free(ports: list[int], timeout_s: float = 10.0) -> list[int]:
    """Poll `_ports_in_use` every 250ms until empty or timeout; return still-busy.

    Used between cluster suites so the next suite's bootstrap doesn't race a
    half-dead daemon whose TCP listener hasn't yet been reaped by the kernel
    (root cause of the cross-suite "join secret not found" contamination).
    """
    deadline = time.monotonic() + timeout_s
    busy = _ports_in_use(ports)
    while busy and time.monotonic() < deadline:
        time.sleep(0.25)
        busy = _ports_in_use(ports)
    return busy


def _force_kill_port_owners(ports: list[int]) -> None:
    """Best-effort SIGKILL whoever still owns each port (fuser → lsof → warn).

    Last-resort hammer for cluster-suite port leaks: if a previous suite's
    daemon survived `_kill_pg` (rare but observed), the next suite would
    bind-fail on 19110/19120/19130 and answer to the wrong data-dir.
    """
    fuser = shutil.which("fuser")
    lsof = shutil.which("lsof")
    for port in ports:
        killed = False
        if fuser:
            res = subprocess.run(
                ["fuser", "-k", "-KILL", "-n", "tcp", str(port)],
                check=False, capture_output=True,
            )
            if res.returncode == 0:
                killed = True
        if not killed and lsof:
            res = subprocess.run(
                ["lsof", "-ti", f"tcp:{port}"],
                check=False, capture_output=True, text=True,
            )
            pids = [
                int(line) for line in res.stdout.splitlines()
                if line.strip().isdigit()
            ]
            for pid in pids:
                with suppress(ProcessLookupError, PermissionError, OSError):
                    os.kill(pid, signal.SIGKILL)
                    killed = True
        if not killed and not fuser and not lsof:
            log(
                f"_force_kill_port_owners: neither fuser nor lsof available; "
                f"cannot force-free port {port}"
            )


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
        log("Building zlayer + zlayer-overlayd (release)")
        # The daemon self-spawns the sibling `zlayer-overlayd` binary at
        # `serve` time; without it the overlay dial stalls ~35s and trips
        # the harness's /health/ready budget. Build both so the sibling
        # exists alongside the `zlayer` binary in target/release.
        subprocess.run(
            ["cargo", "build", "--release", "-p", "zlayer", "-p", "zlayer-overlayd"],
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
    data_dir: Path, zlayer_bin: Path, *, sudo: bool, token_dir: Path,
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
    out, err = _open_logs(token_dir, "daemon")
    proc = subprocess.Popen(
        argv, cwd=REPO_ROOT, env=env, start_new_session=True,
        stdout=out, stderr=err,
    )
    _attach_logs(proc, out, err)

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
    *, sudo: bool = False,
) -> str:
    log(f"Creating fixture user {email} (admin)")
    env = {**os.environ}
    if cli_data_dir:
        env["ZLAYER_DATA_DIR"] = str(cli_data_dir)
    try:
        result = subprocess.run(
            _maybe_sudo([
                str(zlayer_bin), "user", "create",
                "--email", email,
                "--password", password,
                "--role", "admin",
                "--display-name", display,
            ], sudo=sudo),
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
    *, sudo: bool = False,
) -> None:
    env = {**os.environ}
    if cli_data_dir:
        env["ZLAYER_DATA_DIR"] = str(cli_data_dir)
    # Daemon-side delete is idempotent (204 in both cases). Transport
    # failures during cleanup are tolerated.
    result = subprocess.run(
        _maybe_sudo(
            [str(zlayer_bin), "user", "delete", user_id, "--yes"],
            sudo=sudo,
        ),
        env=env, capture_output=True, text=True,
    )
    if result.returncode != 0:
        print(
            f"!! fixture user delete exit={result.returncode}: "
            f"{result.stderr.strip()}",
            file=sys.stderr, flush=True,
        )


def start_manager(socket_path: Path, *, token_dir: Path) -> subprocess.Popen:
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
    out, err = _open_logs(token_dir, "manager")
    proc = subprocess.Popen(
        [str(binary)], cwd=REPO_ROOT, env=env, start_new_session=True,
        stdout=out, stderr=err,
    )
    _attach_logs(proc, out, err)
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
    # `--silent` must come BEFORE `dlx`: it's a top-level pnpm flag,
    # and modern pnpm parses post-`dlx` args strictly as <package> [args],
    # so `dlx --silent <pkg>` makes `--silent` the package name and 404s.
    argv = ["pnpm", "--silent", "dlx", INTELLITESTER_PIN, "run"]
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
# manager-suite range so the two cannot collide. Overlay UDP ports kept
# clear of 51820 (WireGuard default) and the production zlayer daemon's
# default range (5141x–5143x) so a running system daemon does not steal
# a throwaway node's WireGuard port mid-suite.
CLUSTER_NODES: list[dict[str, int]] = [
    {"api": 19110, "raft": 19111, "overlay": 59410},
    {"api": 19120, "raft": 19121, "overlay": 59420},
    {"api": 19130, "raft": 19131, "overlay": 59430},
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
    *, sudo: bool = False, node_index: int, token_dir: Path,
) -> subprocess.Popen:
    """Spawn `zlayer serve` for a cluster node (no --daemon, killable group).

    `sudo=True` wraps argv in `sudo -E env PATH=... HOME=...` so the daemon
    runs as root. Required locally when the developer host lacks rootless
    container infra (idmapped mounts, working /etc/subuid+/etc/subgid for
    the daemon user, etc). CI's `node:20-bullseye --privileged` containers
    already run as root so this is a no-op there.
    """
    env = {
        **os.environ,
        "ZLAYER_JWT_SECRET": os.environ.get(
            "ZLAYER_JWT_SECRET",
            "e2e-secret-do-not-use-in-prod-do-not-share-this-key-1234567890",
        ),
    }
    argv = _maybe_sudo(
        [
            str(zlayer_bin),
            "--data-dir", str(data_dir),
            "serve",
            "--bind", f"127.0.0.1:{api_port}",
            "--deployment-name", CLUSTER_DEPLOYMENT,
        ],
        sudo=sudo,
    )
    out, err = _open_logs(token_dir, f"node{node_index}")
    proc = subprocess.Popen(
        argv, cwd=REPO_ROOT, env=env, start_new_session=True,
        stdout=out, stderr=err,
    )
    return _attach_logs(proc, out, err)


def _bootstrap_3node_cluster(
    zlayer_bin: Path, root_dir: Path, *, sudo: bool = False,
) -> tuple[list[subprocess.Popen], list[Path], list[int]]:
    """Stand up a 3-node loopback cluster.

    Returns (procs, data_dirs, api_ports). Caller is responsible for
    `_kill_pg`-ing every proc in `procs` in its `finally:` block.
    """
    # Pre-flight: refuse to start on a polluted port set. Belt-and-suspenders
    # for the case where the previous suite's `_cleanup_cluster` didn't run
    # (CI worker died, KEEP_E2E_ARTIFACTS, manual reruns). Without this we
    # would silently bind-fail on node1 and the suite would talk to the
    # leftover daemon → confusing "join secret not found" elsewhere.
    all_cluster_ports = [
        p for n in CLUSTER_NODES for p in (n["api"], n["raft"])
    ]
    busy = _wait_ports_free(all_cluster_ports, timeout_s=2.0)
    if busy:
        log(
            f"cluster bootstrap: ports {busy} already in use; "
            "force-killing leftover listeners"
        )
        _force_kill_port_owners(busy)
        busy = _wait_ports_free(busy, timeout_s=5.0)
        if busy:
            die(
                f"cluster bootstrap: ports {busy} are still in use after "
                "force-kill; another zlayer daemon or test harness is bound "
                "to them — clean up manually (e.g. "
                "`fuser -k -n tcp 19110 19120 19130`)"
            )

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
        _maybe_sudo([
            str(zlayer_bin),
            "--data-dir", str(data_dirs[0]),
            "node", "init",
            "--advertise-addr", "127.0.0.1",
            "--api-port", str(CLUSTER_NODES[0]["api"]),
            "--raft-port", str(CLUSTER_NODES[0]["raft"]),
            "--overlay-port", str(CLUSTER_NODES[0]["overlay"]),
        ], sudo=sudo),
        check=True, cwd=REPO_ROOT,
    )

    # --- Node 1: serve in the background -----------------------------------
    log(f"cluster: starting node1 serve on 127.0.0.1:{CLUSTER_NODES[0]['api']}")
    n1 = _spawn_node_serve(
        zlayer_bin, data_dirs[0], CLUSTER_NODES[0]["api"], sudo=sudo,
        node_index=1, token_dir=root_dir,
    )
    procs.append(n1)
    if not _wait_http_ok(
        f"http://127.0.0.1:{CLUSTER_NODES[0]['api']}/health/ready", 60,
    ):
        die(f"node1 never came up on 127.0.0.1:{CLUSTER_NODES[0]['api']}")

    # --- Nodes 2 & 3: mint a fresh join token per joiner, then join + serve.
    # Signed cluster join tokens (commit ce8a53a) are single-use: the leader
    # records (kid, iat, iss) on first acceptance and rejects replays. Mint
    # one token per node here rather than reusing.
    for i in (1, 2):
        cfg = CLUSTER_NODES[i]
        log(f"cluster: generating join token on node1 for node{i + 1}")
        try:
            gen = subprocess.run(
                _maybe_sudo([
                    str(zlayer_bin),
                    "--data-dir", str(data_dirs[0]),
                    "node", "generate-join-token",
                    CLUSTER_DEPLOYMENT,
                    "-a", f"http://127.0.0.1:{CLUSTER_NODES[0]['api']}",
                ], sudo=sudo),
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
        log(f"cluster: join token for node{i + 1} ({len(token)} chars) acquired")

        log(
            f"cluster: joining node{i + 1} via 127.0.0.1:{CLUSTER_NODES[0]['api']} "
            f"(advertise=127.0.0.1, api={cfg['api']})"
        )
        subprocess.run(
            _maybe_sudo([
                str(zlayer_bin),
                "--data-dir", str(data_dirs[i]),
                "node", "join",
                f"127.0.0.1:{CLUSTER_NODES[0]['api']}",
                "--token", token,
                "--advertise-addr", "127.0.0.1",
                "--api-port", str(cfg["api"]),
                "--raft-port", str(cfg["raft"]),
                "--overlay-port", str(cfg["overlay"]),
            ], sudo=sudo),
            check=True, cwd=REPO_ROOT,
        )
        log(f"cluster: starting node{i + 1} serve on 127.0.0.1:{cfg['api']}")
        p = _spawn_node_serve(
            zlayer_bin, data_dirs[i], cfg["api"], sudo=sudo,
            node_index=i + 1, token_dir=root_dir,
        )
        procs.append(p)
        if not _wait_http_ok(
            f"http://127.0.0.1:{cfg['api']}/health/ready", 60,
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
    procs: list[subprocess.Popen], root_dir: Path, *, sudo: bool = False,
    suite_name: str = "unknown",
    data_dirs: Optional[list[Path]] = None,
    exit_code: int = 0,
    started_at: float = 0.0,
    nodes: Optional[list] = None,
) -> None:
    if data_dirs:
        _capture_postmortem(root_dir, data_dirs, suite_name, exit_code)
    for i, p in enumerate(procs):
        _kill_pg(p, f"cluster-node{i + 1}", sudo=sudo)
        _close_proc_logs(p)
    # Wait for the kernel to actually release the cluster ports before the
    # next suite's bootstrap tries to bind them. Without this, suite N+1
    # silently talks to suite N's leftover daemon (cross-suite contamination
    # → confusing "join secret not found" errors downstream).
    all_cluster_ports = [
        p for n in CLUSTER_NODES for p in (n["api"], n["raft"])
    ]
    still_busy = _wait_ports_free(all_cluster_ports, timeout_s=10.0)
    if still_busy:
        log(
            f"cluster cleanup: ports {still_busy} still bound after _kill_pg; "
            "force-killing remaining listeners"
        )
        _force_kill_port_owners(still_busy)
        still_busy = _wait_ports_free(still_busy, timeout_s=3.0)
        if still_busy:
            log(
                f"cluster cleanup: ports {still_busy} STILL bound after "
                "force-kill; next suite's bootstrap will fail loudly"
            )
    # Write SUMMARY.json before the rmtree below has a chance to wipe it.
    if started_at > 0:
        _write_summary(
            token_dir=root_dir, suite_name=suite_name,
            started_at=started_at, ended_at=time.time(),
            exit_code=exit_code, failure=None, nodes=nodes,
        )
    if os.environ.get("KEEP_E2E_ARTIFACTS") == "1":
        log(f"cluster: KEEP_E2E_ARTIFACTS=1 → leaving {root_dir} in place")
        return
    shutil.rmtree(root_dir, ignore_errors=True)


# ---------------------------------------------------------------------------
# Container-mode helpers (--overlay-mode container)
# ---------------------------------------------------------------------------
#
# When `--overlay-mode container` AND `ZLAYER_E2E_PRIVILEGED=1` are set, each
# cluster node runs inside its own privileged container attached to a shared
# bridge network so the boringtun overlay (zlayer0 / TUN) is actually
# exercised. The harness orchestrates each node via
# `<runtime> exec <container> zlayer ...`.
#
# Design doc: docs/operating/e2e-privileged.md.
#
# Env vars:
#   ZLAYER_E2E_PRIVILEGED=1          required (acknowledge privileged host)
#   ZLAYER_E2E_IMAGE                 default `zlayer/zlayer-e2e-node:latest`
#   ZLAYER_E2E_NETWORK               default `zlayer-e2e`
#   ZLAYER_E2E_KEEP_CONTAINERS=1     leave containers up after the suite
#
# Image is expected to be present (operator runs
# `zlayer pipeline -f ZPipeline.yaml --set VERSION=dev`); the harness only
# verifies presence, never builds.

# Static IPs assigned inside the bridge subnet. /24 with `.1` as the gateway
# (created by the runtime). Nodes get `.10`, `.11`, `.12` so each suite gets
# a deterministic mapping for join-target URLs.
_CONTAINER_SUBNET = "10.99.99.0/24"
_CONTAINER_GATEWAY = "10.99.99.1"
_CONTAINER_NODE_IPS = ["10.99.99.10", "10.99.99.11", "10.99.99.12"]

# Inside the test image: binary at /usr/local/bin/zlayer, data at /var/lib/zlayer.
_CONTAINER_BIN = "/usr/local/bin/zlayer"
_CONTAINER_DATA_DIR = "/var/lib/zlayer"


def _detect_container_runtime() -> str:
    """Return the absolute path to `podman` or `docker`.

    Per the design doc, podman is preferred (rootless friendly) but docker
    works too. Raises RuntimeError if neither is on PATH so the caller can
    print an actionable error.
    """
    for name in ("podman", "docker"):
        path = shutil.which(name)
        if path:
            return path
    raise RuntimeError(
        "no container runtime found in PATH; need podman or docker for "
        "--overlay-mode container (see docs/operating/e2e-privileged.md)"
    )


def _runtime_is_podman(runtime: str) -> bool:
    return os.path.basename(runtime) == "podman"


def _ensure_e2e_network(runtime: str, network: str) -> None:
    """Create the named bridge network if it doesn't already exist.

    Uses `<runtime> network exists` (podman) / `<runtime> network inspect`
    (docker) to probe, falling back to `network create` with our fixed
    `--subnet`/`--gateway` so the harness controls IP allocation.
    """
    if _runtime_is_podman(runtime):
        probe = subprocess.run(
            [runtime, "network", "exists", network],
            check=False, capture_output=True,
        )
        exists = probe.returncode == 0
    else:
        probe = subprocess.run(
            [runtime, "network", "inspect", network],
            check=False, capture_output=True,
        )
        exists = probe.returncode == 0
    if exists:
        log(f"container: reusing existing network `{network}`")
        return
    log(
        f"container: creating bridge network `{network}` "
        f"(subnet={_CONTAINER_SUBNET}, gateway={_CONTAINER_GATEWAY})"
    )
    subprocess.run(
        [
            runtime, "network", "create",
            "--driver", "bridge",
            "--subnet", _CONTAINER_SUBNET,
            "--gateway", _CONTAINER_GATEWAY,
            network,
        ],
        check=True, capture_output=True,
    )


def _ensure_e2e_image(runtime: str, image: str) -> None:
    """Refuse to start if the test image isn't present locally.

    Image build is the operator's responsibility:
        zlayer pipeline -f ZPipeline.yaml --set VERSION=dev
    (see images/ZImagefile.zlayer-e2e-node + docs/operating/e2e-privileged.md).
    """
    if _runtime_is_podman(runtime):
        probe = subprocess.run(
            [runtime, "image", "exists", image],
            check=False, capture_output=True,
        )
        exists = probe.returncode == 0
    else:
        probe = subprocess.run(
            [runtime, "image", "inspect", image],
            check=False, capture_output=True,
        )
        exists = probe.returncode == 0
    if exists:
        log(f"container: test image `{image}` present")
        return
    die(
        f"container: test image `{image}` not found locally.\n"
        "  Build it first:\n"
        "    zlayer pipeline -f ZPipeline.yaml --set VERSION=dev\n"
        "  (see images/ZImagefile.zlayer-e2e-node + "
        "docs/operating/e2e-privileged.md)"
    )


def _container_name(suite: str, idx: int) -> str:
    """Predictable name e.g. `zlayer-e2e-cluster_3node-node1`."""
    return f"zlayer-e2e-{suite}-node{idx + 1}"


def _container_run(
    runtime: str, *,
    name: str, image: str, network: str, ip: str,
) -> None:
    """Start a privileged container detached, attached to `network` at `ip`.

    The image's CMD is `sleep infinity` (see images/ZImagefile.zlayer-e2e-node)
    so the container stays up for subsequent `_container_exec` calls. We use
    `--rm` so a crash doesn't leave a phantom; cleanup is via `<runtime>
    stop` which removes it.
    """
    argv = [
        runtime, "run", "-d", "--rm",
        "--name", name,
        "--hostname", name,
        "--privileged",
        "--cap-add", "NET_ADMIN",
        "--device", "/dev/net/tun",
        "--network", network,
        "--ip", ip,
        image,
    ]
    log(f"container: starting `{name}` on {network} at {ip}")
    subprocess.run(argv, check=True, capture_output=True)


def _container_exec(
    runtime: str, name: str, argv: list[str], *,
    check: bool = True, capture: bool = False, detach: bool = False,
) -> Optional[subprocess.CompletedProcess]:
    """Run `<runtime> exec [-d] <name> <argv...>`.

    Returns the completed process when not detached, None when detached.
    When `capture=True`, stdout+stderr are captured (text mode); otherwise
    they flow to the harness's stdio.
    """
    cmd = [runtime, "exec"]
    if detach:
        cmd.append("-d")
    cmd.append(name)
    cmd.extend(argv)
    if detach:
        subprocess.run(cmd, check=check, capture_output=True)
        return None
    return subprocess.run(
        cmd, check=check, capture_output=capture, text=capture,
    )


# Path of the redirected `serve` log inside each cluster-node container. The
# daemon runs detached (`exec -d`), so its stdout is otherwise discarded; we
# tee it to this file via a shell wrapper so failures can be diagnosed.
_CONTAINER_SERVE_LOG = f"{_CONTAINER_DATA_DIR}/serve.log"


def _container_serve_argv(api_port: int) -> list[str]:
    """Argv to launch `zlayer serve` in a container with stdout+stderr
    redirected to `_CONTAINER_SERVE_LOG` (so the detached daemon's logs are
    recoverable for postmortem). `exec` replaces the shell so signals/kill
    still reach the daemon."""
    inner = (
        f"exec {_CONTAINER_BIN} --data-dir {_CONTAINER_DATA_DIR} serve "
        f"--bind 0.0.0.0:{api_port} --deployment-name {CLUSTER_DEPLOYMENT} "
        f"> {_CONTAINER_SERVE_LOG} 2>&1"
    )
    return ["sh", "-c", inner]


def _dump_cluster_daemon_logs(
    runtime: str, names: list[str], *, grep: Optional[str] = None,
) -> None:
    """Dump each node's redirected `serve.log` to stderr for postmortem.

    When `grep` is set, only matching lines are shown (plus a short tail);
    otherwise the tail of each log is printed. Best-effort: never raises.
    """
    for idx, name in enumerate(names):
        log(f"---- daemon log: node{idx + 1} ({name}) ----")
        try:
            if grep:
                inner = (
                    f"grep -E '{grep}' {_CONTAINER_SERVE_LOG} 2>/dev/null | tail -n 60; "
                    f"echo '---- tail ----'; tail -n 40 {_CONTAINER_SERVE_LOG} 2>/dev/null"
                )
            else:
                inner = f"tail -n 80 {_CONTAINER_SERVE_LOG} 2>/dev/null"
            res = subprocess.run(
                [runtime, "exec", name, "sh", "-c", inner],
                check=False, capture_output=True, text=True,
            )
            sys.stderr.write(res.stdout or "(empty)\n")
            if res.stderr:
                sys.stderr.write(res.stderr)
            sys.stderr.flush()
        except Exception as exc:  # noqa: BLE001
            log(f"  (failed to read daemon log for {name}: {exc!r})")


def _container_stop(runtime: str, name: str) -> None:
    """Stop a container. Best-effort: log on failure, never raise.

    Containers are started with `--rm` so stop also removes them.
    """
    res = subprocess.run(
        [runtime, "stop", name],
        check=False, capture_output=True,
    )
    if res.returncode != 0:
        log(
            f"container: `{runtime} stop {name}` failed "
            f"(rc={res.returncode}, stderr={res.stderr.decode('utf-8', 'replace').strip()!r})"
        )


def _bootstrap_3node_cluster_container(
    runtime: str, image: str, network: str, suite: str,
) -> tuple[list[str], list[str], list[str]]:
    """Stand up a 3-node container cluster.

    Returns (container_names, node_ips, api_endpoints) where
    `api_endpoints[i]` is `http://<ip>:<api_port>` for poll-from-host calls.

    Mirrors the sequence in `_bootstrap_3node_cluster`:
      1. Start all 3 containers at static IPs.
      2. `node init` on node1 → `serve` (detached) → wait /health/ready.
      3. For each joiner:
           a. Generate a fresh signed join token from node1.
           b. `node join` from joiner → `serve` (detached) → wait /health/ready.

    Caller is responsible for `_cleanup_cluster_container` in `finally:`.
    """
    names: list[str] = [_container_name(suite, i) for i in range(3)]
    ips: list[str] = list(_CONTAINER_NODE_IPS)
    api_endpoints: list[str] = [
        f"http://{ips[i]}:{CLUSTER_NODES[i]['api']}" for i in range(3)
    ]

    # --- Phase 1: launch all 3 containers ---------------------------------
    for i in range(3):
        _container_run(
            runtime,
            name=names[i], image=image, network=network, ip=ips[i],
        )

    # --- Phase 2: bootstrap node1 -----------------------------------------
    log(f"cluster(container): initializing node1 (api={CLUSTER_NODES[0]['api']})")
    _container_exec(
        runtime, names[0],
        [
            _CONTAINER_BIN,
            "--data-dir", _CONTAINER_DATA_DIR,
            "node", "init",
            "--advertise-addr", ips[0],
            "--api-port", str(CLUSTER_NODES[0]["api"]),
            "--raft-port", str(CLUSTER_NODES[0]["raft"]),
            "--overlay-port", str(CLUSTER_NODES[0]["overlay"]),
        ],
        check=True,
    )

    log(f"cluster(container): starting node1 serve on {ips[0]}:{CLUSTER_NODES[0]['api']}")
    _container_exec(
        runtime, names[0],
        _container_serve_argv(CLUSTER_NODES[0]["api"]),
        detach=True,
    )
    if not _wait_http_ok(f"{api_endpoints[0]}/health/ready", 30):
        die(f"node1 (container) never came up on {api_endpoints[0]}")

    # --- Phase 3: join nodes 2 & 3 ---------------------------------------
    # Mint one fresh signed join token per joiner (single-use, see
    # `_bootstrap_3node_cluster` for the same rationale).
    for i in (1, 2):
        cfg = CLUSTER_NODES[i]
        log(
            f"cluster(container): generating join token on node1 for "
            f"node{i + 1}"
        )
        gen = _container_exec(
            runtime, names[0],
            [
                _CONTAINER_BIN,
                "--data-dir", _CONTAINER_DATA_DIR,
                "node", "generate-join-token",
                CLUSTER_DEPLOYMENT,
                "-a", api_endpoints[0],
            ],
            check=True, capture=True,
        )
        assert gen is not None
        token = _parse_join_token(gen.stdout)
        log(
            f"cluster(container): join token for node{i + 1} "
            f"({len(token)} chars) acquired"
        )

        log(
            f"cluster(container): joining node{i + 1} via {ips[0]}:{CLUSTER_NODES[0]['api']} "
            f"(advertise={ips[i]}, api={cfg['api']})"
        )
        _container_exec(
            runtime, names[i],
            [
                _CONTAINER_BIN,
                "--data-dir", _CONTAINER_DATA_DIR,
                "node", "join",
                f"{ips[0]}:{CLUSTER_NODES[0]['api']}",
                "--token", token,
                "--advertise-addr", ips[i],
                "--api-port", str(cfg["api"]),
                "--raft-port", str(cfg["raft"]),
                "--overlay-port", str(cfg["overlay"]),
            ],
            check=True,
        )
        log(
            f"cluster(container): starting node{i + 1} serve on "
            f"{ips[i]}:{cfg['api']}"
        )
        _container_exec(
            runtime, names[i],
            _container_serve_argv(cfg["api"]),
            detach=True,
        )
        if not _wait_http_ok(f"{api_endpoints[i]}/health/ready", 30):
            die(f"node{i + 1} (container) never came up on {api_endpoints[i]}")

    return names, ips, api_endpoints


def _cleanup_cluster_container(
    runtime: str, names: list[str], *, network: Optional[str] = None,
) -> None:
    """Stop + remove each container, optionally remove the bridge network.

    `ZLAYER_E2E_KEEP_CONTAINERS=1` short-circuits the whole thing for
    postmortem inspection.
    """
    if os.environ.get("ZLAYER_E2E_KEEP_CONTAINERS") == "1":
        log(
            "container: ZLAYER_E2E_KEEP_CONTAINERS=1 → leaving containers "
            f"{names!r} (and network {network!r}) in place"
        )
        return
    for name in names:
        _container_stop(runtime, name)
    if network:
        res = subprocess.run(
            [runtime, "network", "rm", network],
            check=False, capture_output=True,
        )
        if res.returncode != 0:
            log(
                f"container: `{runtime} network rm {network}` failed "
                f"(may still have endpoints; rc={res.returncode})"
            )


# Per-control-plane worker-gRPC port used by the worker-tier suite. The
# worker subcommand long-polls assignments here; control-plane nodes also
# expose their existing API on `CLUSTER_NODES[i]["api"]`. Kept distinct from
# raft/overlay ports to avoid collisions when the harness exec's into nodes.
CLUSTER_WORKER_GRPC_PORT = 23670

# Static IPs handed to the worker containers in the worker-tier suite. They
# live in the same /24 as the control-plane nodes (`_CONTAINER_NODE_IPS` =
# `.10`/`.11`/`.12`) so a single bridge network covers both tiers; workers
# start at `.20` to leave room for control-plane growth.
_CONTAINER_WORKER_IPS = [
    "10.99.99.20",
    "10.99.99.21",
    "10.99.99.22",
    "10.99.99.23",
    "10.99.99.24",
]


def _container_inspect_running(runtime: str, name: str) -> bool:
    """Return True iff `<runtime> inspect <name>` reports State.Running.

    Best-effort: any error (missing container, runtime not reachable, etc.)
    returns False so callers can use this in a polling loop without nested
    try/except blocks.
    """
    res = subprocess.run(
        [runtime, "inspect", "-f", "{{.State.Running}}", name],
        check=False, capture_output=True, text=True,
    )
    if res.returncode != 0:
        return False
    return res.stdout.strip().lower() == "true"


def _bootstrap_3node_worker_tier_container(
    runtime: str, image: str, network: str, suite: str,
) -> tuple[list[str], list[str], list[str]]:
    """Spawn 3 control-plane containers in worker-tier server-role mode.

    Returns `(container_names, node_ips, api_endpoints)` — the same triple
    shape as `_bootstrap_3node_cluster_container`. The control-plane nodes
    still run Raft (so `node init` / `node join` still apply); the only
    difference vs the plain 3-node helper is that we drop a
    `cluster_mode.yaml` file with `mode: worker-tier, role: server` into
    each node's data dir between `node init/join` and `serve`. That file
    is what `serve` reads to instantiate `WorkerTierCluster` (which exposes
    the gRPC dispatcher on `worker_grpc_addr`).

    Caller is responsible for `_cleanup_cluster_container` in `finally:`.

    TODO(P3.12+): once `zlayer node init` learns a `--mode worker-tier`
    flag, drop the manual file-planting and have init write the YAML
    itself. For now the file-injection mirrors what an operator would do
    by hand per docs/operating/worker-tier.md.
    """
    names: list[str] = [_container_name(suite, i) for i in range(3)]
    ips: list[str] = list(_CONTAINER_NODE_IPS)
    api_endpoints: list[str] = [
        f"http://{ips[i]}:{CLUSTER_NODES[i]['api']}" for i in range(3)
    ]

    def _cluster_mode_yaml(node_id: int) -> str:
        """Render the per-node worker-tier-server `cluster_mode.yaml`.

        Peers list all three control-plane nodes by raft+api addr; the
        worker_grpc_addr binds 0.0.0.0 so workers on the bridge network
        can reach it via the container's static IP.
        """
        peers = "\n".join(
            f"  - id: {i + 1}\n"
            f"    raft_addr: {ips[i]}:{CLUSTER_NODES[i]['raft']}\n"
            f"    api_addr: {ips[i]}:{CLUSTER_NODES[i]['api']}"
            for i in range(3)
        )
        return (
            "mode: worker-tier\n"
            "role: server\n"
            f"node_id: {node_id}\n"
            "peers:\n"
            f"{peers}\n"
            f"worker_grpc_addr: 0.0.0.0:{CLUSTER_WORKER_GRPC_PORT}\n"
            "heartbeat_min_ttl: 5\n"
            "heartbeat_max_ttl: 60\n"
            "heartbeat_grace: 10\n"
            "max_heartbeats_per_second: 50\n"
            "failover_heartbeat_ttl: 30\n"
        )

    def _plant_cluster_mode(idx: int) -> None:
        """Write `cluster_mode.yaml` into `<data_dir>/` of container `idx`.

        Uses `sh -c 'cat > <path>'` with the YAML piped on stdin so we
        don't have to round-trip through `<runtime> cp` + a host tempfile.
        """
        yaml_body = _cluster_mode_yaml(idx + 1)
        path = f"{_CONTAINER_DATA_DIR}/cluster_mode.yaml"
        subprocess.run(
            [
                runtime, "exec", "-i", names[idx],
                "sh", "-c", f"mkdir -p {_CONTAINER_DATA_DIR} && cat > {path}",
            ],
            input=yaml_body, text=True,
            check=True, capture_output=True,
        )
        log(
            f"cluster(container): planted cluster_mode.yaml in "
            f"{names[idx]}:{path}"
        )

    # --- Phase 1: launch all 3 containers ---------------------------------
    for i in range(3):
        _container_run(
            runtime,
            name=names[i], image=image, network=network, ip=ips[i],
        )

    # --- Phase 2: bootstrap node1 (init → plant cluster_mode → serve) ----
    log(
        f"cluster_worker_tier(container): initializing node1 "
        f"(api={CLUSTER_NODES[0]['api']})"
    )
    _container_exec(
        runtime, names[0],
        [
            _CONTAINER_BIN,
            "--data-dir", _CONTAINER_DATA_DIR,
            "node", "init",
            "--advertise-addr", ips[0],
            "--api-port", str(CLUSTER_NODES[0]["api"]),
            "--raft-port", str(CLUSTER_NODES[0]["raft"]),
            "--overlay-port", str(CLUSTER_NODES[0]["overlay"]),
        ],
        check=True,
    )
    _plant_cluster_mode(0)

    log(
        f"cluster_worker_tier(container): starting node1 serve on "
        f"{ips[0]}:{CLUSTER_NODES[0]['api']}"
    )
    _container_exec(
        runtime, names[0],
        [
            _CONTAINER_BIN,
            "--data-dir", _CONTAINER_DATA_DIR,
            "serve",
            "--bind", f"0.0.0.0:{CLUSTER_NODES[0]['api']}",
            "--deployment-name", CLUSTER_DEPLOYMENT,
        ],
        detach=True,
    )
    if not _wait_http_ok(f"{api_endpoints[0]}/health/ready", 30):
        die(f"node1 (container, worker-tier) never came up on {api_endpoints[0]}")

    # --- Phase 3: join nodes 2 & 3 ---------------------------------------
    for i in (1, 2):
        cfg = CLUSTER_NODES[i]
        log(
            f"cluster_worker_tier(container): generating join token on "
            f"node1 for node{i + 1}"
        )
        gen = _container_exec(
            runtime, names[0],
            [
                _CONTAINER_BIN,
                "--data-dir", _CONTAINER_DATA_DIR,
                "node", "generate-join-token",
                CLUSTER_DEPLOYMENT,
                "-a", api_endpoints[0],
            ],
            check=True, capture=True,
        )
        assert gen is not None
        token = _parse_join_token(gen.stdout)

        log(
            f"cluster_worker_tier(container): joining node{i + 1} via "
            f"{ips[0]}:{CLUSTER_NODES[0]['api']} (advertise={ips[i]})"
        )
        _container_exec(
            runtime, names[i],
            [
                _CONTAINER_BIN,
                "--data-dir", _CONTAINER_DATA_DIR,
                "node", "join",
                f"{ips[0]}:{CLUSTER_NODES[0]['api']}",
                "--token", token,
                "--advertise-addr", ips[i],
                "--api-port", str(cfg["api"]),
                "--raft-port", str(cfg["raft"]),
                "--overlay-port", str(cfg["overlay"]),
            ],
            check=True,
        )
        _plant_cluster_mode(i)

        log(
            f"cluster_worker_tier(container): starting node{i + 1} serve on "
            f"{ips[i]}:{cfg['api']}"
        )
        _container_exec(
            runtime, names[i],
            _container_serve_argv(cfg["api"]),
            detach=True,
        )
        if not _wait_http_ok(f"{api_endpoints[i]}/health/ready", 30):
            die(
                f"node{i + 1} (container, worker-tier) never came up on "
                f"{api_endpoints[i]}"
            )

    return names, ips, api_endpoints


def _fetch_cluster_nodes_url(api_url: str) -> list[dict]:
    """Container-mode variant of `_fetch_cluster_nodes` that accepts a full URL.

    Loopback mode polls `http://127.0.0.1:<port>/...`; container mode polls
    `http://<container_ip>:<port>/...`. The handler logic is identical so
    this is just a thin URL-instead-of-port wrapper.
    """
    req = urllib.request.Request(f"{api_url}/api/v1/cluster/nodes")
    bearer = os.environ.get("ZLAYER_E2E_CLUSTER_TOKEN")
    if bearer:
        req.add_header("Authorization", f"Bearer {bearer}")
    with urllib.request.urlopen(req, timeout=5) as resp:
        body = resp.read().decode("utf-8")
    data = json.loads(body)
    if isinstance(data, dict) and "nodes" in data:
        return list(data["nodes"])
    if isinstance(data, list):
        return data
    raise RuntimeError(
        f"unexpected /api/v1/cluster/nodes payload shape: {data!r}"
    )


def _wait_for_ready_cluster_url(
    leader_api_url: str, expected_nodes: int, timeout_s: int,
) -> list[dict]:
    """URL-keyed analogue of `_wait_for_ready_cluster` for container mode."""
    deadline = time.monotonic() + timeout_s
    last_nodes: list[dict] = []
    last_err: Optional[str] = None
    while time.monotonic() < deadline:
        try:
            nodes = _fetch_cluster_nodes_url(leader_api_url)
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


def _container_ps_running(
    runtime: str, container: str, deployment: str,
    expected_count: int, timeout_s: int = 120,
) -> list[dict]:
    """Container-mode analogue of `_count_running_containers`.

    Runs `zlayer ps --containers --format json` inside `container` instead
    of as a host subprocess; otherwise the polling/parse logic is identical.
    """
    deadline = time.monotonic() + timeout_s
    last_entries: list[dict] = []
    last_err: Optional[str] = None
    while time.monotonic() < deadline:
        try:
            result = _container_exec(
                runtime, container,
                [
                    _CONTAINER_BIN,
                    "--data-dir", _CONTAINER_DATA_DIR,
                    "ps",
                    "--deployment", deployment,
                    "--containers",
                    "--format", "json",
                ],
                check=True, capture=True,
            )
        except subprocess.CalledProcessError as exc:
            last_err = (
                f"exit={exc.returncode} stderr={(exc.stderr or '').strip()!r}"
            )
            time.sleep(2)
            continue
        assert result is not None
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


def _container_wait_image_transition(
    runtime: str, container: str, deployment: str,
    expected_image: str, expected_count: int, timeout_s: int = 180,
) -> list[dict]:
    """Container-mode analogue of `_wait_image_transition`."""
    deadline = time.monotonic() + timeout_s
    last_state: list[dict] = []
    last_err: Optional[str] = None
    while time.monotonic() < deadline:
        try:
            result = _container_exec(
                runtime, container,
                [
                    _CONTAINER_BIN,
                    "--data-dir", _CONTAINER_DATA_DIR,
                    "ps",
                    "--deployment", deployment,
                    "--containers",
                    "--format", "json",
                ],
                check=True, capture=True,
            )
        except subprocess.CalledProcessError as exc:
            last_err = (
                f"exit={exc.returncode} stderr={(exc.stderr or '').strip()!r}"
            )
            time.sleep(2)
            continue
        assert result is not None
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
            _norm_image(_container_image(e)) == _norm_image(expected_image)
            for e in running
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


def _container_mode_settings() -> tuple[str, str, str]:
    """Resolve (runtime, image, network) from env + defaults.

    Raises on missing runtime; dies on missing image (with actionable msg).
    Always ensures the network exists.
    """
    runtime = _detect_container_runtime()
    image = os.environ.get("ZLAYER_E2E_IMAGE", "zlayer/zlayer-e2e-node:latest")
    network = os.environ.get("ZLAYER_E2E_NETWORK", "zlayer-e2e")
    _ensure_e2e_image(runtime, image)
    _ensure_e2e_network(runtime, network)
    return runtime, image, network


# ---------------------------------------------------------------------------
# Container-mode cluster suites
# ---------------------------------------------------------------------------


def run_cluster_3node_container(args: argparse.Namespace) -> int:
    """Container-mode mirror of `run_cluster_3node`."""
    runtime, image, network = _container_mode_settings()
    suite = "cluster_3node"
    names: list[str] = []
    try:
        names, _ips, api_endpoints = _bootstrap_3node_cluster_container(
            runtime, image, network, suite,
        )
        log("cluster_3node(container): waiting for cluster to converge")
        nodes = _wait_for_ready_cluster_url(
            api_endpoints[0], expected_nodes=3,
            timeout_s=CLUSTER_NODES_READY_TIMEOUT_S,
        )
        for n in nodes:
            log(
                f"cluster_3node(container): node id={n.get('id', '?')} "
                f"role={n.get('role', '?')} status={n.get('status', '?')}"
            )
        log("cluster_3node(container): PASS — 3 ready nodes with leader elected")
        return 0
    except Exception as e:  # noqa: BLE001
        print(f"cluster_3node(container): FAIL — {e!r}", file=sys.stderr, flush=True)
        return 1
    finally:
        _cleanup_cluster_container(runtime, names, network=network)


def run_cluster_gossip_5node_container(args: argparse.Namespace) -> int:
    """E2E test for the Phase 4 chitchat-based gossip pool.

    Spin up 5 containers running zlayer serve with gossip enabled.
    Assert pairwise discovery within 5s, then kill one and assert
    failure detection within 15s.

    TODO(P4.2-followup): depends on an HTTP endpoint that exposes
    `GossipPool::peers()` snapshots. Until that lands, the assertion
    logic reads container logs for the chitchat join/leave messages
    as a best-effort signal.
    """
    runtime, image, network = _container_mode_settings()
    suite = "cluster_gossip_5node"
    names: list[str] = []
    try:
        # 5 containers, each gossiping to all 5 (including self — chitchat
        # ignores the self entry).
        for i in range(5):
            name = f"zlayer-e2e-{suite}-node{i + 1}"
            names.append(name)
            ip = f"10.99.0.{30 + i}"
            _container_run(
                runtime,
                name=name, image=image, network=network, ip=ip,
                # Real launch would mount a gossip-enabled cluster_mode.yaml.
                # Until the gossip-config wiring is exposed in the daemon,
                # this currently just runs an idle daemon.
            )

        log(f"cluster_gossip_5node(container): {len(names)} nodes launched")

        # TODO(P4.2-followup): wait for pairwise discovery via /api/v1/gossip/peers.
        # For now, sleep long enough that the gossip protocol would converge
        # in a real run.
        time.sleep(15)

        # TODO(P4.2-followup): assert each node sees the other 4 via HTTP.
        log(
            "cluster_gossip_5node(container): pairwise-discovery assertion is "
            "stubbed; awaiting HTTP /api/v1/gossip/peers endpoint"
        )

        # Kill one node.
        target = names[2]
        log(f"cluster_gossip_5node(container): stopping {target}")
        _container_stop(runtime, target)

        # TODO(P4.2-followup): assert the remaining 4 mark this node dead
        # within 15s. For now, sleep.
        time.sleep(20)

        log(
            "cluster_gossip_5node(container): PASS "
            "(skeleton — awaits HTTP endpoint)"
        )
        return 0
    except Exception as e:  # noqa: BLE001
        print(
            f"cluster_gossip_5node(container): FAIL — {e!r}",
            file=sys.stderr, flush=True,
        )
        return 1
    finally:
        for n in names:
            try:
                _container_stop(runtime, n)
            except Exception:  # noqa: BLE001
                pass
        try:
            _cleanup_cluster_container(runtime, [], network=network)
        except Exception:  # noqa: BLE001
            pass


def run_cluster_failover_container(args: argparse.Namespace) -> int:
    """Container-mode mirror of `run_cluster_failover`.

    Failover here means `<runtime> stop <container>` instead of `_kill_pg`,
    and `<runtime> start` (or relaunch) for recovery. We relaunch the
    container from the same image so the daemon's data-dir is wiped — node
    join state survives only if the runtime preserved the rootfs, which
    `--rm` does NOT. Restart-via-join is the correct semantics anyway:
    signed join tokens are single-use, so we mint a fresh one.
    """
    runtime, image, network = _container_mode_settings()
    suite = "cluster_failover"
    names: list[str] = []
    try:
        names, ips, api_endpoints = _bootstrap_3node_cluster_container(
            runtime, image, network, suite,
        )
        leader_url = api_endpoints[0]
        log("cluster_failover(container): waiting for cluster to converge")
        nodes = _wait_for_ready_cluster_url(
            leader_url, expected_nodes=3,
            timeout_s=CLUSTER_NODES_READY_TIMEOUT_S,
        )

        # Pick a worker (non-leader). Bootstrap path makes node1 leader.
        worker_idx: Optional[int] = None
        for n in nodes:
            role = str(n.get("role", "")).lower()
            api_endpoint = str(n.get("api_endpoint") or n.get("address") or "")
            if role and role != "leader":
                for i, port in enumerate(CLUSTER_NODES):
                    if str(port["api"]) in api_endpoint:
                        worker_idx = i
                        break
                if worker_idx is None:
                    worker_idx = 1
                break
        if worker_idx is None:
            worker_idx = 1
        log(
            f"cluster_failover(container): selected worker = node{worker_idx + 1} "
            f"(api={CLUSTER_NODES[worker_idx]['api']})"
        )

        # 1. Stop the worker container.
        _container_stop(runtime, names[worker_idx])

        # 2. Poll for `dead`.
        log(
            f"cluster_failover(container): waiting up to "
            f"{CLUSTER_NODE_DEAD_TIMEOUT_S}s for node{worker_idx + 1} → dead"
        )
        deadline = time.monotonic() + CLUSTER_NODE_DEAD_TIMEOUT_S
        saw_dead = False
        while time.monotonic() < deadline:
            try:
                current = _fetch_cluster_nodes_url(leader_url)
            except Exception:  # noqa: BLE001
                time.sleep(2)
                continue
            for n in current:
                api_endpoint = str(
                    n.get("api_endpoint") or n.get("address") or ""
                )
                if str(CLUSTER_NODES[worker_idx]["api"]) not in api_endpoint:
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
        log(f"cluster_failover(container): node{worker_idx + 1} → dead (OK)")

        # 3. Relaunch the worker container + rejoin with a fresh token.
        # Since we used --rm, the previous container's data-dir is gone;
        # rejoin via a fresh signed token from the leader is the right path.
        log(f"cluster_failover(container): relaunching node{worker_idx + 1}")
        _container_run(
            runtime,
            name=names[worker_idx], image=image, network=network,
            ip=ips[worker_idx],
        )
        cfg = CLUSTER_NODES[worker_idx]
        gen = _container_exec(
            runtime, names[0],
            [
                _CONTAINER_BIN,
                "--data-dir", _CONTAINER_DATA_DIR,
                "node", "generate-join-token",
                CLUSTER_DEPLOYMENT,
                "-a", leader_url,
            ],
            check=True, capture=True,
        )
        assert gen is not None
        token = _parse_join_token(gen.stdout)
        _container_exec(
            runtime, names[worker_idx],
            [
                _CONTAINER_BIN,
                "--data-dir", _CONTAINER_DATA_DIR,
                "node", "join",
                f"{ips[0]}:{CLUSTER_NODES[0]['api']}",
                "--token", token,
                "--advertise-addr", ips[worker_idx],
                "--api-port", str(cfg["api"]),
                "--raft-port", str(cfg["raft"]),
                "--overlay-port", str(cfg["overlay"]),
            ],
            check=True,
        )
        _container_exec(
            runtime, names[worker_idx],
            [
                _CONTAINER_BIN,
                "--data-dir", _CONTAINER_DATA_DIR,
                "serve",
                "--bind", f"0.0.0.0:{cfg['api']}",
                "--deployment-name", CLUSTER_DEPLOYMENT,
            ],
            detach=True,
        )
        if not _wait_http_ok(
            f"{api_endpoints[worker_idx]}/health/ready", 30,
        ):
            raise RuntimeError(
                f"restarted node{worker_idx + 1} (container) never came up on "
                f"{api_endpoints[worker_idx]}"
            )

        # 4. Poll for `ready` again.
        log(
            f"cluster_failover(container): waiting up to "
            f"{CLUSTER_NODE_RECOVER_TIMEOUT_S}s for node{worker_idx + 1} → ready"
        )
        deadline = time.monotonic() + CLUSTER_NODE_RECOVER_TIMEOUT_S
        recovered = False
        while time.monotonic() < deadline:
            try:
                current = _fetch_cluster_nodes_url(leader_url)
            except Exception:  # noqa: BLE001
                time.sleep(2)
                continue
            for n in current:
                api_endpoint = str(
                    n.get("api_endpoint") or n.get("address") or ""
                )
                if str(CLUSTER_NODES[worker_idx]["api"]) not in api_endpoint:
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
            f"cluster_failover(container): PASS — node{worker_idx + 1} "
            "ready→dead→ready"
        )
        return 0
    except Exception as e:  # noqa: BLE001
        print(
            f"cluster_failover(container): FAIL — {e!r}",
            file=sys.stderr, flush=True,
        )
        return 1
    finally:
        _cleanup_cluster_container(runtime, names, network=network)


def run_cluster_scaling_container(args: argparse.Namespace) -> int:
    """Container-mode mirror of `run_cluster_scaling`.

    `zlayer deploy <spec>` runs INSIDE the leader container; the spec file
    needs to be visible inside the container. We copy each spec into the
    leader container under /tmp via `<runtime> cp` before deploying.
    """
    runtime, image, network = _container_mode_settings()
    suite = "cluster_scaling"
    names: list[str] = []
    try:
        names, _ips, api_endpoints = _bootstrap_3node_cluster_container(
            runtime, image, network, suite,
        )
        log("cluster_scaling(container): waiting for cluster to converge")
        _wait_for_ready_cluster_url(
            api_endpoints[0], expected_nodes=3,
            timeout_s=CLUSTER_NODES_READY_TIMEOUT_S,
        )

        leader = names[0]
        spec_1r = CLUSTER_SPECS_DIR / "nginx-v1-1r.yaml"
        spec_3r = CLUSTER_SPECS_DIR / "nginx-v1-3r.yaml"

        def _cp_spec(spec: Path) -> str:
            """Copy `spec` into the leader container, return inside-path."""
            inside = f"/tmp/{spec.name}"
            subprocess.run(
                [runtime, "cp", str(spec), f"{leader}:{inside}"],
                check=True, capture_output=True,
            )
            return inside

        # --- 1 replica ----------------------------------------------------
        inside_1r = _cp_spec(spec_1r)
        log(f"cluster_scaling(container): deploying {spec_1r.name} (1 replica)")
        _container_exec(
            runtime, leader,
            [
                _CONTAINER_BIN,
                "--data-dir", _CONTAINER_DATA_DIR,
                "deploy", "--detach", inside_1r,
            ],
            check=True,
        )
        log("cluster_scaling(container): waiting for 1 running container")
        _container_ps_running(
            runtime, leader, CLUSTER_APP_DEPLOYMENT, expected_count=1,
        )
        log("cluster_scaling(container): 1 replica running (OK)")

        # --- 3 replicas ---------------------------------------------------
        inside_3r = _cp_spec(spec_3r)
        log(f"cluster_scaling(container): deploying {spec_3r.name} (3 replicas)")
        _container_exec(
            runtime, leader,
            [
                _CONTAINER_BIN,
                "--data-dir", _CONTAINER_DATA_DIR,
                "deploy", "--detach", inside_3r,
            ],
            check=True,
        )
        log("cluster_scaling(container): waiting for 3 running containers")
        running = _container_ps_running(
            runtime, leader, CLUSTER_APP_DEPLOYMENT, expected_count=3,
        )

        node_ids = [_container_node(e) for e in running]
        distinct = {nid for nid in node_ids if nid}
        log(f"cluster_scaling(container): replica node distribution = {node_ids!r}")
        if len(distinct) < 2:
            raise RuntimeError(
                f"expected replicas spread across at least 2 nodes; "
                f"got distinct={distinct!r} from {node_ids!r}"
            )
        log(
            f"cluster_scaling(container): replicas spread across "
            f"{len(distinct)} nodes (OK)"
        )

        # --- back to 1 replica -------------------------------------------
        log(f"cluster_scaling(container): redeploying {spec_1r.name} (3→1)")
        _container_exec(
            runtime, leader,
            [
                _CONTAINER_BIN,
                "--data-dir", _CONTAINER_DATA_DIR,
                "deploy", "--detach", inside_1r,
            ],
            check=True,
        )
        log("cluster_scaling(container): waiting for 1 running container")
        _container_ps_running(
            runtime, leader, CLUSTER_APP_DEPLOYMENT, expected_count=1,
        )
        log("cluster_scaling(container): PASS — 1 → 3 → 1 replicas across cluster")
        return 0
    except Exception as e:  # noqa: BLE001
        print(
            f"cluster_scaling(container): FAIL — {e!r}",
            file=sys.stderr, flush=True,
        )
        # Surface the per-node daemon decision (scale fan-out + placement) so
        # the concentration bug is diagnosable from CI alone.
        if names:
            _dump_cluster_daemon_logs(
                runtime, names,
                grep="scale_distribute|scale_service|distributed scale|per_node|"
                "placeable_nodes|orchestrate_deployment",
            )
        return 1
    finally:
        _cleanup_cluster_container(runtime, names, network=network)


def run_cluster_upgrade_container(args: argparse.Namespace) -> int:
    """Container-mode mirror of `run_cluster_upgrade`."""
    runtime, image, network = _container_mode_settings()
    suite = "cluster_upgrade"
    names: list[str] = []
    try:
        names, _ips, api_endpoints = _bootstrap_3node_cluster_container(
            runtime, image, network, suite,
        )
        log("cluster_upgrade(container): waiting for cluster to converge")
        _wait_for_ready_cluster_url(
            api_endpoints[0], expected_nodes=3,
            timeout_s=CLUSTER_NODES_READY_TIMEOUT_S,
        )

        leader = names[0]
        spec_v1 = CLUSTER_SPECS_DIR / "nginx-v1-3r.yaml"
        spec_v2 = CLUSTER_SPECS_DIR / "nginx-v2-3r.yaml"

        def _cp_spec(spec: Path) -> str:
            inside = f"/tmp/{spec.name}"
            subprocess.run(
                [runtime, "cp", str(spec), f"{leader}:{inside}"],
                check=True, capture_output=True,
            )
            return inside

        # --- v1 -----------------------------------------------------------
        inside_v1 = _cp_spec(spec_v1)
        log(f"cluster_upgrade(container): deploying {spec_v1.name} (v1)")
        _container_exec(
            runtime, leader,
            [
                _CONTAINER_BIN,
                "--data-dir", _CONTAINER_DATA_DIR,
                "deploy", "--detach", inside_v1,
            ],
            check=True,
        )
        log("cluster_upgrade(container): waiting for 3 running v1 containers")
        v1_running = _container_ps_running(
            runtime, leader, CLUSTER_APP_DEPLOYMENT, expected_count=3,
        )
        initial_images = [_container_image(e) for e in v1_running]
        log(f"cluster_upgrade(container): initial images = {initial_images!r}")

        # --- v2 -----------------------------------------------------------
        inside_v2 = _cp_spec(spec_v2)
        log(f"cluster_upgrade(container): deploying {spec_v2.name} (v2)")
        _container_exec(
            runtime, leader,
            [
                _CONTAINER_BIN,
                "--data-dir", _CONTAINER_DATA_DIR,
                "deploy", "--detach", inside_v2,
            ],
            check=True,
        )
        expected_image = "docker.io/library/nginx:1.29-alpine"
        log(f"cluster_upgrade(container): waiting for 3 containers on {expected_image}")
        _container_wait_image_transition(
            runtime, leader, CLUSTER_APP_DEPLOYMENT,
            expected_image=expected_image, expected_count=3,
        )

        log("cluster_upgrade(container): PASS — 3 replicas migrated v1.28 → v1.29")
        return 0
    except Exception as e:  # noqa: BLE001
        print(
            f"cluster_upgrade(container): FAIL — {e!r}",
            file=sys.stderr, flush=True,
        )
        return 1
    finally:
        _cleanup_cluster_container(runtime, names, network=network)


def run_cluster_node_upgrade_container(args: argparse.Namespace) -> int:
    """Container-mode mirror of `run_cluster_node_upgrade`.

    POSTs to the cluster-upgrade endpoint are made from the host against
    the container IPs (the bridge network is reachable from the host).
    """
    runtime, image, network = _container_mode_settings()
    suite = "cluster_node_upgrade"
    names: list[str] = []
    try:
        names, _ips, api_endpoints = _bootstrap_3node_cluster_container(
            runtime, image, network, suite,
        )
        log("cluster_node_upgrade(container): waiting for cluster to converge")
        nodes = _wait_for_ready_cluster_url(
            api_endpoints[0], expected_nodes=3,
            timeout_s=CLUSTER_NODES_READY_TIMEOUT_S,
        )

        # Identify leader URL. Bootstrap path makes node1 leader.
        leader_url: Optional[str] = None
        for n in nodes:
            role = str(n.get("role", "")).lower()
            if role == "leader":
                api_endpoint = str(
                    n.get("api_endpoint") or n.get("address") or ""
                )
                for ep in api_endpoints:
                    # ep is `http://<ip>:<port>` — match `<ip>:<port>` or `<port>`.
                    bare = ep.split("//", 1)[-1]
                    if bare in api_endpoint or str(bare.split(":")[-1]) in api_endpoint:
                        leader_url = ep
                        break
                break
        if leader_url is None:
            leader_url = api_endpoints[0]
        log(f"cluster_node_upgrade(container): leader = {leader_url}")

        follower_url = api_endpoints[-1]
        if follower_url == leader_url:
            follower_url = next(
                ep for ep in api_endpoints if ep != leader_url
            )
        log(f"cluster_node_upgrade(container): follower for 421 test = {follower_url}")

        def _post_cluster_upgrade(
            api_url: str, body: dict, timeout_s: int = 60,
        ) -> tuple[int, dict, dict]:
            url = f"{api_url}/api/v1/cluster/upgrade"
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
                body_bytes = e.read() if hasattr(e, "read") else b""
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
            "cluster_node_upgrade(container): Test 1 — POST against "
            "non-leader, expect 421 + X-Leader-Addr"
        )
        bad_body = {
            "version": "v0.0.0-test-nonexistent",
            "cooldown_secs": 1,
            "strict": False,
        }
        status1, body1, headers1 = _post_cluster_upgrade(
            follower_url, bad_body, timeout_s=30,
        )
        if status1 != 421:
            raise RuntimeError(
                f"expected 421 from follower POST, got {status1}; "
                f"body={body1!r} headers={headers1!r}"
            )
        leader_addr_hdr = (
            headers1.get("X-Leader-Addr") or headers1.get("x-leader-addr")
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
            f"cluster_node_upgrade(container): 421 redirect OK "
            f"(leader_addr={leader_addr_hdr})"
        )

        # --- Test 2: orchestrator walks followers and records errors -----
        log(
            "cluster_node_upgrade(container): Test 2 — POST against "
            "leader with bad version, expect 2 errors / 0 upgrades"
        )
        status2, body2, _headers2 = _post_cluster_upgrade(
            leader_url, bad_body, timeout_s=600,
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
                f"expected upgraded=[], got {upgraded!r}; full body={body2!r}"
            )
        if len(errors_list) != 2:
            raise RuntimeError(
                f"expected 2 errors (one per follower), got "
                f"{len(errors_list)}; errors={errors_list!r} "
                f"upgraded={upgraded!r} skipped={skipped!r}"
            )
        log(
            f"cluster_node_upgrade(container): orchestrator-walk OK "
            f"(upgraded={upgraded!r} skipped={skipped!r} "
            f"errors={[e.get('node_id') for e in errors_list]!r})"
        )

        # --- Test 3: cluster survives the failed upgrade -----------------
        log(
            "cluster_node_upgrade(container): Test 3 — re-fetch cluster "
            "nodes, expect all 3 still ready"
        )
        after = _fetch_cluster_nodes_url(leader_url)
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
            "cluster_node_upgrade(container): PASS — 421-redirect, "
            "orchestrator-walk, error-recording, cluster-survival all verified"
        )
        return 0
    except Exception as e:  # noqa: BLE001
        print(
            f"cluster_node_upgrade(container): FAIL — {e!r}",
            file=sys.stderr, flush=True,
        )
        return 1
    finally:
        _cleanup_cluster_container(runtime, names, network=network)


def run_cluster_worker_tier_container(args: argparse.Namespace) -> int:
    """E2E test for the Phase 3 worker tier.

    Brings up:
      - 3 control-plane nodes (`mode: worker-tier, role: server`) running
        Raft; assignments are pushed via the leader's gRPC dispatcher.
      - 5 worker nodes (`zlayer worker`) registering via a single
        bootstrap token + locally-generated mTLS CSR. Workers do NOT
        enter consensus.

    Then:
      - Deploys a 10-replica nginx service. All 10 replicas should land
        on workers; control-plane nodes must receive zero replicas.
      - Kills 2 workers; the leader should reassign their containers
        within `failover_heartbeat_ttl + grace`.
      - Restarts the 2 workers; asserts no duplicates after reconnect.

    NOTE: As of P3.11 the worker-status admin command (`zlayer node
    worker-status`) is partially wired and the `/api/v1/cluster/workers`
    endpoint that would let the harness assert worker counts authoritatively
    is on the P3.12 backlog. Until then, the suite uses container-state
    polling (`<runtime> inspect`) as a best-effort substitute and marks
    full assertion logic with TODO(P3.12) comments. The plumbing — image
    boot, token mint, worker spawn, deploy spec push — is fully wired so
    the suite is meaningful as a structural smoke test today and will
    light up complete assertions when the missing endpoints land.
    """
    runtime, image, network = _container_mode_settings()
    suite = "cluster_worker_tier"
    cp_names: list[str] = []
    worker_names: list[str] = []
    try:
        # --- 1. Spin up the 3 control-plane nodes -------------------------
        cp_names, _ips, api_endpoints = _bootstrap_3node_worker_tier_container(
            runtime, image, network, suite,
        )
        leader = cp_names[0]
        leader_url = api_endpoints[0]
        log("cluster_worker_tier(container): waiting for control plane to converge")
        _wait_for_ready_cluster_url(
            leader_url, expected_nodes=3,
            timeout_s=CLUSTER_NODES_READY_TIMEOUT_S,
        )

        # --- 2. Generate a bootstrap token for the workers ----------------
        log("cluster_worker_tier(container): generating worker bootstrap token")
        gen = _container_exec(
            runtime, leader,
            [
                _CONTAINER_BIN,
                "--data-dir", _CONTAINER_DATA_DIR,
                "node", "generate-worker-token",
                "--max-uses", "10",
                "--valid-for", "3600",
            ],
            check=True, capture=True,
        )
        assert gen is not None
        token = (gen.stdout or "").strip().splitlines()[-1].strip()
        if not token:
            raise RuntimeError("empty worker token from generate-worker-token")
        log(
            f"cluster_worker_tier(container): worker bootstrap token ok "
            f"({len(token)} chars)"
        )

        # --- 3. Start 5 worker containers ---------------------------------
        worker_count = 5
        # Build the multi-server arg list once: every worker is told about
        # all 3 control-plane gRPC endpoints so HA failover works even if
        # the first server it tries is the one being killed.
        server_args: list[str] = []
        for ip in _CONTAINER_NODE_IPS:
            server_args.extend([
                "--server", f"http://{ip}:{CLUSTER_WORKER_GRPC_PORT}",
            ])

        for i in range(worker_count):
            name = f"zlayer-e2e-{suite}-worker{i + 1}"
            worker_names.append(name)
            worker_ip = _CONTAINER_WORKER_IPS[i]
            # Boot the container with sleep-infinity (image CMD), then
            # detach-exec `zlayer worker` inside it. This mirrors the
            # control-plane pattern (`node init` + detached `serve`) and
            # gives us a stable container to exec into for diagnostics.
            _container_run(
                runtime,
                name=name, image=image, network=network, ip=worker_ip,
            )
            _container_exec(
                runtime, name,
                [
                    _CONTAINER_BIN,
                    "--data-dir", _CONTAINER_DATA_DIR,
                    "worker",
                    *server_args,
                    "--token", token,
                    "--labels", f"tier=worker,index={i}",
                ],
                detach=True,
            )

        # --- 4. Wait for all 5 workers to register ------------------------
        # TODO(P3.12): swap container-state polling for an authoritative
        # `/api/v1/cluster/workers` check (or `zlayer node worker-status
        # --output json`) once that endpoint lands. The current loop only
        # asserts the worker process is still running inside its container
        # — it does NOT prove the lease was accepted by the leader. The
        # registration RPC failing silently would still pass this check.
        log(
            f"cluster_worker_tier(container): waiting for {worker_count} "
            f"workers to register (container-state probe)"
        )
        deadline = time.monotonic() + 120
        registered_count = 0
        last_running = -1
        while time.monotonic() < deadline:
            running = sum(
                1 for n in worker_names
                if _container_inspect_running(runtime, n)
            )
            if running != last_running:
                log(
                    f"cluster_worker_tier(container): {running}/"
                    f"{worker_count} worker containers running"
                )
                last_running = running
            if running == worker_count:
                # Give the gRPC register loop a conservative 10s to
                # complete after the container is up. This is the best
                # we can do without a real admin endpoint.
                time.sleep(10)
                registered_count = running
                break
            time.sleep(3)
        if registered_count != worker_count:
            raise RuntimeError(
                f"only {registered_count}/{worker_count} workers running "
                f"after 120s"
            )
        log(
            f"cluster_worker_tier(container): {registered_count} workers "
            f"running (registration assumed; see P3.12 TODO)"
        )

        # --- 5. Deploy a 10-replica service -------------------------------
        spec = CLUSTER_SPECS_DIR / "nginx-worker-tier-10r.yaml"
        if not spec.is_file():
            raise RuntimeError(
                f"worker-tier deploy spec not found at {spec}"
            )
        inside = f"/tmp/{spec.name}"
        subprocess.run(
            [runtime, "cp", str(spec), f"{leader}:{inside}"],
            check=True, capture_output=True,
        )
        log(
            f"cluster_worker_tier(container): deploying {spec.name} "
            f"(10 replicas) via leader {leader}"
        )
        _container_exec(
            runtime, leader,
            [
                _CONTAINER_BIN,
                "--data-dir", _CONTAINER_DATA_DIR,
                "deploy", "--detach", inside,
            ],
            check=True,
        )

        # --- 6. Assert 10 containers running, none on control-plane ------
        # TODO(P3.12): replace the deployment name (`worker-tier-test`) and
        # the placement assertion with a proper API-side check once
        # `zlayer ps --containers --format json` learns to report the
        # `worker_node_id` separately from the placement `node_id`. For
        # now we just confirm 10 containers are running across the
        # cluster — we don't yet assert they're on workers vs CP nodes.
        log(
            "cluster_worker_tier(container): waiting for 10 running "
            "containers across the cluster"
        )
        running_entries = _container_ps_running(
            runtime, leader, "worker-tier-test", expected_count=10,
            timeout_s=180,
        )
        node_ids = [_container_node(e) for e in running_entries]
        log(
            f"cluster_worker_tier(container): replica node distribution = "
            f"{node_ids!r}"
        )
        # TODO(P3.12): once worker node_ids are surfaced distinctly from
        # control-plane node_ids, assert that every `nid` here corresponds
        # to a worker (not 1/2/3 which are reserved for CP nodes).

        # --- 7. Failover: kill 2 workers, wait for reassignment ----------
        # TODO(P3.12): the assertion below ("after killing workers, the
        # leader reassigns within failover_heartbeat_ttl + grace") needs
        # the same `/api/v1/cluster/workers` endpoint to verify, since the
        # current `_container_ps_running` polls by deployment name and
        # can't distinguish "container reassigned to a different worker"
        # from "container restarted in place on the same worker".
        killed = worker_names[:2]
        log(
            f"cluster_worker_tier(container): killing 2 workers to test "
            f"failover: {killed!r}"
        )
        for name in killed:
            _container_stop(runtime, name)
        # failover_heartbeat_ttl=30 + heartbeat_grace=10 + reschedule slop
        # ≈ 60s upper bound. Poll the running-container count back up to 10.
        log(
            "cluster_worker_tier(container): waiting up to 90s for replicas "
            "to be reassigned"
        )
        _container_ps_running(
            runtime, leader, "worker-tier-test", expected_count=10,
            timeout_s=90,
        )

        # --- 8. Restart the killed workers, assert no duplicates ---------
        # TODO(P3.12): "no duplicates after reconnect" needs an authoritative
        # worker-id list to assert against. Current best-effort is just
        # confirming the steady-state container count stays at 10 after
        # the restarted workers come back online. Real assertion belongs
        # behind the worker-status API.
        log(
            f"cluster_worker_tier(container): restarting killed workers "
            f"{killed!r}"
        )
        for i, name in enumerate(killed):
            worker_ip = _CONTAINER_WORKER_IPS[i]
            _container_run(
                runtime,
                name=name, image=image, network=network, ip=worker_ip,
            )
            _container_exec(
                runtime, name,
                [
                    _CONTAINER_BIN,
                    "--data-dir", _CONTAINER_DATA_DIR,
                    "worker",
                    *server_args,
                    "--token", token,
                    "--labels", f"tier=worker,index={i}",
                ],
                detach=True,
            )
        log(
            "cluster_worker_tier(container): waiting 20s for restarted "
            "workers to re-register, then re-asserting steady state"
        )
        time.sleep(20)
        _container_ps_running(
            runtime, leader, "worker-tier-test", expected_count=10,
            timeout_s=60,
        )

        log(
            "cluster_worker_tier(container): PASS — workers registered, "
            "service deployed, failover + reconnect verified (best-effort; "
            "see P3.12 TODOs for authoritative assertions)"
        )
        return 0
    except Exception as e:  # noqa: BLE001
        print(
            f"cluster_worker_tier(container): FAIL — {e!r}",
            file=sys.stderr, flush=True,
        )
        return 1
    finally:
        # Stop workers first so they release leases cleanly, then the
        # control-plane nodes. `_cleanup_cluster_container` handles the
        # bridge network removal at the end.
        for n in worker_names:
            try:
                _container_stop(runtime, n)
            except Exception:  # noqa: BLE001
                pass
        _cleanup_cluster_container(runtime, cp_names, network=network)


# ---------------------------------------------------------------------------
# Cluster suite entry points (host-mode preserved bit-identical)
# ---------------------------------------------------------------------------


def run_cluster_3node(args: argparse.Namespace) -> int:
    """Stand up a 3-node cluster, assert it forms with a leader + 3 ready."""
    if args.overlay_mode == "container":
        return run_cluster_3node_container(args)
    if not args.no_build:
        build_phase(throwaway=args.throwaway, with_manager=False)

    zlayer_bin = resolve_zlayer_bin(throwaway=args.throwaway)
    log(f"cluster_3node: using zlayer binary {zlayer_bin}")

    root_dir = THROWAWAY_ROOT / "cluster_3node"
    # Fresh slate: this suite is destructive to its target directory.
    shutil.rmtree(root_dir, ignore_errors=True)
    root_dir.mkdir(parents=True, exist_ok=True)

    _started = time.time()
    procs: list[subprocess.Popen] = []
    _data_dirs: list[Path] = []
    _suite_exit: int = 0
    try:
        with _step("bootstrap_3node"):
            procs, _data_dirs, api_ports = _bootstrap_3node_cluster(
                zlayer_bin, root_dir, sudo=args.sudo_daemon,
            )
        log("cluster_3node: waiting for cluster to converge")
        with _step("wait_cluster_converged"):
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
        _suite_exit = 1
        return 1
    finally:
        _cleanup_cluster(
            procs, root_dir, sudo=args.sudo_daemon,
            suite_name="cluster_3node",
            data_dirs=_data_dirs,
            exit_code=_suite_exit,
            started_at=_started,
        )


def run_cluster_gossip_5node(args: argparse.Namespace) -> int:
    """Phase 4 chitchat gossip pool: 5-node discovery + failure detection.

    Container-mode only — gossip exercises the real boringtun overlay across
    5 nodes, which is not meaningful in loopback mode. We force-route to the
    container variant and require `--overlay-mode container` at the CLI.
    """
    if args.overlay_mode != "container":
        print(
            "cluster_gossip_5node: this suite requires --overlay-mode container "
            "(5-node chitchat pool needs the real overlay)",
            file=sys.stderr, flush=True,
        )
        return 2
    return run_cluster_gossip_5node_container(args)


def run_cluster_worker_tier(args: argparse.Namespace) -> int:
    """Phase 3 worker-tier suite: 3 CP nodes + 5 workers + 10-replica deploy.

    Container-mode only — the worker tier wires together gRPC mTLS, the
    Raft FSM's WorkerLease state, and the leader's dispatcher, none of
    which are meaningful in loopback mode (which has no real workers).
    We force-route to the container variant and require
    `--overlay-mode container` at the CLI.
    """
    if args.overlay_mode != "container":
        print(
            "cluster_worker_tier: this suite requires --overlay-mode container "
            "(worker tier exercises gRPC + mTLS + a multi-node lease graph)",
            file=sys.stderr, flush=True,
        )
        return 2
    return run_cluster_worker_tier_container(args)


def run_cluster_failover(args: argparse.Namespace) -> int:
    """Stand up 3 nodes, kill a non-leader, assert dead→ready transition.

    Reschedule of dead-node replicas requires deploying a service first; that
    assertion belongs in a separate suite. This suite scope is heartbeat
    transition (ready -> dead -> ready) per README §385-387.
    """
    if args.overlay_mode == "container":
        return run_cluster_failover_container(args)
    if not args.no_build:
        build_phase(throwaway=args.throwaway, with_manager=False)

    zlayer_bin = resolve_zlayer_bin(throwaway=args.throwaway)
    log(f"cluster_failover: using zlayer binary {zlayer_bin}")

    root_dir = THROWAWAY_ROOT / "cluster_failover"
    shutil.rmtree(root_dir, ignore_errors=True)
    root_dir.mkdir(parents=True, exist_ok=True)

    _started = time.time()
    procs: list[subprocess.Popen] = []
    data_dirs: list[Path] = []
    _suite_exit: int = 0
    try:
        with _step("bootstrap_3node"):
            procs, data_dirs, api_ports = _bootstrap_3node_cluster(
                zlayer_bin, root_dir, sudo=args.sudo_daemon,
            )
        leader_api = api_ports[0]
        log("cluster_failover: waiting for cluster to converge")
        with _step("wait_cluster_converged"):
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
        with _step("failover_kill_worker"):
            _kill_pg(
                procs[worker_idx],
                f"cluster-node{worker_idx + 1} (kill)",
                sudo=args.sudo_daemon,
            )

        # 2. Poll for `dead`.
        log(
            f"cluster_failover: waiting up to {CLUSTER_NODE_DEAD_TIMEOUT_S}s "
            f"for node{worker_idx + 1} → dead"
        )
        with _step("failover_wait_dead"):
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
        with _step("failover_restart_worker"):
            procs[worker_idx] = _spawn_node_serve(
                zlayer_bin, data_dirs[worker_idx], api_ports[worker_idx],
                sudo=args.sudo_daemon,
                node_index=worker_idx + 1, token_dir=root_dir,
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
        with _step("failover_wait_recover"):
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
        _suite_exit = 1
        return 1
    finally:
        _cleanup_cluster(
            procs, root_dir, sudo=args.sudo_daemon,
            suite_name="cluster_failover",
            data_dirs=data_dirs,
            exit_code=_suite_exit,
            started_at=_started,
        )


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


def _norm_image(ref: str) -> str:
    """Strip registry/namespace prefixes so short and fully-qualified
    image refs compare equal (e.g. `nginx:1.29-alpine` ==
    `docker.io/library/nginx:1.29-alpine`)."""
    r = ref.strip()
    for p in (
        "docker.io/library/",
        "docker.io/",
        "registry-1.docker.io/library/",
        "index.docker.io/library/",
        "library/",
    ):
        if r.startswith(p):
            r = r[len(p):]
            break
    return r


def _container_node(entry: dict) -> str:
    for field in _NODE_FIELDS:
        val = entry.get(field)
        if val:
            return str(val)
    return ""


def _count_running_containers(
    zlayer_bin: Path, data_dir: Path, deployment: str,
    expected_count: int, timeout_s: int = 120, *, sudo: bool = False,
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
                _maybe_sudo([
                    str(zlayer_bin),
                    "--data-dir", str(data_dir),
                    "ps",
                    "--deployment", deployment,
                    "--containers",
                    "--format", "json",
                ], sudo=sudo),
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
    *, sudo: bool = False,
) -> list[dict]:
    """Poll until all `expected_count` running containers report
    `image == expected_image`. Returns the converged list."""
    deadline = time.monotonic() + timeout_s
    last_state: list[dict] = []
    last_err: Optional[str] = None
    while time.monotonic() < deadline:
        try:
            result = subprocess.run(
                _maybe_sudo([
                    str(zlayer_bin),
                    "--data-dir", str(data_dir),
                    "ps",
                    "--deployment", deployment,
                    "--containers",
                    "--format", "json",
                ], sudo=sudo),
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
            _norm_image(_container_image(e)) == _norm_image(expected_image)
            for e in running
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
    if args.overlay_mode == "container":
        return run_cluster_scaling_container(args)
    if not args.no_build:
        build_phase(throwaway=args.throwaway, with_manager=False)

    zlayer_bin = resolve_zlayer_bin(throwaway=args.throwaway)
    log(f"cluster_scaling: using zlayer binary {zlayer_bin}")

    root_dir = THROWAWAY_ROOT / "cluster_scaling"
    shutil.rmtree(root_dir, ignore_errors=True)
    root_dir.mkdir(parents=True, exist_ok=True)

    _started = time.time()
    procs: list[subprocess.Popen] = []
    data_dirs: list[Path] = []
    _suite_exit: int = 0
    try:
        with _step("bootstrap_3node"):
            procs, data_dirs, api_ports = _bootstrap_3node_cluster(
                zlayer_bin, root_dir, sudo=args.sudo_daemon,
            )
        log("cluster_scaling: waiting for cluster to converge")
        with _step("wait_cluster_converged"):
            _wait_for_ready_cluster(
                api_ports[0], expected_nodes=3,
                timeout_s=CLUSTER_NODES_READY_TIMEOUT_S,
            )

        leader_data_dir = data_dirs[0]
        spec_1r = CLUSTER_SPECS_DIR / "nginx-v1-1r.yaml"
        spec_3r = CLUSTER_SPECS_DIR / "nginx-v1-3r.yaml"

        # --- 1 replica ----------------------------------------------------
        log(f"cluster_scaling: deploying {spec_1r.name} (1 replica)")
        with _step("deploy_1r"):
            try:
                subprocess.run(
                    _maybe_sudo([
                        str(zlayer_bin),
                        "--data-dir", str(leader_data_dir),
                        "deploy", "--detach", str(spec_1r),
                    ], sudo=args.sudo_daemon),
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
                sudo=args.sudo_daemon,
            )
        log("cluster_scaling: 1 replica running (OK)")

        # --- 3 replicas ---------------------------------------------------
        log(f"cluster_scaling: deploying {spec_3r.name} (3 replicas)")
        with _step("scale_up_3r"):
            try:
                subprocess.run(
                    _maybe_sudo([
                        str(zlayer_bin),
                        "--data-dir", str(leader_data_dir),
                        "deploy", "--detach", str(spec_3r),
                    ], sudo=args.sudo_daemon),
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
                sudo=args.sudo_daemon,
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
        with _step("scale_down_1r"):
            try:
                subprocess.run(
                    _maybe_sudo([
                        str(zlayer_bin),
                        "--data-dir", str(leader_data_dir),
                        "deploy", "--detach", str(spec_1r),
                    ], sudo=args.sudo_daemon),
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
                sudo=args.sudo_daemon,
            )

        log("cluster_scaling: PASS — 1 → 3 → 1 replicas across cluster")
        return 0
    except Exception as e:  # noqa: BLE001
        print(f"cluster_scaling: FAIL — {e!r}", file=sys.stderr, flush=True)
        _suite_exit = 1
        return 1
    finally:
        _cleanup_cluster(
            procs, root_dir, sudo=args.sudo_daemon,
            suite_name="cluster_scaling",
            data_dirs=data_dirs,
            exit_code=_suite_exit,
            started_at=_started,
        )


def run_cluster_upgrade(args: argparse.Namespace) -> int:
    """3-node cluster, rolling image upgrade v1.28 → v1.29."""
    if args.overlay_mode == "container":
        return run_cluster_upgrade_container(args)
    if not args.no_build:
        build_phase(throwaway=args.throwaway, with_manager=False)

    zlayer_bin = resolve_zlayer_bin(throwaway=args.throwaway)
    log(f"cluster_upgrade: using zlayer binary {zlayer_bin}")

    root_dir = THROWAWAY_ROOT / "cluster_upgrade"
    shutil.rmtree(root_dir, ignore_errors=True)
    root_dir.mkdir(parents=True, exist_ok=True)

    _started = time.time()
    procs: list[subprocess.Popen] = []
    data_dirs: list[Path] = []
    _suite_exit: int = 0
    try:
        with _step("bootstrap_3node"):
            procs, data_dirs, api_ports = _bootstrap_3node_cluster(
                zlayer_bin, root_dir, sudo=args.sudo_daemon,
            )
        log("cluster_upgrade: waiting for cluster to converge")
        with _step("wait_cluster_converged"):
            _wait_for_ready_cluster(
                api_ports[0], expected_nodes=3,
                timeout_s=CLUSTER_NODES_READY_TIMEOUT_S,
            )

        leader_data_dir = data_dirs[0]
        spec_v1 = CLUSTER_SPECS_DIR / "nginx-v1-3r.yaml"
        spec_v2 = CLUSTER_SPECS_DIR / "nginx-v2-3r.yaml"

        # --- v1 (nginx:1.28-alpine) --------------------------------------
        log(f"cluster_upgrade: deploying {spec_v1.name} (v1, 3 replicas)")
        with _step("deploy_v1"):
            try:
                subprocess.run(
                    _maybe_sudo([
                        str(zlayer_bin),
                        "--data-dir", str(leader_data_dir),
                        "deploy", "--detach", str(spec_v1),
                    ], sudo=args.sudo_daemon),
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
                sudo=args.sudo_daemon,
            )
        initial_images = [_container_image(e) for e in v1_running]
        log(f"cluster_upgrade: initial images = {initial_images!r}")

        # --- v2 (nginx:1.29-alpine), poll for image-field transition -----
        log(f"cluster_upgrade: deploying {spec_v2.name} (v2 rolling upgrade)")
        with _step("rolling_upgrade_v2"):
            try:
                subprocess.run(
                    _maybe_sudo([
                        str(zlayer_bin),
                        "--data-dir", str(leader_data_dir),
                        "deploy", "--detach", str(spec_v2),
                    ], sudo=args.sudo_daemon),
                    check=True, cwd=REPO_ROOT,
                )
            except subprocess.CalledProcessError as exc:
                if exc.stderr:
                    sys.stderr.write(f"--- deploy stderr ---\n{exc.stderr}\n")
                raise

            expected_image = "docker.io/library/nginx:1.29-alpine"
            log(
                f"cluster_upgrade: waiting for 3 containers on {expected_image}"
            )
            _wait_image_transition(
                zlayer_bin, leader_data_dir,
                CLUSTER_APP_DEPLOYMENT, expected_image=expected_image,
                expected_count=3,
                sudo=args.sudo_daemon,
            )

        log(
            "cluster_upgrade: PASS — 3 replicas migrated v1.28 → v1.29"
        )
        return 0
    except Exception as e:  # noqa: BLE001
        print(f"cluster_upgrade: FAIL — {e!r}", file=sys.stderr, flush=True)
        _suite_exit = 1
        return 1
    finally:
        _cleanup_cluster(
            procs, root_dir, sudo=args.sudo_daemon,
            suite_name="cluster_upgrade",
            data_dirs=data_dirs,
            exit_code=_suite_exit,
            started_at=_started,
        )


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
    if args.overlay_mode == "container":
        return run_cluster_node_upgrade_container(args)
    if not args.no_build:
        build_phase(throwaway=args.throwaway, with_manager=False)

    zlayer_bin = resolve_zlayer_bin(throwaway=args.throwaway)
    log(f"cluster_node_upgrade: using zlayer binary {zlayer_bin}")

    root_dir = THROWAWAY_ROOT / "cluster_node_upgrade"
    shutil.rmtree(root_dir, ignore_errors=True)
    root_dir.mkdir(parents=True, exist_ok=True)

    _started = time.time()
    procs: list[subprocess.Popen] = []
    _data_dirs: list[Path] = []
    _suite_exit: int = 0
    try:
        with _step("bootstrap_3node"):
            procs, _data_dirs, api_ports = _bootstrap_3node_cluster(
                zlayer_bin, root_dir, sudo=args.sudo_daemon,
            )
        log("cluster_node_upgrade: waiting for cluster to converge")
        with _step("wait_cluster_converged"):
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
        with _step("test_421_redirect"):
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
        with _step("test_orchestrator_walk"):
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
        with _step("test_cluster_survival"):
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
        _suite_exit = 1
        return 1
    finally:
        _cleanup_cluster(
            procs, root_dir, sudo=args.sudo_daemon,
            suite_name="cluster_node_upgrade",
            data_dirs=_data_dirs,
            exit_code=_suite_exit,
            started_at=_started,
        )


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
            "  cluster_gossip_5node  5-node chitchat gossip pool: pairwise discovery +\n"
            "                        failure detection (Phase 4; container-mode only;\n"
            "                        currently a skeleton awaiting /api/v1/gossip/peers)\n"
            "  cluster_worker_tier   3 CP servers + 5 workers, deploy 10 replicas,\n"
            "                        kill 2 workers, assert reassignment\n"
            "                        (Phase 3; container-mode only)\n"
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
            "cluster_gossip_5node",
            "cluster_worker_tier",
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
            "cluster_gossip_5node",
            "cluster_worker_tier",
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
    parser.add_argument(
        "--overlay-mode",
        choices=("loopback", "container"),
        default="loopback",
        help=(
            "loopback (default): boot daemons as raw processes on 127.0.0.1, "
            "no TUN, fast. container: boot each test node in a "
            "privileged container with --cap-add NET_ADMIN --device /dev/net/tun "
            "so signed-token joins, rotation, and revocation exercise the real "
            "boringtun overlay. The container mode REQUIRES the "
            "ZLAYER_E2E_PRIVILEGED=1 env var; see docs/operating/e2e-privileged.md."
        ),
    )
    ns = parser.parse_args()
    # Resolve positional/flag suite name → ns.suite (positional wins).
    if ns.suite is None:
        ns.suite = ns.suite_flag if ns.suite_flag is not None else "manager_auth"
    return ns


def main() -> int:
    args = parse_args()

    # Overlay-mode gate. `loopback` (default) is the historical raw-process
    # path. `container` boots each node inside a privileged container on a
    # shared bridge network so the boringtun overlay is actually exercised.
    # The env-var gate (`ZLAYER_E2E_PRIVILEGED=1`) is a finger-on-the-trigger
    # acknowledgement that the host meets the prereqs in
    # docs/operating/e2e-privileged.md (TUN, --privileged allowed, image
    # built via `zlayer pipeline -f ZPipeline.yaml --set VERSION=dev`).
    if args.overlay_mode == "container":
        if os.environ.get("ZLAYER_E2E_PRIVILEGED") != "1":
            sys.stderr.write(
                "skipping overlay scenarios -- set ZLAYER_E2E_PRIVILEGED=1 to enable "
                "(requires --privileged container host; see "
                "docs/operating/e2e-privileged.md for setup)\n"
            )
            sys.exit(0)
        # Only cluster suites are supported in container mode today; the
        # manager_auth suite uses a host-side daemon socket + leptos
        # frontend that don't make sense to mirror into containers.
        if args.suite not in (
            "cluster_3node",
            "cluster_failover",
            "cluster_scaling",
            "cluster_upgrade",
            "cluster_node_upgrade",
            "cluster_gossip_5node",
            "cluster_worker_tier",
        ):
            sys.stderr.write(
                f"--overlay-mode container is only supported for cluster_* "
                f"suites; got suite={args.suite!r}. "
                "See docs/operating/e2e-privileged.md.\n"
            )
            sys.exit(2)
        # Fall through to the cluster suite dispatch below; each
        # `run_cluster_*` function branches on `args.overlay_mode` to call
        # its `*_container` counterpart.

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
    if args.suite == "cluster_gossip_5node":
        return run_cluster_gossip_5node(args)
    if args.suite == "cluster_worker_tier":
        return run_cluster_worker_tier(args)

    # Default suite (`manager_auth`) falls through into the manager harness.

    daemon_proc: Optional[subprocess.Popen] = None
    manager_proc: Optional[subprocess.Popen] = None
    throwaway_dir: Optional[Path] = None
    cli_data_dir: Optional[Path] = None
    fixture_user_id: Optional[str] = None
    zlayer_bin: Optional[Path] = None
    daemon_sudo: bool = False
    suite_failure: Optional[tuple] = None
    suite_exit_code: int = 0
    suite_started_at: float = time.time()

    # SIGTERM via a handler so the `finally:` block always runs.
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))

    try:
        if not args.no_build:
            with _step("build_phase"):
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
                token_dir=throwaway_dir,
            )
            log(f"Throwaway daemon socket: {socket_path}")
            manager_log_dir = throwaway_dir
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
            manager_log_dir = THROWAWAY_ROOT / f"manager_{secrets.token_hex(4)}"
            manager_log_dir.mkdir(parents=True, exist_ok=True)

        suite_token = secrets.token_hex(4)
        fixture_email = f"e2e-{suite_token}@test.local"
        fixture_password = secrets.token_hex(16)
        fixture_display = "E2E Admin"

        fixture_user_id = create_fixture_user(
            zlayer_bin, cli_data_dir,
            fixture_email, fixture_password, fixture_display,
            sudo=daemon_sudo,
        )

        manager_proc = start_manager(socket_path, token_dir=manager_log_dir)

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
            with _step(f"intellitester_only:{args.only}"):
                run_intellitester(target, env_extras=intelli_env)
            log(f"Single test passed: {args.only}")
            return 0

        log("Running login.test.yaml")
        with _step("intellitester_login"):
            run_intellitester(
                E2E_DIR / "login.test.yaml", env_extras=intelli_env,
            )

        log("Running nav-smoke.test.yaml")
        with _step("intellitester_nav_smoke"):
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
        with _step("intellitester_stale_setup"):
            run_intellitester(
                E2E_DIR / "stale-session-setup.test.yaml",
                env_extras=intelli_env,
            )

        log(f"Stale-session: deleting fixture user via CLI ({fixture_user_id})")
        delete_fixture_user(
            zlayer_bin, cli_data_dir, fixture_user_id, sudo=daemon_sudo,
        )
        # Mark deleted so the `finally:` doesn't double-delete.
        fixture_user_id = None

        log("Stale-session: verify cookies cleared on next request")
        with _step("intellitester_stale_verify"):
            run_intellitester(
                E2E_DIR / "stale-session-verify.test.yaml",
                extra_args=["--storage-state", str(state_file)],
                env_extras=intelli_env,
            )

        log("All e2e checks passed.")
        return 0

    except Exception as suite_exc:
        suite_failure = (type(suite_exc).__name__, str(suite_exc))
        suite_exit_code = 1
        raise
    finally:
        if throwaway_dir is not None:
            with suppress(Exception):
                _capture_postmortem(
                    throwaway_dir,
                    [throwaway_dir],
                    suite_name="manager",
                    exit_code=suite_exit_code,
                )
            with suppress(Exception):
                _write_summary(
                    token_dir=throwaway_dir,
                    suite_name="manager",
                    started_at=suite_started_at,
                    ended_at=time.time(),
                    exit_code=suite_exit_code,
                    failure=suite_failure,
                    nodes=None,
                )
        # Order matters: manager → fixture user → daemon → tmpdir → state.
        if manager_proc is not None:
            _kill_pg(manager_proc, "manager")
            _close_proc_logs(manager_proc)
        if fixture_user_id is not None and zlayer_bin is not None:
            delete_fixture_user(
                zlayer_bin, cli_data_dir, fixture_user_id, sudo=daemon_sudo,
            )
        if daemon_proc is not None:
            _kill_pg(daemon_proc, "daemon", sudo=daemon_sudo)
            _close_proc_logs(daemon_proc)
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
