# zlayer-paths

Centralized, cross-platform filesystem path resolution for ZLayer.

## Overview

Every ZLayer crate that needs to know "where do my data files live"
asks `zlayer-paths` instead of hardcoding paths. The crate resolves
the data, run, log, socket, and binary directories for the current
platform and combines them with a fixed subdirectory layout
(`containers/`, `rootfs/`, `bundles/`, `cache/`, `volumes/`, `wasm/`,
`secrets/`, `certs/`, `raft/`, ...). Centralising this means a layout
change only needs to touch one crate, and Linux / macOS / Windows
each get the right defaults without `cfg` blocks scattered through
the workspace.

## Public API

All resolution lives on [`ZLayerDirs`](src/lib.rs):

- [`ZLayerDirs::new(data_dir)`](src/lib.rs) — anchor the layout to
  an explicit data directory.
- [`ZLayerDirs::system_default()`](src/lib.rs) — anchor to the
  platform default.
- [`ZLayerDirs::default_data_dir()`](src/lib.rs) — platform default
  data directory (see *Platform notes*).
- [`ZLayerDirs::detect_data_dir()`](src/lib.rs) — like
  `default_data_dir`, but probes for an existing system-wide
  install (`daemon.json` under `/var/lib/zlayer` on Linux or
  `%ProgramData%\ZLayer` on Windows) and prefers it.
- [`ZLayerDirs::default_run_dir()`](src/lib.rs),
  [`default_log_dir()`](src/lib.rs),
  [`default_socket_path()`](src/lib.rs),
  [`default_docker_socket_path()`](src/lib.rs),
  [`default_binary_dir()`](src/lib.rs) — platform-aware run, log,
  daemon-socket, Docker-API-socket, and binary-install locations.

Subdirectory accessors on a `ZLayerDirs` instance return owned
`PathBuf`s under `data_dir`: `containers()`, `rootfs()`, `bundles()`,
`cache()`, `volumes()`, `wasm()`, `wasm_compiled()`, `secrets()`,
`certs()`, `raft()`, `admin_password()`, `admin_bearer_path()`,
`daemon_json()`, `agent_ipam_state()`, `logs()`, `vms()`, `images()`,
`bin()`, `toolchain_cache()`, `tmp()`.

Free functions:

- [`default_admin_bearer_path()`](src/lib.rs) — convenience for
  `ZLayerDirs::system_default().admin_bearer_path()`.
- [`is_root()`](src/lib.rs) — true when the process has superuser
  (Unix EUID `0`) or Administrator (`IsUserAnAdmin`) rights.

## Use from ZLayer

Almost every workspace crate depends on `zlayer-paths`:

- `bin/zlayer` — resolves the daemon socket, data dir, log dir, and
  admin-bearer file at startup.
- `crates/zlayer-agent` — picks `containers/`, `rootfs/`, `bundles/`,
  and the named-volume directory.
- `crates/zlayer-api` — finds the daemon socket, `daemon.json`, and
  the admin-password / admin-bearer files.
- `crates/zlayer-client` — calls `default_socket_path` and
  `admin_bearer_path` to wire up its transport.
- `crates/zlayer-docker` — calls `default_docker_socket_path` for
  the Docker-API listener.
- `crates/zlayer-secrets`, `crates/zlayer-storage`,
  `crates/zlayer-overlay`, `crates/zlayer-registry`,
  `crates/zlayer-builder`, `crates/zlayer-core`,
  `crates/zlayer-proxy`, `crates/zlayer-scheduler`,
  `crates/zlayer-wsl` — each pulls the relevant subdirectory off
  `ZLayerDirs`.

## Platform notes

Defaults resolved by `default_data_dir()`:

- **Linux** (root): `/var/lib/zlayer`.
- **Linux** (non-root): `~/.zlayer` (falling back to `/tmp/.zlayer`
  if `HOME` is unset).
- **macOS**: `~/.zlayer`.
- **Windows**: `%ProgramData%\ZLayer`, falling back to
  `C:\ProgramData\ZLayer` when `PROGRAMDATA` is unset (HCS-backed
  nodes run as SYSTEM, where the system-wide path is correct).

Daemon socket:

- **Linux**: `/var/run/zlayer.sock`.
- **macOS**: `<data_dir>/run/zlayer.sock`.
- **Windows**: `tcp://127.0.0.1:3669` (no Unix socket, so a loopback
  TCP endpoint is returned instead).

Docker-API socket:

- **Linux** (root): `/var/run/zlayer/docker.sock`.
- **Linux** (non-root): `${XDG_RUNTIME_DIR}/zlayer/docker.sock`,
  falling back to `<data_dir>/run/docker.sock`.
- **macOS**: `<data_dir>/run/docker.sock`.
- **Windows**: the named pipe `\\.\pipe\zlayer-docker`.

Binary install dir (`default_binary_dir`): probes `/usr/local/bin`
for write access on Unix and falls back to `<data_dir>/bin` when
that directory is read-only (e.g. on overlayfs without write
permissions).

Privilege detection (`is_root`) is implemented with `libc::geteuid`
on Unix and `windows::Win32::UI::Shell::IsUserAnAdmin` on Windows.

## When to edit this crate

- You're adding a new on-disk artefact (new data subdirectory, new
  cache, new config file) — add an accessor here so every consumer
  picks it up at once instead of hardcoding the path.
- You're changing where a given platform stores ZLayer state (e.g.
  moving Linux non-root state under `XDG_DATA_HOME`).
- You're adding support for a new platform — extend the `cfg` arms
  in `default_data_dir`, `default_run_dir`, `default_log_dir`,
  `default_socket_path`, `default_docker_socket_path`, and
  `is_root`.
