# ZLayer's youki fork

## Why we fork

ZLayer consumes `libcontainer` from [youki-dev/youki](https://github.com/youki-dev/youki) as its in-process Linux container runtime. Upstream releases slowly:

- 0.6.0 shipped 2026-02-25.
- [PR #3440](https://github.com/youki-dev/youki/pull/3440) ("Release for v0.6.1") has been sitting `MERGEABLE` with zero reviewers and zero comments since 2026-03-02 — a single-click release that no maintainer has clicked in 7+ weeks at the time of writing.
- Individual bug-fix PRs we need (e.g. [#3347](https://github.com/youki-dev/youki/pull/3347) for the cgroup-v2 nested-exec / DinD break) sit open for weeks after receiving approval + green CI.

Rather than pin specific upstream commits one at a time and wait on maintainers for each merge, we maintain a thin fork with a patches branch that tracks upstream `main` plus cherry-picks of open PRs.

## Fork location

`https://github.com/ZachHandley/youki`

## Branch semantics

- `main` — fast-forward mirror of `youki-dev/youki@main`. Synced periodically.
- `zlayer-patches` — branched from `main`, carries squashed cherry-picks of open upstream PRs plus the `[package].name` rename to `zlayer-libcgroups` / `zlayer-libcontainer` so they can be published to crates.io. **This is what `.github/workflows/publish-zlayer-fork.yml` publishes from.**

## Current patches

| Patch | Upstream PR | Upstream issue | Status | Notes |
|-------|-------------|----------------|--------|-------|
| Retry cgroup join on EBUSY for nested exec | [#3347](https://github.com/youki-dev/youki/pull/3347) | [#3342](https://github.com/youki-dev/youki/issues/3342) | Open upstream, CI green, unreviewed | Fixes Docker-in-Docker / nested dev container break on cgroup v2 systemd. |

## Distribution

The fork is published to crates.io as **two renamed crates**:

- [`zlayer-libcgroups`](https://crates.io/crates/zlayer-libcgroups)
- [`zlayer-libcontainer`](https://crates.io/crates/zlayer-libcontainer)

Versioning is `0.6.1-zlayer.N` — the base tracks upstream's planned 0.6.1 release, the suffix auto-increments per publish.

`crates/zlayer-agent/Cargo.toml` consumes them via Cargo's `package =` rename so the Rust import paths stay unchanged:

```toml
[target.'cfg(target_os = "linux")'.dependencies]
libcontainer = { package = "zlayer-libcontainer", version = "0.6.1-zlayer.1", optional = true }
```

Why renamed (not a git override): a `git = ...` dep is stripped by `cargo publish` and replaced with the plain `version =`, so downstream consumers installing `zlayer-agent` from crates.io would resolve the **unpatched** upstream `libcontainer` from crates.io — silently losing every patch on `zlayer-patches`. Renaming to `zlayer-libcontainer` puts the patched code in crates.io directly.

## Publish workflow

`.github/workflows/publish-zlayer-fork.yml` (in the fork repo) handles publishes automatically:

- **Triggers:** push to `zlayer-patches` (when files under `crates/libcgroups/`, `crates/libcontainer/`, or the workflow itself change), or manual `workflow_dispatch` with optional `force: true` to re-publish HEAD.
- **Auto-bump:** queries crates.io for the highest existing `-zlayer.N` across both crates, picks `max + 1`, applies the new version with `sed` in-CI (never committed back to the fork).
- **Publish order:** `zlayer-libcgroups` first, 45 s wait for index propagation, then `zlayer-libcontainer` (with one auto-retry).
- **Tagging:** on success, pushes `published/zlayer-libcgroups-<v>` and `published/zlayer-libcontainer-<v>` tags pointing at HEAD. The skip-if-tagged guard at the top of the workflow uses these tags as the audit trail.
- **Secret:** the fork repo needs `CARGO_REGISTRY_TOKEN` set with publish scope on both crates.

## Sync procedure

Run when you want to pick up new upstream commits or a new cherry-pick:

```bash
cd ~/github/youki  # or wherever the fork is cloned

# 1. Sync `main` with upstream
git fetch upstream
git checkout main
git merge --ff-only upstream/main
git push

# 2. Rebase `zlayer-patches` on top of the new `main`
git checkout zlayer-patches
git rebase main
# If a carried patch got merged upstream, its cherry-pick commit will drop
# out during rebase as a no-op. Remove the corresponding row from the table
# above when that happens.
git push --force-with-lease
# Pushing automatically triggers .github/workflows/publish-zlayer-fork.yml
# which bumps -zlayer.N and publishes both crates to crates.io. Watch the
# run at https://github.com/ZachHandley/youki/actions and grab the new
# version from the workflow summary.

# 3. Pick up the new published version in ZLayer / zlayer-zql
cd /path/to/ZLayer   # (repeat in zlayer-zql)
# Bump the `version =` field in crates/zlayer-agent/Cargo.toml to match the
# new -zlayer.N from the workflow summary, then:
cargo update -p zlayer-libcontainer -p zlayer-libcgroups
cargo check --workspace
```

## Adding a new patch

To carry another open upstream PR:

```bash
cd ~/github/youki
git checkout zlayer-patches
git fetch https://github.com/<pr-author>/youki.git <pr-branch>
git merge --squash FETCH_HEAD
git commit -m "cherry-pick youki-dev/youki#<N>: <short description>

Squashed import of PR #<N> (currently OPEN upstream).
Fixes youki-dev/youki#<issue>.

Carried locally until upstream merges."
git push --force-with-lease  # or plain push if no rebase needed
```

Add a row to the **Current patches** table above. The push to `zlayer-patches` triggers the publish workflow automatically; once it finishes, bump the `version =` field in `crates/zlayer-agent/Cargo.toml` to the new `-zlayer.N` (in both ZLayer and zlayer-zql) and run `cargo update -p zlayer-libcontainer -p zlayer-libcgroups`.

## Retirement

When every cherry-pick has been upstreamed and the fixes are available in a published upstream `libcontainer` (e.g. 0.6.1 finally ships):

1. Edit `crates/zlayer-agent/Cargo.toml` (in both ZLayer and zlayer-zql) — drop the `package =` rename and depend on upstream directly: `libcontainer = { version = "0.6.1", optional = true }`.
2. `cargo update -p libcontainer` and run the workspace check.
3. Disable `.github/workflows/publish-zlayer-fork.yml` on the fork (`gh workflow disable` or delete the file on `zlayer-patches`).
4. Delete the `zlayer-patches` branch (the `published/*` tags stay as audit trail).
5. Optionally `cargo yank` the `zlayer-libcgroups` / `zlayer-libcontainer` versions on crates.io — leaving them yanked-but-listed is fine; consumers get a clear "this version is yanked, use upstream `libcontainer` instead" message.

Don't delete the fork repo itself — keeping it around costs nothing and saves setup time if we need to carry patches again.

## Mirror note

Changes to `crates/zlayer-agent/Cargo.toml` in ZLayer must also be applied (as edits, never `cp`) to the same file in `zlayer-zql`. This document must be kept in sync between both repos.
