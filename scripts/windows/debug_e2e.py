#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = [
#   "rich>=13.7",
# ]
# ///
"""
debug_e2e.py — One-shot Windows HCS+HCN e2e debug cycle driver.

Replaces the manual loop of: rsync -> cleanup -> launch -> poll -> fetch ->
read log -> cross-reference HCS/HCN docs. Invoke from the ZLayer repo root:

    uv run scripts/windows/debug_e2e.py

All the heavy lifting lives in the existing PowerShell scripts in this
directory; this driver only orchestrates SSH/SCP/rsync and assembles a
report.md from the artifacts the test wrote.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Iterable, Iterator

from rich.console import Console
from rich.rule import Rule

console = Console(highlight=False)

# OpenSSH spams "post-quantum key exchange not used" warnings on every
# connection right now; filter them out so output stays readable.
PQ_NOISE = re.compile(r"post-quantum|store now|openssh\.com/pq", re.IGNORECASE)


def _filter_pq(line: str) -> bool:
    return PQ_NOISE.search(line) is None


def die(msg: str, code: int = 2) -> "None":
    console.print(f"[bold red]error:[/] {msg}")
    sys.exit(code)


def section(title: str) -> None:
    console.print()
    console.print(Rule(f"[bold cyan]{title}[/]"))


# ---------------------------------------------------------------------------
# Subprocess helpers
# ---------------------------------------------------------------------------


def stream(
    argv: list[str],
    prefix: str,
    *,
    capture: bool = False,
    check: bool = True,
) -> tuple[int, list[str]]:
    """Run argv, stream stdout/stderr line-by-line with prefix. Optionally capture lines."""
    captured: list[str] = []
    try:
        proc = subprocess.Popen(
            argv,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
    except FileNotFoundError as e:
        die(f"missing binary: {e.filename}")
    assert proc.stdout is not None
    for raw in proc.stdout:
        line = raw.rstrip("\r\n")
        if not _filter_pq(line):
            continue
        if capture:
            captured.append(line)
        console.print(f"[dim]{prefix}[/] {line}")
    rc = proc.wait()
    if check and rc != 0:
        die(f"{prefix} failed (rc={rc})")
    return rc, captured


def ssh_argv(host: str, remote_cmd: str) -> list[str]:
    return [
        "ssh",
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=accept-new",
        host,
        remote_cmd,
    ]


def remote_ps(host: str, ps_path: str, args: list[str] | None = None) -> str:
    """Quote a PowerShell File invocation for SSH."""
    parts = [
        "powershell",
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        ps_path,
    ]
    if args:
        parts.extend(args)
    return " ".join(shlex.quote(p) for p in parts)


def run_remote_ps(
    host: str,
    ps_path: str,
    args: list[str] | None = None,
    *,
    prefix: str = "ssh",
    capture: bool = True,
    check: bool = True,
) -> tuple[int, list[str]]:
    cmd = remote_ps(host, ps_path, args)
    return stream(ssh_argv(host, cmd), prefix, capture=capture, check=check)


# ---------------------------------------------------------------------------
# Phase helpers
# ---------------------------------------------------------------------------


@dataclass
class LaunchInfo:
    pid: str
    rundir_remote: str
    log_remote: str


def phase_rsync(local_repo: Path, host: str) -> None:
    section("1. Rsync")
    if shutil.which("rsync") is None:
        die("rsync not on PATH")
    src = str(local_repo).rstrip("/") + "/"
    dst = f"{host}:/cygdrive/c/src/ZLayer/"
    argv = [
        "rsync",
        "-az",
        "--delete",
        "--exclude=target",
        "--exclude=.git",
        "--exclude=node_modules",
        "--exclude=*.log",
        "--exclude=.claude",
        src,
        dst,
    ]
    # Cygwin OpenSSH on Windows occasionally drops the connection at the END
    # of a transfer (rc=12 / "connection unexpectedly closed") after the
    # actual file transfer has completed. Retry once before giving up;
    # subsequent runs are cheap because rsync only ships what changed.
    rc, _ = stream(argv, "[rsync]", capture=False, check=False)
    if rc != 0:
        console.print(f"[yellow]rsync rc={rc} — retrying once[/]")
        rc, _ = stream(argv, "[rsync-retry]", capture=False, check=False)
        if rc != 0:
            die(f"rsync failed twice (rc={rc})")


def phase_clean_deep(host: str, repo_remote: str) -> None:
    section("0. Deep clean (test scratch dirs + stale HCS systems + HCN)")
    # check=False per the same Cygwin-OpenSSH late-disconnect pattern.
    rc, lines = run_remote_ps(
        host,
        f"{repo_remote}/scripts/windows/deep_clean.ps1",
        prefix="[deep-clean]",
        check=False,
    )
    saw_done = any(ln.strip() == "DONE" for ln in lines)
    if not saw_done:
        die(f"[deep-clean] no DONE marker in output (rc={rc}) — script may have failed")
    console.print(f"[green]deep clean OK[/] (ssh rc={rc})")


def phase_cleanup(host: str, repo_remote: str) -> None:
    section("2. Cleanup HCN")
    # check=False because Cygwin OpenSSH on Windows often returns rc=255 with
    # "Timeout, server X not responding" AFTER the remote work completed. We
    # judge success by whether the cleanup script's own "REMOVED=" marker
    # appears in the captured output, not the SSH exit code.
    rc, lines = run_remote_ps(
        host,
        f"{repo_remote}/scripts/windows/cleanup_hcn.ps1",
        prefix="[cleanup]",
        check=False,
    )
    removed = None
    saw_activity = False
    for ln in lines:
        if ln.startswith("REMOVED="):
            removed = ln.split("=", 1)[1].strip()
            break
        if ln.startswith("Removing "):
            saw_activity = True
    if removed is None:
        if not saw_activity:
            die(f"[cleanup] no output (rc={rc}) — cleanup script may not have run")
        # Cygwin OpenSSH late-disconnect: script ran and made progress, just
        # the connection-close exit was lost. Trust the activity lines.
        console.print(
            f"[yellow]cleanup ran (saw Removing lines) but no REMOVED= marker (rc={rc}) "
            "— likely late-disconnect; continuing[/]"
        )
    else:
        console.print(f"[green]removed {removed} endpoints/networks[/] (ssh rc={rc})")


def phase_check_hcn(host: str, repo_remote: str, label: str) -> list[str]:
    section(f"3. Capture {label}-state (check_hcn.ps1)")
    _, lines = run_remote_ps(
        host,
        f"{repo_remote}/scripts/windows/check_hcn.ps1",
        prefix=f"[{label}]",
    )
    return lines


def phase_launch(host: str, repo_remote: str, test: str) -> LaunchInfo:
    section("4. Launch")
    rc, lines = run_remote_ps(
        host,
        f"{repo_remote}/scripts/windows/launch_e2e.ps1",
        ["-Test", test],
        prefix="[launch]",
        check=False,
    )
    if rc != 0:
        die(f"launch_e2e.ps1 exited rc={rc}; aborting (no poll on failed launch)")
    info: dict[str, str] = {}
    for ln in lines:
        m = re.match(r"^(PID|RUNDIR|LOG)=(.+)$", ln)
        if m:
            info[m.group(1)] = m.group(2).strip()
    if "PID" not in info or "RUNDIR" not in info:
        die(f"launch_e2e.ps1 did not print PID/RUNDIR; got: {lines!r}")
    return LaunchInfo(
        pid=info["PID"],
        rundir_remote=info["RUNDIR"],
        log_remote=info.get("LOG", info["RUNDIR"] + r"\stdout.log"),
    )


def phase_poll(
    host: str,
    repo_remote: str,
    test: str,
    poll_interval: int,
    timeout_min: int,
) -> dict[str, str]:
    section("5. Poll")
    deadline = time.monotonic() + (timeout_min * 60)
    started = time.monotonic()
    last: dict[str, str] = {}
    while True:
        elapsed = int(time.monotonic() - started)
        # status_e2e prints to stdout — quiet capture, then print one summary
        cmd = remote_ps(
            host,
            f"{repo_remote}/scripts/windows/status_e2e.ps1",
            ["-Test", test, "-Tail", "1"],
        )
        proc = subprocess.run(
            ssh_argv(host, cmd),
            capture_output=True,
            text=True,
        )
        out_lines = [
            ln for ln in (proc.stdout + proc.stderr).splitlines() if _filter_pq(ln)
        ]
        kv: dict[str, str] = {}
        for ln in out_lines:
            m = re.match(r"^([A-Z]+)=(.*)$", ln)
            if m:
                kv[m.group(1)] = m.group(2).strip()
        last = kv
        summary = (
            f"[poll t+{elapsed}s] STATE={kv.get('STATE', '?')} "
            f"LOGSIZE={kv.get('LOGSIZE', '?')} "
            f"ALIVE={kv.get('ALIVE', '?')} "
            f"PID={kv.get('PID', '?')}"
        )
        color = "yellow"
        if kv.get("STATE") == "DONE":
            color = "green" if kv.get("RC") == "0" else "red"
        elif kv.get("ALIVE") == "false" and kv.get("STATE") != "DONE":
            color = "red"
        console.print(f"[{color}]{summary}[/]")

        if kv.get("STATE") == "DONE":
            return kv
        if kv.get("ALIVE") == "false" and kv.get("STATE") != "DONE":
            console.print(
                "[red]process died before sentinel — treating as failure[/]"
            )
            return kv
        if time.monotonic() >= deadline:
            console.print(f"[red]hard timeout after {timeout_min}m[/]")
            return kv
        time.sleep(poll_interval)


def phase_fetch(host: str, rundir_remote: str, local_target: Path) -> None:
    section("6. Fetch artifacts")
    local_target.mkdir(parents=True, exist_ok=True)
    # Windows OpenSSH server (the default on Win10/11) does NOT understand
    # `/cygdrive/c/...` paths — only native `C:/...` (forward slashes). Cygwin
    # sshd would accept both. Use the Windows-native form; falls back to the
    # original if the path doesn't look like a drive-letter path.
    rd = rundir_remote.replace("\\", "/")
    src_dir = f"{host}:{rd}"
    # Fetch the directory itself (no wildcard — cygwin/Windows OpenSSH expand
    # wildcards differently and unreliably). This nests the rundir under
    # local_target.parent; we flatten it back into local_target afterwards.
    argv = [
        "scp",
        "-q",
        "-o",
        "BatchMode=yes",
        "-r",
        src_dir,
        str(local_target.parent) + "/",
    ]
    proc = subprocess.run(argv, capture_output=True, text=True)
    if proc.returncode != 0:
        console.print(f"[yellow]scp rc={proc.returncode}: {proc.stderr.strip()}[/]")
        console.print("[yellow]continuing with whatever we got[/]")
    else:
        nested = local_target.parent / Path(rd).name
        if nested.exists() and nested != local_target:
            for item in nested.iterdir():
                dst = local_target / item.name
                if dst.exists():
                    shutil.rmtree(dst) if dst.is_dir() else dst.unlink()
                shutil.move(str(item), str(dst))
            nested.rmdir()
    for ln in (proc.stderr or "").splitlines():
        if _filter_pq(ln):
            console.print(f"[dim][scp][/] {ln}")
    console.print(f"[green]fetched into {local_target}[/]")


# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------


def _read_text_safe(p: Path, limit: int | None = None) -> str:
    try:
        data = p.read_text(encoding="utf-8", errors="replace")
    except FileNotFoundError:
        return ""
    if limit is not None and len(data) > limit:
        data = data[-limit:]
    return data


def _tail(lines: Iterable[str], n: int) -> list[str]:
    buf: list[str] = []
    for ln in lines:
        buf.append(ln)
        if len(buf) > n:
            buf.pop(0)
    return buf


def _parse_test_results(log: str) -> tuple[list[str], list[tuple[str, str]]]:
    """Return (per-test outcome lines, [(name, panic-block)]) from cargo test output."""
    outcomes: list[str] = []
    failures: list[tuple[str, str]] = []
    for ln in log.splitlines():
        if re.match(r"^test \S+ \.\.\. (ok|FAILED|ignored)", ln):
            outcomes.append(ln)

    # Failure detail blocks live under "---- <name> stdout ----"
    cur_name: str | None = None
    cur_lines: list[str] = []
    for ln in log.splitlines():
        m = re.match(r"^---- (\S+) stdout ----", ln)
        if m:
            if cur_name is not None:
                failures.append((cur_name, "\n".join(cur_lines).rstrip()))
            cur_name = m.group(1)
            cur_lines = []
            continue
        if cur_name is not None:
            if ln.startswith("failures:") or ln.startswith("test result:"):
                failures.append((cur_name, "\n".join(cur_lines).rstrip()))
                cur_name = None
                cur_lines = []
                continue
            cur_lines.append(ln)
    if cur_name is not None:
        failures.append((cur_name, "\n".join(cur_lines).rstrip()))
    return outcomes, failures


def _result_line(log: str) -> tuple[int, int, int, str]:
    """Extract `test result:` line; return (passed, failed, ignored, raw)."""
    for ln in reversed(log.splitlines()):
        m = re.search(
            r"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored", ln
        )
        if m:
            return int(m.group(1)), int(m.group(2)), int(m.group(3)), ln.strip()
    return 0, 0, 0, "(no test result line found)"


def _finished_in(log: str) -> str:
    for ln in reversed(log.splitlines()):
        m = re.search(r"finished in [\d.]+s", ln)
        if m:
            return ln.strip()
    return ""


def _embed_json(p: Path) -> str:
    try:
        raw = p.read_text(encoding="utf-8", errors="replace")
        obj = json.loads(raw)
        return json.dumps(obj, indent=2)
    except (json.JSONDecodeError, FileNotFoundError):
        return _read_text_safe(p)


def write_report(
    run_local: Path,
    *,
    test_name: str,
    rc: str,
    duration_s: float,
    pre_state: list[str],
    post_state: list[str],
    final_status: dict[str, str],
) -> Path:
    log_path = run_local / "stdout.log"
    log = _read_text_safe(log_path)
    passed, failed, ignored, raw_result = _result_line(log)
    outcomes, failures = _parse_test_results(log)

    docs_dir = run_local / "hcs-docs"
    hcs_files: list[Path] = []
    hcn_files: list[Path] = []
    if docs_dir.is_dir():
        for p in sorted(docs_dir.iterdir()):
            if not p.is_file() or p.suffix.lower() != ".json":
                continue
            name = p.name.lower()
            if "hcn" in name or "endpoint" in name or "network" in name or "namespace" in name:
                hcn_files.append(p)
            else:
                hcs_files.append(p)

    pre_path = run_local / "pre-state.txt"
    post_path = run_local / "post-state.txt"
    pre_path.write_text("\n".join(pre_state) + "\n", encoding="utf-8")
    post_path.write_text("\n".join(post_state) + "\n", encoding="utf-8")

    overall = "PASS" if rc == "0" else "FAIL"
    out: list[str] = []
    out.append(f"# debug_e2e report — {test_name}")
    out.append("")
    out.append(f"- Generated: {datetime.now(timezone.utc).isoformat()}")
    out.append(f"- Run dir: `{run_local}`")
    out.append("")
    out.append("## Test result")
    out.append("")
    out.append(f"- Overall: **{overall}**")
    out.append(f"- rc: `{rc}`")
    out.append(f"- duration: `{duration_s:.1f}s`")
    out.append(f"- {raw_result}")
    fi = _finished_in(log)
    if fi:
        out.append(f"- {fi}")
    for k, v in final_status.items():
        out.append(f"- status.{k.lower()}: `{v}`")
    out.append("")
    out.append("## Per-test outcomes")
    out.append("")
    if outcomes:
        for o in outcomes:
            out.append(f"- `{o}`")
    else:
        out.append("(no `test ... ... ok/FAILED/ignored` lines parsed)")
    out.append("")

    out.append("## Failures")
    out.append("")
    if not failures:
        out.append("(none)")
    else:
        for name, block in failures:
            out.append(f"### {name}")
            out.append("")
            # First panic line + file:line if present
            for ln in block.splitlines():
                m = re.search(r"panicked at ([^:]+:\d+(?::\d+)?)", ln)
                if m:
                    out.append(f"- panic site: `{m.group(1)}`")
                    break
            # Whole panic block
            out.append("")
            out.append("```")
            out.append(block if block else "(empty)")
            out.append("```")
            out.append("")

    out.append("## HCS docs")
    out.append("")
    if not hcs_files:
        out.append("(none found in hcs-docs/)")
    for p in hcs_files:
        out.append(f"### `{p.name}`")
        out.append("")
        out.append("```json")
        out.append(_embed_json(p))
        out.append("```")
        out.append("")

    out.append("## HCN docs")
    out.append("")
    if not hcn_files:
        out.append("(none found in hcs-docs/)")
    for p in hcn_files:
        out.append(f"### `{p.name}`")
        out.append("")
        out.append("```json")
        out.append(_embed_json(p))
        out.append("```")
        out.append("")

    out.append("## HCN state — pre")
    out.append("")
    out.append("```")
    out.append("\n".join(pre_state) if pre_state else "(empty)")
    out.append("```")
    out.append("")
    out.append("## HCN state — post")
    out.append("")
    out.append("```")
    out.append("\n".join(post_state) if post_state else "(empty)")
    out.append("```")
    out.append("")

    out.append("## stdout.log — last 60 lines")
    out.append("")
    tail = _tail(log.splitlines(), 60)
    out.append("```")
    out.append("\n".join(tail) if tail else "(stdout.log missing or empty)")
    out.append("```")
    out.append("")

    report = run_local / "report.md"
    report.write_text("\n".join(out), encoding="utf-8")
    return report


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    ap = argparse.ArgumentParser(
        prog="debug_e2e.py",
        description="Drive one Windows HCS+HCN e2e debug cycle end-to-end.",
    )
    ap.add_argument("--test", default="composite_dispatch_e2e")
    ap.add_argument("--no-rsync", action="store_true")
    ap.add_argument("--no-cleanup", action="store_true")
    ap.add_argument(
        "--clean-deep",
        action="store_true",
        help="Before the run, also nuke stale test scratch dirs + HCS systems "
        "(saves disk; takes ~30s). Idempotent.",
    )
    ap.add_argument("--host", default="MiniWindows@192.168.68.92")
    ap.add_argument("--repo-remote", default="C:/src/ZLayer")
    ap.add_argument("--poll-interval", type=int, default=30)
    ap.add_argument("--timeout-min", type=int, default=30)
    ap.add_argument(
        "--report-dir",
        default=os.environ.get("ZLAYER_DEBUG_REPORT_DIR", "/tmp/zlayer-debug"),
    )
    ap.add_argument(
        "--local-repo",
        default=os.environ.get("ZLAYER_LOCAL_REPO", "/home/zach/github/ZLayer"),
    )
    return ap.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)

    local_repo = Path(args.local_repo).resolve()
    if not local_repo.is_dir():
        die(f"local repo does not exist: {local_repo}")

    report_root = Path(args.report_dir).expanduser().resolve()
    stamp = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H-%M-%S")
    run_local = report_root / f"run-{stamp}"
    run_local.mkdir(parents=True, exist_ok=True)

    console.print(f"[bold]debug_e2e[/] test=[cyan]{args.test}[/] host=[cyan]{args.host}[/]")
    console.print(f"local report dir: [cyan]{run_local}[/]")

    # 1. Rsync
    if args.no_rsync:
        console.print("[yellow]skipping rsync (--no-rsync)[/]")
    else:
        phase_rsync(local_repo, args.host)

    # 2. Cleanup
    if args.clean_deep:
        phase_clean_deep(args.host, args.repo_remote)
    if args.no_cleanup:
        console.print("[yellow]skipping cleanup_hcn (--no-cleanup)[/]")
    else:
        phase_cleanup(args.host, args.repo_remote)

    # 3. Pre-state
    pre_state = phase_check_hcn(args.host, args.repo_remote, "pre")

    # 4. Launch
    started = time.monotonic()
    launch = phase_launch(args.host, args.repo_remote, args.test)
    console.print(
        f"[green]launched[/] PID={launch.pid} RUNDIR={launch.rundir_remote}"
    )

    # 5. Poll
    final_status = phase_poll(
        args.host,
        args.repo_remote,
        args.test,
        args.poll_interval,
        args.timeout_min,
    )
    duration_s = time.monotonic() - started
    rc = final_status.get("RC", "?")

    # 6. Fetch
    phase_fetch(args.host, launch.rundir_remote, run_local)

    # Capture post-state AFTER fetch so test had time to flush
    post_state = phase_check_hcn(args.host, args.repo_remote, "post")

    # 7. Report
    section("7. Generate report.md")
    report_path = write_report(
        run_local,
        test_name=args.test,
        rc=rc,
        duration_s=duration_s,
        pre_state=pre_state,
        post_state=post_state,
        final_status=final_status,
    )
    console.print(f"[green]wrote[/] {report_path}")

    # 8. Summary line
    log = _read_text_safe(run_local / "stdout.log")
    passed, failed, ignored, _ = _result_line(log)
    color = "green" if rc == "0" else "red"
    console.print()
    console.print(
        f"[bold {color}]RESULT:[/] {passed} passed, {failed} failed "
        f"(rc={rc}) — report at {report_path}"
    )
    return 0 if rc == "0" else 1


if __name__ == "__main__":
    sys.exit(main())
