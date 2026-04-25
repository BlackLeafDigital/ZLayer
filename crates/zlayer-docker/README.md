# zlayer-docker

Docker CLI, Compose, and API socket compatibility layer for ZLayer.

## Overview

This crate makes existing Docker tooling work against a ZLayer daemon. It
ships three things:

1. A `docker`-compatible CLI (`zlayer docker ...`) that mirrors the
   subcommands developers and CI scripts already use.
2. A subset of the Docker Engine HTTP API served on a Unix socket
   (Linux/macOS) or named pipe (Windows), so anything that speaks
   `DOCKER_HOST` — the Go SDK, `testcontainers`, BuildKit, the IDE
   integrations — sees ZLayer as a Docker daemon.
3. A `docker-compose` translator that converts a Compose file into a
   native ZLayer `DeploymentSpec` and applies it through the same daemon
   API the rest of ZLayer uses.

The goal is "drop your existing Docker workflow on top of ZLayer with one
command" — `zlayer docker install`.

## Platform

Cross-platform. Linux, macOS, and Windows all build and run the crate.

- Unix: Engine API listens on a Unix domain socket. Default path is the
  one returned by `zlayer_paths::ZLayerDirs::default_docker_socket_path()`.
  An optional `--replace-docker-sock` install step can symlink
  `/var/run/docker.sock` so root-owned tooling that hardcodes that path
  works unchanged.
- Windows: Engine API listens on the named pipe `\\.\pipe\zlayer-docker`
  via `tokio::net::windows::named_pipe::ServerOptions`. `DOCKER_HOST` is
  written as `npipe:////./pipe/zlayer-docker`.

The `--features docker-compat` flag on `bin/zlayer`
(`docker-compat = ["dep:zlayer-docker"]`) gates this crate into the main
`zlayer` binary; it's part of the `full` feature set.

## Public API

The crate root re-exports the entry points most callers need:

- `zlayer_docker::cli::DockerCommands` — the clap enum of every
  subcommand.
- `zlayer_docker::cli::handle_docker_command(cmd)` — the dispatcher the
  `zlayer docker` subcommand calls into.
- `zlayer_docker::compose::{ComposeFile, parse_compose}` — Compose YAML
  loader.
- `zlayer_docker::compose::convert::compose_to_deployment(compose, name)` —
  translates a parsed `ComposeFile` into a ZLayer `DeploymentSpec`.
- `zlayer_docker::shim::{install_shim, uninstall_shim, ShimInstalled, ShimUninstalled}`
  — drops or removes a `docker` / `docker-compose` shim binary on `PATH`
  that re-execs `zlayer docker ...`.
- `zlayer_docker::socket::serve(socket_path)` — boots the Engine API
  server. Internally dispatches to `socket::listener_unix::serve_on` or
  `socket::listener_windows::serve_on` depending on platform.
- `zlayer_docker::{DockerError, Result}` — error type covering compose
  parse, conversion, unsupported-feature, IO, YAML, and `zlayer-spec`
  failures.

The Engine API surface is grouped by Docker resource:
`socket::containers`, `socket::images`, `socket::networks`,
`socket::volumes`, `socket::system`, `socket::types`. New endpoints land
under the matching module.

### Subcommands

`zlayer docker` covers, end-to-end:

- Container lifecycle: `run`, `ps`, `start`, `stop`, `restart`, `kill`,
  `rm`, `exec`, `logs`, `inspect`, `cp`, `stats`.
- Images: `build`, `pull`, `push`, `images`, `rmi`, `tag`, `login`,
  `logout`.
- Resources: `volume`, `network`.
- Compose: `compose` (sub-app delegating to `compose_cmd`).
- Daemon plumbing: `system` (info, df, prune, ...).
- Setup: `install`, `uninstall`.

### Install / uninstall

`zlayer docker install` (`cli::install::InstallArgs`) is the one-shot setup
flow. It:

1. Reconfigures the zlayer daemon service (systemd / launchd / Windows
   SCM) to expose the Docker Engine API socket.
2. Drops `docker` and `docker-compose` shims on `PATH` that re-exec
   `zlayer docker ...`.
3. Writes `DOCKER_HOST` and `DOCKER_BUILDKIT=0` to the user's shell
   profile (Unix) or user environment registry (Windows).
4. Optionally offers to symlink `/var/run/docker.sock` to the ZLayer
   Docker socket (Unix only — Windows named pipes can't be symlinked).

`zlayer docker uninstall` reverses every step and restores any backed-up
files. Both subcommands honor opt-out flags (`--no-shim`, `--no-env`,
`--no-daemon-restart`, `--skip-symlink-prompt`, `--force`,
`--socket-path`, `--replace-docker-sock`).

### Compose translation

`compose/convert.rs` walks the parsed `ComposeFile` and emits a single
ZLayer `DeploymentSpec`: each compose service becomes a ZLayer service,
volumes/networks become deployment-level resources, healthchecks /
restart policy / ports are mapped onto the ZLayer equivalents. Unsupported
features fail loudly with `DockerError::Unsupported(...)` rather than
being silently dropped.

## Feature flags

- `default = ["build"]`
- `build = ["dep:zlayer-builder"]` — pulls in `zlayer-builder` so
  `zlayer docker build` can drive a real OCI build.
- `socket = []` — historical; the socket modules now compile
  unconditionally because `bin/zlayer` always wants the API server when
  `docker-compat` is on.

## Use from ZLayer

`bin/zlayer/Cargo.toml` enables `docker-compat` in the `full` feature set
so the published `zlayer` binary ships with the Docker shim built in.
The daemon side (`zlayer serve`) bootstraps `socket::serve(...)` against
the configured socket / pipe path; the CLI side wires `DockerCommands`
into the top-level `zlayer docker ...` subcommand.

This is an internal ZLayer crate (`publish = false` in `Cargo.toml`).
Consume it via the workspace path dependency, not crates.io.

## When to edit this crate

Edit `zlayer-docker` when you need to:

- Add or fix a `zlayer docker` subcommand (`cli/*.rs`).
- Add a Docker Engine API endpoint (`socket/*.rs`).
- Extend Compose support — new YAML fields go in `compose/types.rs`,
  translation logic in `compose/convert.rs`.
- Change the install / uninstall flow on a specific OS
  (`cli/install_linux.rs`, `cli/install_macos.rs`,
  `cli/install_windows.rs`) or the shim binaries (`shim.rs`).
- Adjust how `DOCKER_HOST` and friends are written into the user's shell
  profile (`cli/env_profile.rs`, `cli/env_profile_windows.rs`).

Repository: <https://github.com/BlackLeafDigital/ZLayer>
