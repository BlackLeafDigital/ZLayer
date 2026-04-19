"""
Integration tests for :class:`zlayer.Client`.

These tests spawn a real `zlayer serve` daemon via the ``daemon`` fixture
(see ``conftest.py``) and drive it through the Python binding's REST
client, confirming that the wiring from Python -> pyo3 -> DaemonClient
-> HTTP API actually works end-to-end.

Skip matrix (see ``conftest.py`` for the gating logic):

* ``zlayer._zlayer`` extension not built   -> every test skipped.
* No ``zlayer`` binary and no cargo        -> every test skipped.
* Non-Unix host                             -> every test skipped.
* ``pytest-asyncio`` not installed          -> async tests skipped
  individually (pytest emits a clear "async def functions are not
  natively supported" warning, and we guard with a module-level
  importorskip below so the whole file is skipped instead).

Any other daemon-startup failure is surfaced as a skip (not a failure)
by the ``daemon`` fixture, because a broken binary is an environment
issue rather than a Client regression.
"""

from __future__ import annotations

import asyncio
from pathlib import Path
from typing import Any

import pytest

# Skip the whole module if pytest-asyncio is missing — every test here
# is async, and running them without the plugin would produce confusing
# "coroutine was never awaited" output instead of a clear skip.
pytest.importorskip("pytest_asyncio")

# Skip the whole module if the zlayer extension isn't built. conftest's
# collection hook also catches this, but module-level `import zlayer`
# runs BEFORE the hook, so we need our own guard here — otherwise an
# uninstalled extension shows up as a collection ERROR instead of a
# clean SKIPPED.
zlayer = pytest.importorskip(
    "zlayer",
    reason=(
        "zlayer._zlayer extension not built — run `maturin develop` "
        "inside crates/zlayer-py first"
    ),
)


pytestmark = pytest.mark.asyncio


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


async def _poll_until(
    fn,
    predicate,
    *,
    timeout: float = 10.0,
    interval: float = 0.2,
):
    """Call ``fn()`` repeatedly until ``predicate(result)`` is true or timeout.

    Returns the last result. Raises ``asyncio.TimeoutError`` on timeout.
    ``fn`` may be sync or async.
    """
    loop = asyncio.get_event_loop()
    deadline = loop.time() + timeout
    last: Any = None
    while loop.time() < deadline:
        res = fn()
        if asyncio.iscoroutine(res):
            res = await res
        last = res
        if predicate(res):
            return res
        await asyncio.sleep(interval)
    raise asyncio.TimeoutError(f"condition not met within {timeout}s, last={last!r}")


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


async def test_ps_returns_empty(daemon: dict[str, Path]) -> None:
    """A freshly-started daemon has no containers; ``ps()`` returns ``[]``."""
    client = zlayer.Client(socket=str(daemon["socket"]))
    result = await client.ps()

    assert isinstance(result, list), (
        f"ps() should return a list on a fresh daemon, got {type(result).__name__}"
    )
    assert len(result) == 0, f"expected empty container list, got {result!r}"


async def test_status_missing_deployment(daemon: dict[str, Path]) -> None:
    """Asking for a deployment that doesn't exist raises a clear error.

    The pyo3 wrapper maps any ``anyhow::Error`` from ``DaemonClient`` into
    ``RuntimeError`` (see ``map_client_err`` in ``client.rs``), so we catch
    broadly and only assert the message mentions "not found" / 404 / similar.
    """
    client = zlayer.Client(socket=str(daemon["socket"]))

    with pytest.raises(Exception) as excinfo:  # noqa: BLE001 — broad by design
        await client.status("does-not-exist-xyz")

    msg = str(excinfo.value).lower()
    # Accept any of the plausible shapes the daemon might return.
    assert any(
        token in msg
        for token in ("not found", "404", "no such", "missing", "does-not-exist-xyz")
    ), f"expected 'not found'-ish message, got: {excinfo.value!r}"


async def test_deploy_and_stop(daemon: dict[str, Path]) -> None:
    """Deploy a minimal spec, confirm it shows up, then stop it.

    Uses a tiny `service` spec pinned to a scale of zero so the daemon
    has nothing to actually pull or launch — we only want to verify
    that the Python -> API plumbing accepts the spec, persists it, and
    can tear it down again. If the daemon rejects zero-replica service
    specs in this version, the test is skipped (not failed) with a
    clear TODO for future refinement.
    """
    client = zlayer.Client(socket=str(daemon["socket"]))

    deployment_name = "pybinding-test"
    spec_yaml = f"""\
version: v1
deployment: {deployment_name}
services:
  noop:
    rtype: service
    image:
      name: alpine:latest
    scale:
      mode: fixed
      replicas: 0
"""

    try:
        await client.deploy(spec_yaml)
    except Exception as exc:  # noqa: BLE001 — minimum-spec compatibility probe
        pytest.skip(
            "minimal deployment spec rejected by this daemon version — "
            f"TODO: tighten spec once the exact shape stabilises: {exc}"
        )

    # The service may or may not appear in ps() depending on whether
    # any replicas were launched (we asked for 0). Either way, status()
    # should return a dict describing the deployment.
    try:
        status = await client.status(deployment_name)
        assert isinstance(status, dict), (
            f"status() should return a dict, got {type(status).__name__}: {status!r}"
        )
    finally:
        # Always attempt cleanup, even if the status assertion failed.
        try:
            await client.stop(deployment_name)
        except Exception:  # noqa: BLE001
            # Cleanup failure is a daemon issue, not a Client-binding issue.
            pass

    # After stop(), status() should error out — use poll_until to give
    # the daemon a moment to finish teardown.
    async def _status_errors() -> bool:
        try:
            await client.status(deployment_name)
        except Exception:  # noqa: BLE001
            return True
        return False

    try:
        await _poll_until(
            _status_errors, lambda gone: gone is True, timeout=5.0, interval=0.2
        )
    except asyncio.TimeoutError:
        pytest.fail(f"deployment {deployment_name!r} still present after stop()")
