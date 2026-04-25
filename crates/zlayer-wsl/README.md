# zlayer-wsl

WSL2 backend integration for ZLayer on Windows.

## Overview

On Windows hosts, ZLayer runs Linux-image workloads inside a dedicated
`zlayer` WSL2 distribution (a full ZLayer daemon delegate). This crate
owns that distro's lifecycle: detection, provisioning, exec, vhdx capacity
management, and the elevated `wsl --install` consent flow. The Windows
`zlayer` CLI talks to it from `zlayer-agent`'s `Wsl2DelegateRuntime`
whenever an HCS-native runtime can't service a request (e.g. Linux images
on a Windows host).

The crate compiles on all platforms but every meaningful function is
Windows-gated; on Linux/macOS the stubs return errors or no-op values so
the rest of the workspace can build cross-platform.

## Platform

Cross-compile clean, Windows-functional. Behind the `wsl` Cargo feature on
both `bin/zlayer` (`wsl = ["dep:zlayer-wsl"]`) and `zlayer-agent`
(`wsl = ["dep:zlayer-wsl", "dep:anyhow"]`). The Windows-only
`ShellExecuteExW` UAC consent path uses `windows = "0.62"` with
`Win32_UI_Shell` / `Win32_System_Threading`.

## Public API

Modules align with the lifecycle stages:

- `zlayer_wsl::detect` — `WslStatus` plus `detect_wsl()` report whether
  WSL2 is installed, available, and which kernel/version is active.
- `zlayer_wsl::setup`
  - `ensure_wsl_backend_ready()` — happy-path one-shot bootstrap.
  - `ensure_wsl_backend_ready_with_consent(...)` — same, but takes a
    consent callback so the caller can drive the proactive
    `wsl --install` UAC prompt (the `--install-wsl ask|yes|no` flag on
    `zlayer join` and `zlayer serve`).
  - `ensure_wsl_backend_ready_with_vhd_gb(...)` — applies the daemon's
    `--vhd-gb` cap when sizing the distro's vhdx.
- `zlayer_wsl::distro`
  - `DISTRO_NAME` constant — `"zlayer"`.
  - `create_distro(rootfs_path, install_dir)` — imports a rootfs tarball.
  - `wsl_exec(cmd, args)` — runs a command inside the `zlayer` distro.
  - `distro_exists()` / `remove_distro()` — idempotent presence and
    teardown.
- `zlayer_wsl::daemon` — `WslBackendConfig`, `start_daemon`,
  `stop_daemon`, `check_daemon_health`, `wait_for_daemon` manage the
  ZLayer daemon process running inside the distro.
- `zlayer_wsl::compact` — `CompactReport`, `CompactMethod`, and
  `compact_distro()` reclaim space from the WSL2 vhdx (the `optimize-vhd`
  / `diskpart` shrink flow).
- `zlayer_wsl::wslconfig`
  - `ensure_wslconfig(min_gb)` writes `~/.wslconfig` with at-least the
    requested vhdx size cap (`WslconfigOutcome` reports what changed).
  - `ensure_mirrored_networking()` flips the experimental
    `networkingMode = mirrored` setting needed by ZLayer's overlay.
  - `compute_default_gb(install_dir)` — picks a reasonable default vhdx
    size from the host disk.
- `zlayer_wsl::paths` — Path translation helpers: `install_dir`,
  `vhdx_path`, `windows_to_wsl`, `wsl_to_windows`, `is_windows_path`.
- `zlayer_wsl::shell` — `wsl_control(args)`, `powershell(script)`,
  `diskpart_script(script)` — the thin process wrappers the rest of the
  crate is built on.
- `zlayer_wsl::errors::WslError` — `thiserror`-based error type covering
  the failure modes (not installed, install reboot required, vhdx
  resize failed, ...).

## Use from ZLayer

- `crates/zlayer-agent/src/runtimes/wsl2_delegate.rs` is the runtime
  consumer. It calls `ensure_wsl_backend_ready*` at startup, then routes
  Linux container requests into the in-distro daemon over the configured
  socket. The agent's selection logic (`crates/zlayer-agent/src/lib.rs`)
  pairs `Wsl2DelegateRuntime` as the delegate for Linux images alongside
  the primary `HcsRuntime` for native Windows containers.
- The `zlayer join` and `zlayer serve` CLI commands surface the proactive
  `--install-wsl ask|yes|no` consent flag and the `--vhd-gb` cap, both of
  which are plumbed into this crate.

This is an internal ZLayer crate. The workspace `version = "0.0.0-dev"` is
unpublished; consume it via the workspace path dependency, not crates.io.

## When to edit this crate

Edit `zlayer-wsl` when you need to:

- Change how the `zlayer` distro is provisioned (rootfs source, image
  layout, init).
- Update the elevated install consent path (UAC prompt, reboot detection).
- Adjust vhdx sizing defaults or the compact/shrink procedure.
- Plumb a new `~/.wslconfig` setting (e.g. another networking mode).
- Add or rename a path-translation helper used by the agent.

The Windows-side daemon-process registration (Service Control Manager,
service installer) lives outside this crate, in the `zlayer` binary.

Repository: <https://github.com/BlackLeafDigital/ZLayer>
