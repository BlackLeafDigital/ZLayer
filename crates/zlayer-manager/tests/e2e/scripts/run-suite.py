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

def build_phase(*, throwaway: bool) -> None:
    on_path = shutil.which("zlayer") is not None
    if throwaway or not on_path:
        log("Building zlayer (release)")
        subprocess.run(
            ["cargo", "build", "--release", "-p", "zlayer"],
            cwd=REPO_ROOT, check=True,
        )
    else:
        log(f"Skipping zlayer build (using {shutil.which('zlayer')})")
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

    healthz = f"http://127.0.0.1:{API_PORT}/healthz"
    root_url = f"http://127.0.0.1:{API_PORT}/"
    if not (
        _wait_http_ok(healthz, HEALTHCHECK_TIMEOUT_S)
        or _wait_http_ok(root_url, 5)
    ):
        _kill_pg(proc, "daemon (failed startup)", sudo=sudo)
        die(f"throwaway daemon never came up on 127.0.0.1:{API_PORT}")

    sock = data_dir / "run" / "zlayer.sock"
    deadline = time.monotonic() + SOCKET_WAIT_S
    while time.monotonic() < deadline:
        if is_socket(sock):
            return proc, sock
        time.sleep(1)
    _kill_pg(proc, "daemon (no socket)", sudo=sudo)
    die(f"throwaway daemon socket never appeared at {sock}")
    raise RuntimeError("unreachable")  # for type checkers


def create_fixture_user(
    zlayer_bin: Path, cli_data_dir: Optional[Path],
    email: str, password: str, display: str,
) -> str:
    log(f"Creating fixture user {email} (admin)")
    env = {**os.environ}
    if cli_data_dir:
        env["ZLAYER_DATA_DIR"] = str(cli_data_dir)
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
        ),
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
    return parser.parse_args()


def main() -> int:
    args = parse_args()

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
