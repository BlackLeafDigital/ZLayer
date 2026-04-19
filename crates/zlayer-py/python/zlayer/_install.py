"""
Bootstrap the ``zlayer`` binary from Python.

Provides :func:`ensure_daemon`, which downloads the release binary from
GitHub to a userland location (or escalates to ``zlayer daemon install``
for system-wide placement) so that ``zlayer.Client()`` and friends have a
daemon to talk to without shipping a separate install script.

Stdlib only: ``urllib.request``, ``hashlib``, ``pathlib``, ``subprocess``,
``platform``, ``importlib.metadata``.
"""

from __future__ import annotations

import hashlib
import json
import os
import platform as _platform
import shutil
import ssl
import stat
import subprocess
import sys
import tarfile
import tempfile
import urllib.error
import urllib.request
from importlib import metadata as importlib_metadata
from pathlib import Path
from typing import Optional, Tuple

__all__ = ["ensure_daemon", "ZLayerInstallError"]


# ---------------------------------------------------------------------------
# Constants (kept in sync with install.py at the repo root)
# ---------------------------------------------------------------------------

GITHUB_REPO = "BlackLeafDigital/ZLayer"
GITHUB_API_LATEST = "https://api.github.com/repos/{repo}/releases/latest"
GITHUB_API_TAG = "https://api.github.com/repos/{repo}/releases/tags/{tag}"
GITHUB_DOWNLOAD = "https://github.com/{repo}/releases/download/{tag}/{filename}"

USERLAND_DIR = Path.home() / ".local" / "share" / "zlayer" / "bin"
BINARY_NAME = "zlayer"

_USER_AGENT = "zlayer-py-install/1.0"


class ZLayerInstallError(RuntimeError):
    """Raised when the Python installer cannot produce a usable binary."""


# ---------------------------------------------------------------------------
# Platform detection (mirrors install.py::Platform)
# ---------------------------------------------------------------------------

def _detect_platform() -> Tuple[str, str]:
    """Return ``(os_name, arch)`` tuple for asset naming.

    Raises :class:`ZLayerInstallError` on unsupported systems.
    """
    system = _platform.system().lower()
    if system == "linux":
        os_name = "linux"
    elif system == "darwin":
        os_name = "darwin"
    elif system == "windows":
        raise ZLayerInstallError(
            "Windows is not directly supported by ensure_daemon(). "
            "Use WSL2 or install the Windows build from "
            "https://github.com/{repo}/releases manually.".format(repo=GITHUB_REPO)
        )
    else:
        raise ZLayerInstallError(f"Unsupported operating system: {system}")

    machine = _platform.machine().lower()
    arch_map = {
        "x86_64": "amd64",
        "amd64": "amd64",
        "aarch64": "arm64",
        "arm64": "arm64",
    }
    arch = arch_map.get(machine)
    if arch is None:
        raise ZLayerInstallError(f"Unsupported architecture: {machine}")
    return os_name, arch


def _artifact_suffix() -> str:
    os_name, arch = _detect_platform()
    return f"{os_name}-{arch}"


# ---------------------------------------------------------------------------
# HTTP helpers
# ---------------------------------------------------------------------------

def _urlopen(url: str, timeout: int = 30):
    """Open a URL with a GitHub-friendly UA and TLS verification.

    Falls back to an unverified SSL context only if the default one fails
    (mirrors the behavior of ``install.py`` — see that file for rationale).
    """
    try:
        ctx = ssl.create_default_context()
    except Exception:  # pragma: no cover - exceptional system
        ctx = ssl._create_unverified_context()  # noqa: SLF001

    req = urllib.request.Request(url, headers={"User-Agent": _USER_AGENT})
    try:
        return urllib.request.urlopen(req, timeout=timeout, context=ctx)
    except urllib.error.URLError:
        # Retry with unverified context in case of outdated system CA bundle.
        ctx2 = ssl._create_unverified_context()  # noqa: SLF001
        return urllib.request.urlopen(req, timeout=timeout, context=ctx2)


def _fetch_release(tag: Optional[str]) -> dict:
    """Fetch release metadata from GitHub. ``tag=None`` means 'latest'."""
    if tag is None:
        url = GITHUB_API_LATEST.format(repo=GITHUB_REPO)
    else:
        normalized = tag if tag.startswith("v") else f"v{tag}"
        url = GITHUB_API_TAG.format(repo=GITHUB_REPO, tag=normalized)
    try:
        with _urlopen(url) as resp:
            return json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        raise ZLayerInstallError(
            f"GitHub returned {exc.code} for {url}: {exc.reason}"
        ) from exc
    except Exception as exc:  # pragma: no cover - network error
        raise ZLayerInstallError(f"Failed to fetch release metadata: {exc}") from exc


