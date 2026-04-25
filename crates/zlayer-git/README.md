# zlayer-git

Pure-Rust git operations for ZLayer's project, sync, and webhook flows.

## Overview

`zlayer-git` is the in-process git layer used by the ZLayer daemon. It
powers `zlayer project` repository management, `zlayer sync` GitOps
reconciliation, and the git-provider webhook receiver. Every operation is
implemented on top of [`gix`](https://docs.rs/gix) (gitoxide) — the daemon
never shells out to the `git` CLI and never links `libgit2`. Blocking gix
calls are dispatched through `tokio::task::spawn_blocking` so the public
API is async-friendly.

`publish = false`: this is a workspace-internal crate.

## Public API

Top-level git operations (all `async`, all return `anyhow::Result`):

- [`clone_repo(url, branch, auth, dest)`](src/lib.rs) — clone `url` at
  `branch` into a fresh `dest` directory, return the resulting HEAD
  SHA.
- [`fetch(repo_path, auth)`](src/lib.rs) — fetch from `origin` into
  `refs/remotes/origin/*`.
- [`pull_ff(repo_path, branch, auth)`](src/lib.rs) — fetch and
  fast-forward the local branch; bails if the histories have
  diverged. Returns the new HEAD SHA.
- [`rev_parse(repo_path, spec)`](src/lib.rs) — resolve a revision spec
  (branch, tag, partial SHA) to a full object id.
- [`current_sha(repo_path)`](src/lib.rs) — read the current HEAD SHA.
- [`checkout(repo_path, target)`](src/lib.rs) — write the worktree
  for `target` (branch, tag, or SHA) and detach HEAD at the resolved
  object.
- [`ls_remote(url, branch, auth)`](src/lib.rs) — query a remote for
  the SHA of `branch` without cloning. Returns `None` when the branch
  is absent. Used by the per-project git poller to detect new commits.

Authentication is selected by the [`GitAuth`](src/lib.rs) value passed to
each call:

- `GitAuth::anonymous()` — public reads only.
- `GitAuth::pat(username, token)` — HTTPS personal access token.
  Credentials are supplied to gix through an in-memory closure
  installed via `with_credentials`; nothing is written to disk and
  nothing reaches a command line.
- `GitAuth::ssh_key(pem)` — the PEM key is materialised in a
  mode-`0600` `tempfile::TempDir` for the duration of the operation,
  and `GIT_SSH_COMMAND` is pointed at it (with strict host-key
  checking disabled — ZLayer manages peer trust separately). An
  internal mutex serialises SSH operations so concurrent callers
  don't clobber each other's environment.

GitOps sync helpers in the [`sync`](src/sync.rs) submodule:

- [`sync::SyncResource`](src/sync.rs) / [`sync::SyncDiff`](src/sync.rs) —
  resource records and the diff structure.
- [`sync::scan_resources(dir)`](src/sync.rs) — read every `*.yaml` /
  `*.yml` file in `dir` and parse out `(kind, name, content)`.
- [`sync::compute_diff(local, remote_names)`](src/sync.rs) — bucket
  resources into create/update/delete sets.

## Use from ZLayer

- `crates/zlayer-api`:
  - `src/poller.rs` — the per-project background poller calls
    `ls_remote`, `clone_repo`, `pull_ff`, and `current_sha` on the
    interval set by each project's `auto_deploy` /
    `poll_interval_secs`.
  - `src/handlers/webhooks.rs` — the rotatable-secret webhook
    endpoint calls `pull_ff` (or `clone_repo` on first trigger) when a
    push event arrives.
  - `src/handlers/syncs.rs` — `zlayer sync` reconciliation reads
    `SyncResource`/`SyncDiff` from `sync` and pins the applied
    revision via `current_sha`.
  - `src/handlers/projects.rs` — interactive project CRUD reuses the
    same clone/pull paths.

## Platform notes

The PEM-on-disk SSH path is implemented twice with `#[cfg(unix)]` /
`#[cfg(windows)]` arms: the Unix variant sets the file mode to `0o600`
explicitly, the Windows variant relies on inherited ACLs from the
parent `TempDir`. PAT-over-HTTPS auth works identically on every
platform because credentials never touch the filesystem.

## When to edit this crate

- You're adding a new git verb (e.g. tag listing, shallow clone,
  push) — extend the public async API and back it with a new
  `blocking::*` function.
- You need to support an additional auth method (e.g. GitHub App
  installation tokens) — extend `GitAuth` and route it through
  `AuthEnv` / `credentials_for`.
- You're changing the GitOps schema (new resource kind, new YAML
  shape) — update `sync::PartialResource` and the `sync::scan_resources`
  parser.
