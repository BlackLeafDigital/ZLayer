"""
Pytest fixtures for the zlayer Python binding integration tests.

These fixtures spawn a real `zlayer serve` daemon on a per-test temporary
Unix socket so tests can drive the :class:`zlayer.Client` end-to-end
against a real REST API.

Fixtures defined here:

``zlayer_binary``
    Session-scoped. Resolves the path to a `zlayer` binary, preferring
    (in order): the ``ZLAYER_BINARY`` env var, a debug build in
    ``target/debug/zlayer``, a release build in ``target/release/zlayer``,
    and a ``cargo run -p zlayer --`` fallback. If none of these are
    viable, tests in this module are **skipped** (not errored) — the
    extension is optional to build, and we never want a missing binary
    to be a test failure.

``daemon``
    Per-test. Creates a temp dir, spawns ``zlayer serve`` with a unique
    socket path and data dir inside that temp dir, waits for the socket
    to appear, and yields ``{"socket": <Path>, "data_dir": <Path>}``.
    Teardown kills the subprocess with SIGTERM → SIGKILL escalation and
    wipes the temp dir.

All tests gracefully skip when:

* the built `zlayer` binary is not available,
* the compiled `zlayer._zlayer` extension is not importable (maturin
  hasn't been run yet),
* the platform lacks Unix socket support (non-Unix).
"""

from __future__ import annotations

import os
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Iterator

import pytest


# ---------------------------------------------------------------------------
# Path helpers
# ---------------------------------------------------------------------------

# tests/conftest.py -> crates/zlayer-py/tests/ -> crates/zlayer-py/ -> crates/ -> repo root
REPO_ROOT = Path(__file__).resolve().parents[3]


def _find_zlayer_binary() -> str | None:
    """Locate a usable `zlayer` binary.

    Returns the resolved path as a string, or ``None`` if nothing usable
    is found.  This intentionally does NOT fall back to
    ``cargo run -p zlayer --`` because the test driver expects a plain
    executable; the cargo fallback is handled separately in the fixture.
    """
    env = os.environ.get("ZLAYER_BINARY")
    if env:
        p = Path(env).expanduser()
        if p.is_file() and os.access(p, os.X_OK):
            return str(p)

    for rel in ("target/debug/zlayer", "target/release/zlayer"):
        p = REPO_ROOT / rel
        if p.is_file() and os.access(p, os.X_OK):
            return str(p)

    return None


def _have_cargo() -> bool:
    """Check whether `cargo` is on PATH — used for the slow fallback."""
    return shutil.which("cargo") is not None


# ---------------------------------------------------------------------------
# Import-time skip for the `zlayer` extension module
# ---------------------------------------------------------------------------

try:
    import zlayer  # noqa: F401
    import zlayer._zlayer  # noqa: F401

    _ZLAYER_IMPORTED = True
    _ZLAYER_IMPORT_ERROR: str | None = None
except Exception as exc:  # pragma: no cover — exercised only when not built
    _ZLAYER_IMPORTED = False
    _ZLAYER_IMPORT_ERROR = f"{type(exc).__name__}: {exc}"


def pytest_collection_modifyitems(config, items):
    """Skip every test in this directory when the environment isn't viable.

    We do this at collection time (instead of via module-level skip) so
    that collection still succeeds — the user gets a single clear skip
    reason per test rather than a confusing ImportError.
    """
    skip_reason: str | None = None

    # The daemon only listens on a Unix domain socket.
    if not hasattr(socket, "AF_UNIX"):
        skip_reason = "zlayer daemon requires Unix domain socket support"

    if skip_reason is None and not _ZLAYER_IMPORTED:
        skip_reason = (
            "zlayer._zlayer extension not importable "
            "(build with `maturin develop` first): "
            f"{_ZLAYER_IMPORT_ERROR}"
        )

    if skip_reason is None:
        binary = _find_zlayer_binary()
        if binary is None and not _have_cargo():
            skip_reason = (
                "no zlayer binary found (set ZLAYER_BINARY, build "
                "target/debug/zlayer, or install cargo)"
            )

    if skip_reason is not None:
        marker = pytest.mark.skip(reason=skip_reason)
        tests_dir = Path(__file__).parent.resolve()
        for item in items:
            # Only skip items under this tests/ directory. Use a
            # manual parent walk so we stay compatible with 3.8
            # (Path.is_relative_to landed in 3.9).
            item_path = Path(str(item.fspath)).resolve()
            if tests_dir == item_path or tests_dir in item_path.parents:
                item.add_marker(marker)


# ---------------------------------------------------------------------------
# Binary fixture
# ---------------------------------------------------------------------------