def _resolve_asset_url(release: dict, version: str) -> str:
    """Pick the download URL for the current platform from release assets.

    Supports both naming conventions used by the release workflow:

    * ``zlayer-{version}-{os}-{arch}.tar.gz`` (current)
    * ``{binary}-{version}-{os}-{arch}.tar.gz`` (multi-binary future)
    """
    suffix = _artifact_suffix()
    candidates = [
        f"{BINARY_NAME}-{version}-{suffix}.tar.gz",
        f"zlayer-{version}-{suffix}.tar.gz",
    ]
    assets = release.get("assets") or []
    for candidate in candidates:
        for asset in assets:
            if asset.get("name") == candidate:
                url = asset.get("browser_download_url")
                if url:
                    return url
    # Fallback: build URL from tag + the first candidate name.
    tag = release.get("tag_name") or (f"v{version}" if version else "")
    if not tag:
        raise ZLayerInstallError(
            "Release metadata missing tag_name; cannot construct download URL."
        )
    return GITHUB_DOWNLOAD.format(repo=GITHUB_REPO, tag=tag, filename=candidates[0])


# ---------------------------------------------------------------------------
# Download + extract
# ---------------------------------------------------------------------------

def _download_and_hash(url: str, dest: Path) -> str:
    """Stream ``url`` to ``dest``, returning the SHA256 hex digest.

    Computing the digest during the download lets us log a verifiable
    fingerprint (the project does not currently publish a SHA256SUMS file;
    callers can cross-check against the GitHub release page or a future
    checksums file).
    """
    hasher = hashlib.sha256()
    try:
        with _urlopen(url, timeout=300) as resp, open(dest, "wb") as fh:
            while True:
                chunk = resp.read(64 * 1024)
                if not chunk:
                    break
                fh.write(chunk)
                hasher.update(chunk)
    except Exception as exc:
        try:
            dest.unlink()
        except OSError:
            pass
        raise ZLayerInstallError(f"Download failed for {url}: {exc}") from exc
    return hasher.hexdigest()


def _safe_extract(tf: tarfile.TarFile, dest_dir: Path) -> None:
    """Extract ``tf`` into ``dest_dir`` with path-traversal protection."""
    if sys.version_info >= (3, 12):
        tf.extractall(dest_dir, filter="data")
        return
    dest_real = os.path.realpath(dest_dir)
    for member in tf.getmembers():
        member_path = os.path.realpath(os.path.join(dest_dir, member.name))
        if not (member_path == dest_real or member_path.startswith(dest_real + os.sep)):
            raise ZLayerInstallError(
                f"Refusing to extract '{member.name}': path traversal detected"
            )
    tf.extractall(dest_dir)


def _extract_binary(archive: Path, extract_dir: Path) -> Path:
    """Extract the ``zlayer`` binary from ``archive`` into ``extract_dir``.

    Returns the path to the extracted binary.
    """
    try:
        with tarfile.open(archive, "r:gz") as tf:
            _safe_extract(tf, extract_dir)
    except tarfile.TarError as exc:
        raise ZLayerInstallError(f"Failed to extract archive: {exc}") from exc

    for root, _dirs, files in os.walk(extract_dir):
        if BINARY_NAME in files:
            return Path(root) / BINARY_NAME
    raise ZLayerInstallError(
        f"Binary '{BINARY_NAME}' not found inside {archive.name}"
    )


# ---------------------------------------------------------------------------
# Version helpers
# ---------------------------------------------------------------------------

def _package_version() -> Optional[str]:
    """Installed ``zlayer`` Python package version, or None if not installed."""
    try:
        return importlib_metadata.version("zlayer")
    except importlib_metadata.PackageNotFoundError:
        return None


