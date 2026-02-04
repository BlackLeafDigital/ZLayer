#!/usr/bin/env python3
"""
ZLayer Interactive Installer

A beautiful, interactive installer for the ZLayer container orchestration platform.
Supports installing zlayer (runtime), zlayer-build (builder), and zlayer-cli (TUI).

Usage:
    python3 install.py
    python3 install.py --yes --binary zlayer-build --dir /usr/local/bin
    curl -sSL https://zlayer.dev/install.py | python3

Requirements:
    Python 3.8+ (stdlib only, no external dependencies)
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import signal
import stat
import subprocess
import sys
import tarfile
import tempfile
import textwrap
import threading
import time

# typing imports compatible with 3.8
from typing import Any, Dict, List, Optional, Tuple

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

GITHUB_REPO = "zachhandley/ZLayer"
GITHUB_API_LATEST = "https://api.github.com/repos/{repo}/releases/latest"
GITHUB_DOWNLOAD = (
    "https://github.com/{repo}/releases/download/{tag}/{filename}"
)

BINARIES = {
    "zlayer-build": {
        "description": "Lightweight image builder",
        "size_hint": "~10 MB",
        "default": True,
    },
    "zlayer-cli": {
        "description": "Interactive TUI",
        "size_hint": "~12 MB",
        "default": False,
    },
    "zlayer": {
        "description": "Full runtime with orchestration",
        "size_hint": "~50 MB",
        "default": False,
    },
}

BINARY_ORDER = ["zlayer-build", "zlayer-cli", "zlayer"]

INSTALL_LOCATIONS = [
    ("/usr/local/bin", "System-wide (may require sudo)"),
    ("~/.local/bin", "User-local (~/.local/bin)"),
]

SHELL_RC_FILES = {
    "bash": [".bashrc", ".bash_profile"],
    "zsh": [".zshrc"],
    "fish": [".config/fish/config.fish"],
}

VERSION = "1.0.0"


# ---------------------------------------------------------------------------
# Terminal color / style helpers
# ---------------------------------------------------------------------------

class Style:
    """ANSI escape code manager with automatic capability detection."""

    def __init__(self) -> None:
        self._enabled = self._detect_color_support()

    @staticmethod
    def _detect_color_support() -> bool:
        """Determine whether the terminal supports ANSI colors."""
        if os.environ.get("NO_COLOR"):
            return False
        if os.environ.get("FORCE_COLOR"):
            return True
        if not hasattr(sys.stdout, "isatty") or not sys.stdout.isatty():
            return False
        term = os.environ.get("TERM", "")
        if term == "dumb":
            return False
        return True

    @property
    def enabled(self) -> bool:
        return self._enabled

    @enabled.setter
    def enabled(self, value: bool) -> None:
        self._enabled = value

    def _code(self, code: str) -> str:
        return code if self._enabled else ""

    # Reset
    @property
    def reset(self) -> str:
        return self._code("\033[0m")

    # Regular colours
    @property
    def red(self) -> str:
        return self._code("\033[31m")

    @property
    def green(self) -> str:
        return self._code("\033[32m")

    @property
    def yellow(self) -> str:
        return self._code("\033[33m")

    @property
    def blue(self) -> str:
        return self._code("\033[34m")

    @property
    def magenta(self) -> str:
        return self._code("\033[35m")

    @property
    def cyan(self) -> str:
        return self._code("\033[36m")

    @property
    def white(self) -> str:
        return self._code("\033[37m")

    @property
    def gray(self) -> str:
        return self._code("\033[90m")

    # Bright / bold
    @property
    def bold(self) -> str:
        return self._code("\033[1m")

    @property
    def dim(self) -> str:
        return self._code("\033[2m")

    @property
    def italic(self) -> str:
        return self._code("\033[3m")

    @property
    def underline(self) -> str:
        return self._code("\033[4m")

    # Cursor helpers
    @property
    def hide_cursor(self) -> str:
        return self._code("\033[?25l")

    @property
    def show_cursor(self) -> str:
        return self._code("\033[?25h")

    def move_up(self, n: int = 1) -> str:
        return self._code("\033[{}A".format(n))

    def move_down(self, n: int = 1) -> str:
        return self._code("\033[{}B".format(n))

    @property
    def clear_line(self) -> str:
        return self._code("\033[2K\r")

    def clear_lines(self, n: int) -> str:
        if not self._enabled:
            return ""
        return "".join("\033[2K\033[1A" for _ in range(n)) + "\033[2K\r"


S = Style()


# ---------------------------------------------------------------------------
# Unicode helpers (graceful ASCII fallback)
# ---------------------------------------------------------------------------

def _supports_unicode() -> bool:
    """Check if the terminal likely supports unicode box-drawing characters."""
    if os.environ.get("ZLAYER_ASCII"):
        return False
    try:
        encoding = sys.stdout.encoding or ""
        return encoding.lower().startswith("utf")
    except Exception:
        return False


_UNICODE = _supports_unicode()

# Box-drawing characters with ASCII fallback
BOX_TL = "\u250c" if _UNICODE else "+"
BOX_TR = "\u2510" if _UNICODE else "+"
BOX_BL = "\u2514" if _UNICODE else "+"
BOX_BR = "\u2518" if _UNICODE else "+"
BOX_H = "\u2500" if _UNICODE else "-"
BOX_V = "\u2502" if _UNICODE else "|"
CHECK = "\u2714" if _UNICODE else "[x]"
CROSS = "\u2718" if _UNICODE else "[!]"
BULLET = "\u2022" if _UNICODE else "*"
ARROW_R = "\u25b6" if _UNICODE else ">"
SPINNER_FRAMES = (
    ["\u280b", "\u2819", "\u2839", "\u2838", "\u283c", "\u2834", "\u2826", "\u2827", "\u2807", "\u280f"]
    if _UNICODE
    else ["|", "/", "-", "\\"]
)
BLOCK_FULL = "\u2588" if _UNICODE else "#"
BLOCK_EMPTY = "\u2591" if _UNICODE else "."


# ---------------------------------------------------------------------------
# Output helpers
# ---------------------------------------------------------------------------

def write(text: str = "") -> None:
    """Write a line to stdout."""
    sys.stdout.write(text + "\n")
    sys.stdout.flush()


def write_raw(text: str) -> None:
    """Write raw text without newline."""
    sys.stdout.write(text)
    sys.stdout.flush()


def info(msg: str) -> None:
    write("  {}{}{}  {}".format(S.blue, BULLET, S.reset, msg))


def success(msg: str) -> None:
    write("  {}{}{}  {}".format(S.green, CHECK, S.reset, msg))


def warn(msg: str) -> None:
    write("  {}{}{}  {}{}{}".format(S.yellow, CROSS, S.reset, S.yellow, msg, S.reset))


def error(msg: str) -> None:
    write("  {}{}  {}{}".format(S.red, CROSS, msg, S.reset))


def fatal(msg: str) -> None:
    """Print error and exit."""
    write()
    error(msg)
    write()
    sys.exit(1)


def draw_box(lines: List[str], width: int = 45, color: str = "") -> None:
    """Draw a unicode box around the given lines, centered."""
    c = color or S.cyan
    inner = width - 2
    write("  {}{}{}{}{}{}".format(c, BOX_TL, BOX_H * inner, BOX_TR, S.reset, ""))
    for line in lines:
        # Center the text inside the box
        stripped = line
        padding = inner - len(stripped)
        left_pad = padding // 2
        right_pad = padding - left_pad
        write("  {}{}{} {}{}{}{}{} {}{}{}".format(
            c, BOX_V, S.reset,
            " " * (left_pad - 1),
            S.bold, stripped, S.reset,
            " " * (right_pad - 1),
            c, BOX_V, S.reset,
        ))
    write("  {}{}{}{}{}{}".format(c, BOX_BL, BOX_H * inner, BOX_BR, S.reset, ""))


# ---------------------------------------------------------------------------
# Spinner (threaded background animation)
# ---------------------------------------------------------------------------

class Spinner:
    """Animated spinner for long-running operations."""

    def __init__(self, message: str = "") -> None:
        self._message = message
        self._stop_event = threading.Event()
        self._thread = None  # type: Optional[threading.Thread]

    def start(self) -> "Spinner":
        self._stop_event.clear()
        self._thread = threading.Thread(target=self._animate, daemon=True)
        self._thread.start()
        return self

    def stop(self, final: str = "") -> None:
        self._stop_event.set()
        if self._thread is not None:
            self._thread.join(timeout=2.0)
        write_raw(S.clear_line)
        if final:
            write(final)

    def _animate(self) -> None:
        idx = 0
        while not self._stop_event.is_set():
            frame = SPINNER_FRAMES[idx % len(SPINNER_FRAMES)]
            write_raw("{}  {}{}{} {}".format(S.clear_line, S.cyan, frame, S.reset, self._message))
            idx += 1
            self._stop_event.wait(0.08)

    def __enter__(self) -> "Spinner":
        return self.start()

    def __exit__(self, *args: Any) -> None:
        self.stop()


# ---------------------------------------------------------------------------
# Progress bar
# ---------------------------------------------------------------------------

class ProgressBar:
    """Download progress bar with speed and ETA."""

    def __init__(self, total: int, label: str = "", width: int = 30) -> None:
        self.total = total
        self.label = label
        self.width = width
        self.current = 0
        self._start_time = time.time()

    def update(self, amount: int) -> None:
        self.current += amount
        self._draw()

    def _draw(self) -> None:
        if self.total <= 0:
            return
        frac = min(self.current / self.total, 1.0)
        filled = int(self.width * frac)
        bar = BLOCK_FULL * filled + BLOCK_EMPTY * (self.width - filled)
        pct = int(frac * 100)

        elapsed = time.time() - self._start_time
        if elapsed > 0 and self.current > 0:
            speed = self.current / elapsed
            speed_str = _format_bytes(int(speed)) + "/s"
        else:
            speed_str = "---"

        current_str = _format_bytes(self.current)
        total_str = _format_bytes(self.total)

        write_raw("{}  {}{}{} {}{:3d}%{}  {}{} / {}{}  {}{}{}".format(
            S.clear_line,
            S.cyan, bar, S.reset,
            S.bold, pct, S.reset,
            S.gray, current_str, total_str, S.reset,
            S.dim, speed_str, S.reset,
        ))

    def finish(self) -> None:
        self.current = self.total
        self._draw()
        write("")


def _format_bytes(n: int) -> str:
    """Human-readable byte size."""
    if n < 1024:
        return "{} B".format(n)
    elif n < 1024 * 1024:
        return "{:.1f} KB".format(n / 1024.0)
    elif n < 1024 * 1024 * 1024:
        return "{:.1f} MB".format(n / (1024.0 * 1024))
    else:
        return "{:.1f} GB".format(n / (1024.0 * 1024 * 1024))


# ---------------------------------------------------------------------------
# Interactive selection widgets (raw terminal input)
# ---------------------------------------------------------------------------

def _enable_raw_mode() -> Any:
    """Enable raw terminal mode, return old settings for restoration."""
    try:
        import termios
        import tty
        fd = sys.stdin.fileno()
        old = termios.tcgetattr(fd)
        tty.setcbreak(fd)
        return old
    except (ImportError, Exception):
        return None


def _restore_mode(old: Any) -> None:
    """Restore terminal settings."""
    if old is None:
        return
    try:
        import termios
        fd = sys.stdin.fileno()
        termios.tcsetattr(fd, termios.TCSADRAIN, old)
    except (ImportError, Exception):
        pass


def _read_key() -> str:
    """Read a single keypress. Returns special names for arrow keys."""
    try:
        ch = sys.stdin.read(1)
        if ch == "\x1b":
            seq = sys.stdin.read(2)
            if seq == "[A":
                return "UP"
            elif seq == "[B":
                return "DOWN"
            elif seq == "[C":
                return "RIGHT"
            elif seq == "[D":
                return "LEFT"
            return "ESC"
        elif ch == "\r" or ch == "\n":
            return "ENTER"
        elif ch == " ":
            return "SPACE"
        elif ch == "\x03":
            return "CTRL_C"
        elif ch == "q" or ch == "Q":
            return "Q"
        return ch
    except Exception:
        return "ENTER"


def select_binaries(available: List[str], defaults: List[bool]) -> List[str]:
    """Interactive checkbox selector for binaries.

    Returns list of selected binary names.
    """
    selected = list(defaults)
    cursor = 0
    n_items = len(available)

    old_settings = _enable_raw_mode()
    write_raw(S.hide_cursor)

    try:
        # Initial render
        _draw_checkbox(available, selected, cursor)

        while True:
            key = _read_key()
            if key == "CTRL_C" or key == "Q":
                write_raw(S.show_cursor)
                _restore_mode(old_settings)
                write()
                fatal("Installation cancelled.")
            elif key == "UP":
                cursor = (cursor - 1) % n_items
            elif key == "DOWN":
                cursor = (cursor + 1) % n_items
            elif key == "SPACE":
                selected[cursor] = not selected[cursor]
            elif key == "ENTER":
                break
            else:
                continue

            # Redraw
            write_raw(S.move_up(n_items + 2))  # items + hint + blank
            _draw_checkbox(available, selected, cursor)

    finally:
        write_raw(S.show_cursor)
        _restore_mode(old_settings)

    return [name for name, sel in zip(available, selected) if sel]


def _draw_checkbox(names: List[str], selected: List[bool], cursor: int) -> None:
    """Render the checkbox list."""
    for i, name in enumerate(names):
        meta = BINARIES.get(name, {})
        desc = meta.get("description", "")
        size = meta.get("size_hint", "")

        if selected[i]:
            box = "{}[x]{}".format(S.green, S.reset)
        else:
            box = "{}[ ]{}".format(S.gray, S.reset)

        pointer = "{}{}{}".format(S.cyan, ARROW_R, S.reset) if i == cursor else " "

        line = "  {} {} {}{}{} {}({}){}  {}{}{}".format(
            pointer, box,
            S.bold, name, S.reset,
            S.gray, desc, S.reset,
            S.dim, size, S.reset,
        )
        write(line)

    write()
    write("  {}Use {}\u2191\u2193{} to move, {}Space{} to toggle, {}Enter{} to confirm{}".format(
        S.dim,
        S.reset + S.bold, S.reset + S.dim,
        S.reset + S.bold, S.reset + S.dim,
        S.reset + S.bold, S.reset + S.dim,
        S.reset,
    ) if _UNICODE else "  {}Use Up/Down to move, Space to toggle, Enter to confirm{}".format(S.dim, S.reset))


def select_option(options: List[Tuple[str, str]], prompt: str = "") -> int:
    """Interactive single-select from a list of (value, description) tuples.

    Returns the index of the selected option.
    """
    cursor = 0
    n_items = len(options)

    old_settings = _enable_raw_mode()
    write_raw(S.hide_cursor)

    try:
        _draw_options(options, cursor)

        while True:
            key = _read_key()
            if key == "CTRL_C" or key == "Q":
                write_raw(S.show_cursor)
                _restore_mode(old_settings)
                write()
                fatal("Installation cancelled.")
            elif key == "UP":
                cursor = (cursor - 1) % n_items
            elif key == "DOWN":
                cursor = (cursor + 1) % n_items
            elif key == "ENTER":
                break
            else:
                continue

            write_raw(S.move_up(n_items + 1))  # items + hint
            _draw_options(options, cursor)

    finally:
        write_raw(S.show_cursor)
        _restore_mode(old_settings)

    return cursor


def _draw_options(options: List[Tuple[str, str]], cursor: int) -> None:
    """Render option list for single-select."""
    for i, (value, desc) in enumerate(options):
        if i == cursor:
            pointer = "{}{}{}".format(S.cyan, ARROW_R, S.reset)
            label = "{}{}{} {}{}{}".format(S.bold, value, S.reset, S.gray, desc, S.reset)
        else:
            pointer = " "
            label = "{}{}{} {}{}{}".format(S.white, value, S.reset, S.dim, desc, S.reset)
        write("  {} {}".format(pointer, label))

    write("  {}Use {}\u2191\u2193{} to move, {}Enter{} to confirm{}".format(
        S.dim,
        S.reset + S.bold, S.reset + S.dim,
        S.reset + S.bold, S.reset + S.dim,
        S.reset,
    ) if _UNICODE else "  {}Use Up/Down to move, Enter to confirm{}".format(S.dim, S.reset))


def prompt_text(question: str, default: str = "") -> str:
    """Simple text prompt with default value."""
    if default:
        prompt_str = "  {} {}{}[{}]{}: ".format(S.cyan, question, S.reset + S.dim, default, S.reset)
    else:
        prompt_str = "  {} {}{}: ".format(S.cyan, question, S.reset)
    try:
        answer = input(prompt_str).strip()
    except (EOFError, KeyboardInterrupt):
        fatal("Installation cancelled.")
        return ""  # unreachable
    return answer if answer else default


def prompt_confirm(question: str, default: bool = True) -> bool:
    """Yes/no confirmation prompt."""
    hint = "[Y/n]" if default else "[y/N]"
    prompt_str = "  {}{}{} {}{}{} ".format(S.cyan, BULLET, S.reset, question, S.dim, " " + hint + S.reset)
    try:
        answer = input(prompt_str).strip().lower()
    except (EOFError, KeyboardInterrupt):
        fatal("Installation cancelled.")
        return False
    if not answer:
        return default
    return answer in ("y", "yes")


# ---------------------------------------------------------------------------
# Platform detection
# ---------------------------------------------------------------------------

class Platform:
    """Detect and represent the current platform."""

    def __init__(self) -> None:
        self.system = platform.system().lower()
        self.machine = platform.machine().lower()
        self.os_name = self._detect_os()
        self.arch = self._detect_arch()

    def _detect_os(self) -> str:
        if self.system == "linux":
            return "linux"
        elif self.system == "darwin":
            return "darwin"
        elif self.system == "windows":
            fatal(
                "Windows is not directly supported.\n"
                "  Please use WSL (Windows Subsystem for Linux):\n"
                "  https://learn.microsoft.com/en-us/windows/wsl/install"
            )
            return ""
        else:
            fatal("Unsupported operating system: {}".format(self.system))
            return ""

    def _detect_arch(self) -> str:
        arch_map = {
            "x86_64": "amd64",
            "amd64": "amd64",
            "aarch64": "arm64",
            "arm64": "arm64",
        }
        result = arch_map.get(self.machine, "")
        if not result:
            fatal("Unsupported architecture: {}".format(self.machine))
        return result

    def artifact_suffix(self) -> str:
        """Return the platform suffix for artifact filenames (e.g. linux-amd64)."""
        return "{}-{}".format(self.os_name, self.arch)

    def __str__(self) -> str:
        return "{}-{}".format(self.os_name, self.arch)


# ---------------------------------------------------------------------------
# GitHub release API
# ---------------------------------------------------------------------------

def _urlopen(url: str, timeout: int = 30) -> Any:
    """Open a URL using stdlib urllib. Returns the response object."""
    import ssl
    import urllib.request

    # Some systems have outdated CA bundles; try default first, fall back to
    # unverified if necessary (user is always downloading from GitHub).
    ctx = None
    try:
        ctx = ssl.create_default_context()
    except Exception:
        ctx = ssl._create_unverified_context()

    req = urllib.request.Request(url, headers={"User-Agent": "ZLayer-Installer/{}".format(VERSION)})
    try:
        return urllib.request.urlopen(req, timeout=timeout, context=ctx)
    except Exception:
        # Retry with unverified context if the default failed
        try:
            ctx2 = ssl._create_unverified_context()
            return urllib.request.urlopen(req, timeout=timeout, context=ctx2)
        except Exception as e:
            raise RuntimeError("Failed to fetch {}: {}".format(url, e))


def fetch_latest_release() -> Dict[str, Any]:
    """Fetch the latest release metadata from GitHub API.

    Returns a dict with 'tag_name' and 'assets' keys.
    """
    url = GITHUB_API_LATEST.format(repo=GITHUB_REPO)
    try:
        resp = _urlopen(url)
        data = json.loads(resp.read().decode("utf-8"))
        return data  # type: ignore[no-any-return]
    except Exception as e:
        raise RuntimeError("Failed to fetch latest release info: {}".format(e))


def fetch_release_by_tag(tag: str) -> Dict[str, Any]:
    """Fetch a specific release by tag name."""
    url = "https://api.github.com/repos/{}/releases/tags/{}".format(GITHUB_REPO, tag)
    try:
        resp = _urlopen(url)
        data = json.loads(resp.read().decode("utf-8"))
        return data  # type: ignore[no-any-return]
    except Exception as e:
        raise RuntimeError("Failed to fetch release {}: {}".format(tag, e))


def get_version_tag(release_data: Dict[str, Any]) -> str:
    """Extract the version tag (e.g. 'v0.9.0') from release data."""
    return release_data.get("tag_name", "")


def get_version_number(tag: str) -> str:
    """Strip the 'v' prefix from a tag (v0.9.0 -> 0.9.0)."""
    return tag.lstrip("v")


# ---------------------------------------------------------------------------
# Download
# ---------------------------------------------------------------------------

def download_file(url: str, dest: str, label: str = "") -> None:
    """Download a file with a progress bar.

    Args:
        url: URL to download.
        dest: Destination file path.
        label: Label for the progress bar.
    """
    try:
        resp = _urlopen(url, timeout=120)
    except Exception as e:
        raise RuntimeError("Download failed: {}".format(e))

    total = 0
    content_length = resp.headers.get("Content-Length")
    if content_length:
        try:
            total = int(content_length)
        except ValueError:
            total = 0

    progress = ProgressBar(total, label=label) if total > 0 else None
    chunk_size = 64 * 1024  # 64 KB chunks

    try:
        with open(dest, "wb") as f:
            downloaded = 0
            while True:
                chunk = resp.read(chunk_size)
                if not chunk:
                    break
                f.write(chunk)
                downloaded += len(chunk)
                if progress is not None:
                    progress.update(len(chunk))
                elif S.enabled:
                    write_raw("{}  {} Downloaded {}".format(
                        S.clear_line, S.dim, _format_bytes(downloaded), S.reset
                    ))

        if progress is not None:
            progress.finish()
        elif S.enabled:
            write("")

    except Exception as e:
        # Clean up partial download
        try:
            os.unlink(dest)
        except OSError:
            pass
        raise RuntimeError("Download interrupted: {}".format(e))


# ---------------------------------------------------------------------------
# Archive extraction
# ---------------------------------------------------------------------------

def _safe_extractall(tf: tarfile.TarFile, dest_dir: str) -> None:
    """Extract tarfile safely, using the 'data' filter on Python 3.12+."""
    if sys.version_info >= (3, 12):
        tf.extractall(dest_dir, filter="data")
    else:
        # On older Python, manually check for path traversal
        for member in tf.getmembers():
            member_path = os.path.join(dest_dir, member.name)
            abs_dest = os.path.realpath(dest_dir)
            abs_member = os.path.realpath(member_path)
            if not abs_member.startswith(abs_dest + os.sep) and abs_member != abs_dest:
                raise RuntimeError(
                    "Refusing to extract '{}': path traversal detected".format(member.name)
                )
        tf.extractall(dest_dir)


def extract_binary(archive_path: str, binary_name: str, dest_dir: str) -> str:
    """Extract a specific binary from a tar.gz archive.

    Args:
        archive_path: Path to the .tar.gz file.
        binary_name: Name of the binary to find inside.
        dest_dir: Temporary extraction directory.

    Returns:
        Path to the extracted binary.
    """
    try:
        with tarfile.open(archive_path, "r:gz") as tf:
            # List members and find the binary
            members = tf.getnames()
            target = None
            for member_name in members:
                basename = os.path.basename(member_name)
                if basename == binary_name:
                    target = member_name
                    break

            if target is None:
                # Try extracting all and searching
                _safe_extractall(tf, dest_dir)
                for root, dirs, files in os.walk(dest_dir):
                    if binary_name in files:
                        return os.path.join(root, binary_name)
                raise RuntimeError(
                    "Binary '{}' not found in archive. Contents: {}".format(
                        binary_name, ", ".join(members)
                    )
                )

            _safe_extractall(tf, dest_dir)
            return os.path.join(dest_dir, target)

    except tarfile.TarError as e:
        raise RuntimeError("Failed to extract archive: {}".format(e))


# ---------------------------------------------------------------------------
# Installation
# ---------------------------------------------------------------------------

def needs_sudo(path: str) -> bool:
    """Check if writing to path requires elevated privileges."""
    path = os.path.expanduser(path)
    if os.path.exists(path):
        return not os.access(path, os.W_OK)
    parent = os.path.dirname(path)
    if os.path.exists(parent):
        return not os.access(parent, os.W_OK)
    return True


def install_binary(src: str, install_dir: str, binary_name: str, use_sudo: bool = False) -> str:
    """Install a binary to the target directory.

    Args:
        src: Source binary path.
        install_dir: Target directory.
        binary_name: Name for the installed binary.
        use_sudo: Whether to use sudo for the copy.

    Returns:
        Full path to the installed binary.
    """
    install_dir = os.path.expanduser(install_dir)
    dest = os.path.join(install_dir, binary_name)

    # Ensure directory exists
    if not os.path.exists(install_dir):
        if use_sudo:
            _run_sudo(["mkdir", "-p", install_dir])
        else:
            os.makedirs(install_dir, exist_ok=True)

    if use_sudo:
        _run_sudo(["cp", src, dest])
        _run_sudo(["chmod", "755", dest])
    else:
        shutil.copy2(src, dest)
        os.chmod(dest, stat.S_IRWXU | stat.S_IRGRP | stat.S_IXGRP | stat.S_IROTH | stat.S_IXOTH)

    return dest


def _run_sudo(cmd: List[str]) -> None:
    """Run a command with sudo."""
    full_cmd = ["sudo"] + cmd
    try:
        subprocess.run(full_cmd, check=True, capture_output=True)
    except subprocess.CalledProcessError as e:
        stderr = e.stderr.decode("utf-8", errors="replace") if e.stderr else ""
        raise RuntimeError("sudo command failed: {}".format(stderr or str(e)))
    except FileNotFoundError:
        raise RuntimeError("sudo is not available. Try installing to ~/.local/bin instead.")


def verify_binary(binary_path: str) -> Optional[str]:
    """Run the binary with --version and return the output, or None on failure."""
    try:
        result = subprocess.run(
            [binary_path, "--version"],
            capture_output=True,
            timeout=10,
        )
        if result.returncode == 0:
            output = result.stdout.decode("utf-8", errors="replace").strip()
            return output if output else "(ok)"
        return None
    except Exception:
        return None


# ---------------------------------------------------------------------------
# PATH management
# ---------------------------------------------------------------------------

def is_on_path(directory: str) -> bool:
    """Check if a directory is on the current PATH."""
    directory = os.path.expanduser(directory)
    path_dirs = os.environ.get("PATH", "").split(os.pathsep)
    return any(os.path.realpath(d) == os.path.realpath(directory) for d in path_dirs if d)


def detect_shell() -> str:
    """Detect the user's current shell."""
    shell = os.environ.get("SHELL", "")
    if "zsh" in shell:
        return "zsh"
    elif "fish" in shell:
        return "fish"
    return "bash"


