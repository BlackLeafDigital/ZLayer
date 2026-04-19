#!/usr/bin/env python3
"""Deploy a trivial service to ZLayer from Python, end-to-end.

This example demonstrates the two core pieces of the Python SDK:

* ``zlayer.ensure_daemon()`` bootstraps the ``zlayer`` binary into
  ``~/.local/share/zlayer/bin/`` (userland install) so you don't need a
  separate install script. Pass ``system=True`` to also run
  ``sudo zlayer daemon install`` for a system-wide service install.
* ``zlayer.Client()`` connects to a running ``zlayer serve`` daemon over
  its Unix socket and exposes ``deploy``/``ps``/``logs``/``exec``/
  ``status``/``scale``/``stop`` as ``async`` methods.

Run it with either:

    uv run --with zlayer examples/python/deploy_hello.py
    # or:
    pip install zlayer && python examples/python/deploy_hello.py

Prerequisite: a daemon must already be running. Start one with:

    zlayer serve --daemon

(``ensure_daemon()`` installs the binary but does not start the daemon.)
"""

from __future__ import annotations

import asyncio
import sys
from pathlib import Path

import zlayer

SPEC_PATH = Path(__file__).parent / "hello.zlayer.yaml"


async def main() -> int:
    # 1. Ensure the zlayer binary is available on disk (userland install).
    #    Pass system=True to also run `sudo zlayer daemon install`.
    binary = zlayer.ensure_daemon()
    print(f"[zlayer] binary: {binary}")
    print("[zlayer] this example assumes a daemon is already running.")
    print("[zlayer] start one with: zlayer serve --daemon")

    # 2. Connect to the running daemon (default Unix socket path).
    client = zlayer.Client()
    print(f"[zlayer] connected: {client!r}")

    # 3. Deploy the spec.
    spec_yaml = SPEC_PATH.read_text()
    result = await client.deploy(spec_yaml)
    print(f"[zlayer] deployed: {result}")

    # 4. Show what's running.
    containers = await client.ps()
    print(f"[zlayer] containers ({len(containers) if containers else 0}):")
    for c in containers or []:
        print(f"  - {c}")

    # 5. Inspect the deployment's status record.
    status = await client.status("hello")
    print(f"[zlayer] status: {status}")

    # 6. Uncomment to clean up after a short pause:
    # await asyncio.sleep(2)
    # await client.stop("hello")
    # print("[zlayer] stopped deployment 'hello'")

    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