def _binary_version(binary: Path) -> Optional[str]:
    """Run ``<binary> --version`` and return the stripped output, or None."""
    try:
        result = subprocess.run(
            [str(binary), "--version"],
            capture_output=True,
            timeout=10,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if result.returncode != 0:
        return None
    text = result.stdout.decode("utf-8", errors="replace").strip()
    return text or None


def _version_matches(current: str, wanted: str) -> bool:
    """Return True if ``wanted`` (bare number, e.g. '0.10.104') appears in
    the ``--version`` output ``current`` (e.g. 'zlayer 0.10.104')."""
    wanted_clean = wanted.lstrip("v").strip()
    return bool(wanted_clean) and wanted_clean in current


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------

def ensure_daemon(
    *,
    system: bool = False,
    version: Optional[str] = None,
    force: bool = False,
) -> Path:
    """Ensure a ``zlayer`` binary is available, downloading it if needed.

    Userland mode (``system=False``, default) downloads a release tarball
    from GitHub into ``~/.local/share/zlayer/bin/`` and returns the path
    to the extracted binary.

    System mode (``system=True``) first ensures the userland binary exists,
    then runs ``zlayer daemon install``; that subcommand requires elevated
    privileges and will prompt for ``sudo`` on the user's terminal. The
    exact command is printed with a warning banner before it runs — this
    function never silently escalates.

    Args:
        system: If True, also run ``zlayer daemon install`` after the
            userland download.
        version: Specific release version to install (e.g. ``"0.10.104"``
            or ``"v0.10.104"``). If ``None``, the installed Python
            package version is preferred; otherwise the latest release.
        force: If True, re-download even when an existing binary's
            ``--version`` output already matches.

    Returns:
        Absolute :class:`~pathlib.Path` to the userland binary.

    Raises:
        ZLayerInstallError: On any failure (HTTP, checksum, filesystem,
            platform detection, ``daemon install`` exit != 0). The
            underlying cause is always reported, never swallowed.
    """
    # --- System mode: bootstrap userland first, then escalate. -----------
    if system:
        binary = ensure_daemon(system=False, version=version, force=force)
        _run_daemon_install(binary)
        return binary

    # --- Userland mode ---------------------------------------------------
    USERLAND_DIR.mkdir(parents=True, exist_ok=True)
    dest = USERLAND_DIR / BINARY_NAME

    # Pick target version: explicit arg > installed pkg version > "latest".
    requested = version if version is not None else _package_version()

    # Short-circuit when an existing binary matches the target version.
    if not force and dest.exists() and os.access(dest, os.X_OK):
        current = _binary_version(dest)
        if current is not None:
            if requested is None:
                # No specific target: any working binary is good enough.
                return dest
            if _version_matches(current, requested):
                return dest

    # Fetch release metadata.
    tag_arg = requested  # None -> latest
    release = _fetch_release(tag_arg)
    tag = release.get("tag_name") or ""
    if not tag:
        raise ZLayerInstallError(
            "GitHub release response missing 'tag_name'; cannot proceed."
        )
    release_version = tag.lstrip("v")

    asset_url = _resolve_asset_url(release, release_version)

    # Stream to a temp dir so a partial failure never pollutes USERLAND_DIR.
    with tempfile.TemporaryDirectory(prefix="zlayer-py-install-") as tmp_str:
        tmp = Path(tmp_str)
        archive = tmp / Path(asset_url).name
        digest = _download_and_hash(asset_url, archive)

        if archive.stat().st_size < 1024:
            raise ZLayerInstallError(
                f"Downloaded archive is implausibly small "
                f"({archive.stat().st_size} bytes); aborting."
            )

        # No published SHA256SUMS yet — log the digest so the caller can
        # cross-reference against the GitHub release page or a future
        # checksum file. (Raising would break every user today.)
        print(
            f"[zlayer] downloaded {archive.name} "
            f"({archive.stat().st_size} bytes, sha256={digest})",
            file=sys.stderr,
        )

        extract_dir = tmp / "extract"
        extract_dir.mkdir()
        extracted = _extract_binary(archive, extract_dir)

        # Atomic-ish move: copy to a temp path in USERLAND_DIR then rename.
        staging = dest.with_name(BINARY_NAME + ".new")
        try:
            if staging.exists():
                staging.unlink()
            shutil.copy2(extracted, staging)
            staging.chmod(
                stat.S_IRWXU | stat.S_IRGRP | stat.S_IXGRP | stat.S_IROTH | stat.S_IXOTH
            )
            os.replace(staging, dest)
        except OSError as exc:
            raise ZLayerInstallError(
                f"Failed to install binary to {dest}: {exc}"
            ) from exc
        finally:
            if staging.exists():
                try:
                    staging.unlink()
                except OSError:
                    pass

    # Sanity check: the binary we just installed should be runnable.
    final_version = _binary_version(dest)
    if final_version is None:
        raise ZLayerInstallError(
            f"Installed {dest} but '{dest} --version' failed; "
            "binary may be corrupt or incompatible."
        )

    return dest


def _run_daemon_install(binary: Path) -> None:
    """Invoke ``<binary> daemon install`` with a clear privilege warning.

    ``zlayer daemon install`` is what actually escalates via ``sudo`` (or
    equivalent) under the hood. We do not wrap it in ``sudo`` ourselves —
    the binary handles privilege elevation internally and displays its own
    prompt, so the user's credentials stay under the control of the CLI
    they invoked, not this Python wrapper.
    """
    cmd = [str(binary), "daemon", "install"]
    banner = (
        "\n"
        "================================================================\n"
        "  About to run: sudo " + " ".join(cmd) + "\n"
        "  This command requires administrator privileges to install\n"
        "  the zlayer daemon system-wide (services, unit files, etc.).\n"
        "  Your terminal will prompt for your sudo password.\n"
        "================================================================\n"
    )
    print(banner, file=sys.stderr, flush=True)

    try:
        subprocess.run(cmd, check=True)
    except subprocess.CalledProcessError as exc:
        raise ZLayerInstallError(
            f"'zlayer daemon install' failed with exit code {exc.returncode}"
        ) from exc
    except FileNotFoundError as exc:
        raise ZLayerInstallError(
            f"Could not execute {binary}: {exc}"
        ) from exc