def add_to_path(directory: str, auto: bool = False) -> List[str]:
    """Add a directory to shell RC files.

    Args:
        directory: Directory to add.
        auto: If True, don't ask for confirmation.

    Returns:
        List of files that were modified.
    """
    directory = os.path.expanduser(directory)
    shell = detect_shell()
    rc_files = SHELL_RC_FILES.get(shell, [".bashrc"])
    home = os.path.expanduser("~")
    modified = []

    if shell == "fish":
        export_line = 'set -gx PATH "{}" $PATH'.format(directory)
    else:
        export_line = 'export PATH="{}:$PATH"'.format(directory)

    marker = "# Added by ZLayer installer"

    for rc in rc_files:
        rc_path = os.path.join(home, rc)
        if not os.path.exists(rc_path):
            continue

        # Check if already present
        try:
            with open(rc_path, "r") as f:
                contents = f.read()
            if directory in contents:
                continue
        except (IOError, OSError):
            continue

        try:
            with open(rc_path, "a") as f:
                f.write("\n{}  {}\n{}\n".format(marker, "", export_line))
            modified.append(rc_path)
        except (IOError, OSError):
            warn("Could not write to {}".format(rc_path))

    # Also try the other common shells
    for other_shell, other_rcs in SHELL_RC_FILES.items():
        if other_shell == shell:
            continue
        for rc in other_rcs:
            rc_path = os.path.join(home, rc)
            if not os.path.exists(rc_path):
                continue
            try:
                with open(rc_path, "r") as f:
                    contents = f.read()
                if directory in contents:
                    continue
            except (IOError, OSError):
                continue

            if other_shell == "fish":
                other_export = 'set -gx PATH "{}" $PATH'.format(directory)
            else:
                other_export = 'export PATH="{}:$PATH"'.format(directory)

            try:
                with open(rc_path, "a") as f:
                    f.write("\n{}  {}\n{}\n".format(marker, "", other_export))
                modified.append(rc_path)
            except (IOError, OSError):
                pass

    return modified