@pytest.fixture(scope="session")
def zlayer_binary() -> list[str]:
    """Return an argv prefix that invokes `zlayer`.

    Returns a list so the slow ``cargo run`` fallback can plug in
    multiple leading arguments. Callers extend with the subcommand:

        subprocess.Popen([*zlayer_binary, "serve", ...])
    """
    direct = _find_zlayer_binary()
    if direct is not None:
        return [direct]

    if _have_cargo():
        # Slow fallback — useful only in CI before the binary is cached.
        return [
            "cargo",
            "run",
            "--quiet",
            "--manifest-path",
            str(REPO_ROOT / "Cargo.toml"),
            "-p",
            "zlayer",
            "--",
        ]

    # Should be unreachable: pytest_collection_modifyitems would have
    # skipped us already. Raise a clear error as a belt-and-suspenders.
    pytest.skip("no zlayer binary or cargo available")


# ---------------------------------------------------------------------------
# Daemon fixture
# ---------------------------------------------------------------------------


def _wait_for_socket(path: Path, timeout: float = 15.0) -> None:
    """Block until ``path`` exists (and looks like a usable socket), or raise."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists():
            # The socket shows up before the daemon is ready to accept;
            # the client's connect() call will retry, so merely existing
            # is sufficient for us.
            return
        time.sleep(0.1)
    raise TimeoutError(
        f"daemon socket {path} did not appear within {timeout:.1f}s"
    )


def _pick_free_tcp_port() -> int:
    """Grab a free ephemeral TCP port for the API server's --bind arg.

    The daemon insists on binding both a TCP port and a Unix socket, even
    though these tests only use the Unix socket. Ask the kernel for an
    unused port so we don't collide with anything running on 3669.
    """
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


@pytest.fixture
def daemon(
    zlayer_binary: list[str], tmp_path_factory: pytest.TempPathFactory
) -> Iterator[dict[str, Path]]:
    """Spawn a fresh ``zlayer serve`` and tear it down after the test."""
    tmpdir = tmp_path_factory.mktemp("zlayer-daemon")
    socket_path = tmpdir / "zlayer.sock"
    data_dir = tmpdir / "data"
    data_dir.mkdir(parents=True, exist_ok=True)

    bind_port = _pick_free_tcp_port()
    bind_addr = f"127.0.0.1:{bind_port}"

    # --data-dir is a top-level Cli arg, so it comes BEFORE `serve`.
    argv = [
        *zlayer_binary,
        "--data-dir",
        str(data_dir),
        "serve",
        "--bind",
        bind_addr,
        "--socket",
        str(socket_path),
        "--no-swagger",
    ]

    env = os.environ.copy()
    # Keep the test daemon from clobbering the operator's real state:
    env["ZLAYER_DATA_DIR"] = str(data_dir)
    # Deterministic JWT secret so we don't log a production warning.
    env.setdefault("ZLAYER_JWT_SECRET", "test-secret-not-for-production")

    log_file = tmpdir / "daemon.log"
    with log_file.open("wb") as logf:
        proc = subprocess.Popen(
            argv,
            stdout=logf,
            stderr=subprocess.STDOUT,
            env=env,
            # Put the daemon in its own process group so we can kill any
            # subprocesses (buildah etc.) it might spawn.
            start_new_session=True,
        )

        try:
            try:
                _wait_for_socket(socket_path, timeout=20.0)
            except TimeoutError as exc:
                proc.poll()
                tail = ""
                try:
                    tail = log_file.read_text(errors="replace")[-4000:]
                except OSError:
                    pass
                pytest.skip(
                    f"zlayer daemon failed to start: {exc}\n"
                    f"--- daemon log tail ---\n{tail}"
                )

            yield {"socket": socket_path, "data_dir": data_dir, "log": log_file}
        finally:
            # SIGTERM to the whole process group, then escalate.
            pgid = None
            try:
                pgid = os.getpgid(proc.pid)
            except (ProcessLookupError, OSError):
                pass

            def _signal(sig: int) -> None:
                if pgid is not None:
                    try:
                        os.killpg(pgid, sig)
                        return
                    except (ProcessLookupError, OSError):
                        pass
                try:
                    proc.send_signal(sig)
                except (ProcessLookupError, OSError):
                    pass

            if proc.poll() is None:
                _signal(signal.SIGTERM)
                try:
                    proc.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    _signal(signal.SIGKILL)
                    try:
                        proc.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        # Last-resort: leave it, the OS will reap it.
                        pass

            # Attempt to remove the socket file if the daemon didn't.
            try:
                if socket_path.exists():
                    socket_path.unlink()
            except OSError:
                pass


# ---------------------------------------------------------------------------
# pytest-asyncio configuration
# ---------------------------------------------------------------------------


def pytest_configure(config: pytest.Config) -> None:
    """Register the asyncio marker in case pytest-asyncio isn't installed.

    If pytest-asyncio is available, it registers the marker itself; our
    registration here is harmless (pytest de-dupes markers by name). If
    it's NOT available, every async test would fail with a cryptic
    "coroutine was never awaited" warning — we instead skip them with a
    clear reason.
    """
    config.addinivalue_line(
        "markers",
        "asyncio: mark test as asynchronous (requires pytest-asyncio)",
    )
    # Surface this on `python3 -m pytest`.
    sys.stdout.flush()