# ---------------------------------------------------------------------------
# Logo
# ---------------------------------------------------------------------------

LOGO = r"""
  ______
 |___  / |
    / /  | |     __ _ _   _  ___ _ __
   / /   | |    / _` | | | |/ _ \ '__|
  / /__  | |___| (_| | |_| |  __/ |
 /_____| |______\__,_|\__, |\___|_|
                       __/ |
                      |___/
"""


def print_header() -> None:
    """Print the ZLayer logo and header."""
    write()
    for line in LOGO.strip().splitlines():
        write("  {}{}{}".format(S.cyan + S.bold, line, S.reset))
    write()
    draw_box(["ZLayer Installer", "Container Orchestration Platform"], width=47, color=S.cyan)
    write()


# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------

def parse_args(argv: Optional[List[str]] = None) -> argparse.Namespace:
    """Parse command-line arguments."""
    parser = argparse.ArgumentParser(
        description="ZLayer Installer - Install ZLayer container orchestration tools.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=textwrap.dedent("""\
            Examples:
              python3 install.py
              python3 install.py --yes --binary zlayer-build
              python3 install.py --yes --binary all --dir ~/.local/bin
              python3 install.py --version v0.9.0 --binary zlayer
              curl -sSL https://zlayer.dev/install.py | python3 - --yes
        """),
    )
    parser.add_argument(
        "-y", "--yes",
        action="store_true",
        help="Non-interactive mode; accept all defaults.",
    )
    parser.add_argument(
        "--binary",
        type=str,
        default=None,
        help="Binary to install: zlayer, zlayer-build, zlayer-cli, or 'all'. "
             "Can be specified multiple times or comma-separated.",
    )
    parser.add_argument(
        "--dir",
        type=str,
        default=None,
        help="Installation directory (default: /usr/local/bin or ~/.local/bin).",
    )
    parser.add_argument(
        "--version",
        type=str,
        default=None,
        help="Specific version to install (e.g. v0.9.0 or 0.9.0).",
    )
    parser.add_argument(
        "--no-color",
        action="store_true",
        help="Disable colored output.",
    )
    parser.add_argument(
        "--no-modify-path",
        action="store_true",
        help="Do not offer to modify PATH in shell RC files.",
    )

    args = parser.parse_args(argv)
    return args


# ---------------------------------------------------------------------------
# Signal handling
# ---------------------------------------------------------------------------

_cleanup_paths = []  # type: List[str]


def _signal_handler(sig: int, frame: Any) -> None:
    """Handle Ctrl+C gracefully."""
    write_raw(S.show_cursor)
    write()
    write()
    warn("Installation cancelled by user.")
    for p in _cleanup_paths:
        try:
            shutil.rmtree(p, ignore_errors=True)
        except Exception:
            pass
    write()
    sys.exit(130)


# ---------------------------------------------------------------------------
# Main installation flow
# ---------------------------------------------------------------------------

def _is_interactive() -> bool:
    """Check if stdin is a terminal (interactive)."""
    try:
        return sys.stdin.isatty() and sys.stdout.isatty()
    except Exception:
        return False


def _resolve_binaries(args: argparse.Namespace) -> List[str]:
    """Determine which binaries to install based on args or interactive selection."""
    if args.binary:
        raw = args.binary
        if raw == "all":
            return list(BINARY_ORDER)
        names = [n.strip() for n in raw.split(",")]
        valid = []
        for n in names:
            if n in BINARIES:
                valid.append(n)
            else:
                warn("Unknown binary '{}'. Available: {}".format(n, ", ".join(BINARY_ORDER)))
        if not valid:
            fatal("No valid binaries specified.")
        return valid

    if args.yes or not _is_interactive():
        # Default: just zlayer-build
        return ["zlayer-build"]

    # Interactive selection
    write("  {}{}? What would you like to install?{}".format(S.bold, S.green, S.reset))
    write()
    defaults = [BINARIES[name].get("default", False) for name in BINARY_ORDER]
    selected = select_binaries(BINARY_ORDER, defaults)

    if not selected:
        fatal("No binaries selected.")
    return selected


def _resolve_install_dir(args: argparse.Namespace) -> Tuple[str, bool]:
    """Determine install directory. Returns (path, use_sudo)."""
    if args.dir:
        d = os.path.expanduser(args.dir)
        return d, needs_sudo(d)

    if args.yes or not _is_interactive():
        # Default: try /usr/local/bin, fall back to ~/.local/bin
        if not needs_sudo("/usr/local/bin"):
            return "/usr/local/bin", False
        return os.path.expanduser("~/.local/bin"), False

    # Interactive
    write()
    write("  {}{}? Install location{}".format(S.bold, S.green, S.reset))
    write()

    options = [
        (loc, desc) for loc, desc in INSTALL_LOCATIONS
    ]
    options.append(("Custom path...", "Enter a custom directory"))

    idx = select_option(options)

    if idx == len(INSTALL_LOCATIONS):
        # Custom path
        write()
        custom = prompt_text("Enter install directory", default="/usr/local/bin")
        custom = os.path.expanduser(custom)
        return custom, needs_sudo(custom)

    chosen = INSTALL_LOCATIONS[idx][0]
    chosen = os.path.expanduser(chosen)
    return chosen, needs_sudo(chosen)


def _resolve_version(args: argparse.Namespace) -> Tuple[str, Dict[str, Any]]:
    """Determine version to install. Returns (tag, release_data)."""
    if args.version:
        tag = args.version if args.version.startswith("v") else "v" + args.version
        with Spinner("Fetching release {}...".format(tag)) as sp:
            try:
                data = fetch_release_by_tag(tag)
            except RuntimeError:
                sp.stop()
                fatal("Could not find release {}".format(tag))
                return "", {}  # unreachable
            sp.stop("  {}{}{}  Found release {}".format(S.green, CHECK, S.reset, tag))
        return tag, data

    # Fetch latest
    spinner = Spinner("Fetching latest version...")
    spinner.start()
    try:
        data = fetch_latest_release()
        tag = get_version_tag(data)
        if not tag:
            spinner.stop()
            fatal("Could not determine latest version from GitHub.")
        spinner.stop("  {}{}{}  Latest version: {}{}{}".format(
            S.green, CHECK, S.reset, S.bold, tag, S.reset
        ))
    except RuntimeError as e:
        spinner.stop()
        fatal(str(e))
        return "", {}  # unreachable

    if not args.yes and _is_interactive():
        write()
        write("  {}{}? Version{}".format(S.bold, S.green, S.reset))
        write()
        options = [
            ("latest ({})".format(tag), "Recommended"),
            ("Specific version...", "Enter a version number"),
        ]
        idx = select_option(options)
        if idx == 1:
            write()
            ver = prompt_text("Enter version (e.g. 0.9.0)")
            tag = ver if ver.startswith("v") else "v" + ver
            spinner2 = Spinner("Fetching release {}...".format(tag))
            spinner2.start()
            try:
                data = fetch_release_by_tag(tag)
                spinner2.stop("  {}{}{}  Found release {}".format(S.green, CHECK, S.reset, tag))
            except RuntimeError:
                spinner2.stop()
                fatal("Could not find release {}".format(tag))
                return "", {}  # unreachable

    return tag, data


def _construct_download_url(binary_name: str, tag: str, version: str, plat: Platform, release_data: Dict[str, Any]) -> str:
    """Build the download URL for a specific binary.

    First checks if there's a matching asset in the release data, then falls
    back to the constructed URL pattern.
    """
    # The current naming convention from the release workflow:
    #   zlayer-{version}-{os}-{arch}.tar.gz    (for the main zlayer binary)
    # For multiple binaries, the expected pattern is:
    #   {binary_name}-{version}-{os}-{arch}.tar.gz
    # But the current release only packages "zlayer". We'll try both patterns.

    suffix = plat.artifact_suffix()

    # Pattern 1: {binary}-{version}-{os}-{arch}.tar.gz
    filename_with_binary = "{}-{}-{}.tar.gz".format(binary_name, version, suffix)
    # Pattern 2: zlayer-{version}-{os}-{arch}.tar.gz (historical, single binary)
    filename_legacy = "zlayer-{}-{}.tar.gz".format(version, suffix)

    # Check release assets for exact match
    assets = release_data.get("assets", [])
    asset_names = [a.get("name", "") for a in assets]

    for pattern in [filename_with_binary, filename_legacy]:
        for asset in assets:
            if asset.get("name") == pattern:
                # Prefer browser_download_url
                url = asset.get("browser_download_url", "")
                if url:
                    return url

    # Fallback: construct the URL directly
    for filename in [filename_with_binary, filename_legacy]:
        url = GITHUB_DOWNLOAD.format(repo=GITHUB_REPO, tag=tag, filename=filename)
        return url

    return GITHUB_DOWNLOAD.format(repo=GITHUB_REPO, tag=tag, filename=filename_with_binary)


def main(argv: Optional[List[str]] = None) -> None:
    """Main installer entry point."""
    args = parse_args(argv)

    # Handle --no-color
    if args.no_color:
        S.enabled = False

    # Signal handling
    signal.signal(signal.SIGINT, _signal_handler)

    # Header
    print_header()

    # Check Python version
    if sys.version_info < (3, 8):
        fatal("Python 3.8 or later is required. You have {}.{}.{}".format(*sys.version_info[:3]))

    # Platform detection
    plat = Platform()
    info("Detected platform: {}{}{}".format(S.bold, plat, S.reset))
    write()

    # Step 1: Select binaries
    binaries = _resolve_binaries(args)
    write()
    info("Selected: {}{}{}".format(S.bold, ", ".join(binaries), S.reset))

    # Step 2: Install directory
    install_dir, use_sudo = _resolve_install_dir(args)
    write()
    info("Install directory: {}{}{}".format(S.bold, install_dir, S.reset))
    if use_sudo:
        info("{}(will use sudo){}".format(S.yellow, S.reset))

    # Step 3: Version
    write()
    tag, release_data = _resolve_version(args)
    version = get_version_number(tag)

    # Confirmation in interactive mode
    if not args.yes and _is_interactive():
        write()
        write("  {}{}Summary:{}".format(S.bold, S.underline, S.reset))
        write("    Binaries:  {}{}{}".format(S.bold, ", ".join(binaries), S.reset))
        write("    Version:   {}{}{}".format(S.bold, tag, S.reset))
        write("    Platform:  {}{}{}".format(S.bold, plat, S.reset))
        write("    Directory: {}{}{}".format(S.bold, install_dir, S.reset))
        write()
        if not prompt_confirm("Proceed with installation?"):
            fatal("Installation cancelled.")

    # Step 4: Download and install each binary
    write()
    tmpdir = tempfile.mkdtemp(prefix="zlayer-install-")
    _cleanup_paths.append(tmpdir)

    installed_paths = []  # type: List[str]

    for binary_name in binaries:
        write("  {}{}{}  Downloading {}{}{} {} for {}...".format(
            S.blue, BULLET, S.reset,
            S.bold, binary_name, S.reset,
            tag, plat,
        ))

        # Build download URL
        url = _construct_download_url(binary_name, tag, version, plat, release_data)

        archive_name = os.path.basename(url)
        archive_path = os.path.join(tmpdir, archive_name)

        # Download
        try:
            download_file(url, archive_path, label=binary_name)
        except RuntimeError as e:
            error("Failed to download {}: {}".format(binary_name, e))
            write()
            # If this is a secondary binary like zlayer-cli that might not exist yet
            if binary_name != "zlayer-build" and binary_name != "zlayer":
                warn("Binary '{}' may not be available in this release yet.".format(binary_name))
                warn("Skipping...")
                write()
                continue
            else:
                fatal("Download failed. Check your internet connection and try again.")

        # Verify archive size (basic sanity check)
        try:
            file_size = os.path.getsize(archive_path)
            if file_size < 1024:
                error("Downloaded file is suspiciously small ({} bytes).".format(file_size))
                fatal("The download may be corrupted. Please try again.")
        except OSError:
            fatal("Could not read downloaded file.")

        # Extract
        extract_dir = os.path.join(tmpdir, "extract-{}".format(binary_name))
        os.makedirs(extract_dir, exist_ok=True)

        spinner = Spinner("Extracting {}...".format(binary_name))
        spinner.start()
        try:
            binary_path = extract_binary(archive_path, binary_name, extract_dir)
            spinner.stop("  {}{}{}  Extracted {}".format(S.green, CHECK, S.reset, binary_name))
        except RuntimeError as e:
            spinner.stop()
            error("Extraction failed: {}".format(e))
            continue

        # Install
        spinner = Spinner("Installing {}...".format(binary_name))
        spinner.start()
        try:
            installed_path = install_binary(binary_path, install_dir, binary_name, use_sudo=use_sudo)
            spinner.stop("  {}{}{}  Installed {} to {}{}{}".format(
                S.green, CHECK, S.reset,
                binary_name,
                S.bold, installed_path, S.reset,
            ))
            installed_paths.append(installed_path)
        except RuntimeError as e:
            spinner.stop()
            error("Installation failed: {}".format(e))
            if "sudo" in str(e).lower() or "permission" in str(e).lower():
                warn("Try installing to ~/.local/bin instead:")
                warn("  python3 install.py --dir ~/.local/bin")
            continue

        # Verify
        ver_output = verify_binary(installed_path)
        if ver_output:
            success("Verified: {}".format(ver_output))
        else:
            warn("Could not verify {} (this may be normal on first install)".format(binary_name))

        write()

    # Cleanup
    try:
        shutil.rmtree(tmpdir, ignore_errors=True)
    except Exception:
        pass
    if tmpdir in _cleanup_paths:
        _cleanup_paths.remove(tmpdir)

    if not installed_paths:
        fatal("No binaries were installed.")

    # Step 5: PATH management
    if not args.no_modify_path and not is_on_path(install_dir):
        write()
        if args.yes or not _is_interactive():
            # Auto-add in non-interactive mode
            modified = add_to_path(install_dir, auto=True)
            if modified:
                for f in modified:
                    success("Added to PATH in {}".format(f))
            else:
                warn("{} is not on your PATH.".format(install_dir))
                info("Add it manually:")
                info('  export PATH="{}:$PATH"'.format(install_dir))
        else:
            if prompt_confirm("Add {} to your PATH?".format(install_dir)):
                modified = add_to_path(install_dir)
                if modified:
                    for f in modified:
                        success("Added to PATH in {}".format(os.path.basename(f)))
                    write()
                    shell = detect_shell()
                    shell_rc = SHELL_RC_FILES.get(shell, [".bashrc"])[0]
                    info("Run: {}source ~/{}{}".format(S.bold, shell_rc, S.reset))
                else:
                    warn("Could not modify any shell RC files.")
                    info("Add manually:")
                    info('  export PATH="{}:$PATH"'.format(install_dir))
            else:
                write()
                info("To add to PATH later:")
                info('  export PATH="{}:$PATH"'.format(install_dir))

    # Final summary
    write()
    draw_box(["Installation Complete!"], width=47, color=S.green)
    write()

    # Get started hints
    for path in installed_paths:
        name = os.path.basename(path)
        if name == "zlayer":
            info("Get started with zlayer:")
            write("    {}zlayer --help{}".format(S.bold, S.reset))
            write("    {}zlayer serve{}              {}# Start the API server{}".format(
                S.bold, S.reset, S.dim, S.reset
            ))
            write("    {}zlayer deploy --spec app.yaml{}  {}# Deploy services{}".format(
                S.bold, S.reset, S.dim, S.reset
            ))
        elif name == "zlayer-build":
            info("Get started with zlayer-build:")
            write("    {}zlayer-build --help{}".format(S.bold, S.reset))
            write("    {}zlayer-build build .{}        {}# Build from ZImagefile{}".format(
                S.bold, S.reset, S.dim, S.reset
            ))
        elif name == "zlayer-cli":
            info("Get started with zlayer-cli:")
            write("    {}zlayer-cli{}                  {}# Launch interactive TUI{}".format(
                S.bold, S.reset, S.dim, S.reset
            ))
        write()

    info("Documentation: {}https://zlayer.dev{}".format(S.underline, S.reset))
    info("Issues: {}https://github.com/{}/issues{}".format(S.underline, GITHUB_REPO, S.reset))
    write()


if __name__ == "__main__":
    main()
