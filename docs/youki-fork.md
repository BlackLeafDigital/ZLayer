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
- `zlayer-patches` — branched from `main`, carries squashed cherry-picks of open upstream PRs. **This is what `crates/zlayer-agent/Cargo.toml` points at.**

## Current patches

| Patch | Upstream PR | Upstream issue | Status | Notes |
|-------|-------------|----------------|--------|-------|
| Retry cgroup join on EBUSY for nested exec | [#3347](https://github.com/youki-dev/youki/pull/3347) | [#3342](https://github.com/youki-dev/youki/issues/3342) | Open upstream, CI green, unreviewed | Fixes Docker-in-Docker / nested dev container break on cgroup v2 systemd. |

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

# 3. Pick up the new HEAD in ZLayer / zlayer-zql
cd /path/to/ZLayer   # (repeat in zlayer-zql)
cargo update -p libcontainer
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

Add a row to the **Current patches** table above and bump `Cargo.lock` in both ZLayer and zlayer-zql with `cargo update -p libcontainer`.

## Retirement

When every cherry-pick has been upstreamed and we don't need any local patches, delete the `zlayer-patches` branch and point `Cargo.toml` back at `youki-dev/youki` at a specific `rev = ` or (eventually) a plain `version = ` from crates.io. Don't delete the fork repo itself — keeping it around costs nothing and saves setup time if we need to carry patches again.

## Mirror note

Changes to `crates/zlayer-agent/Cargo.toml` in ZLayer must also be applied (as edits, never `cp`) to the same file in `zlayer-zql`. This document must be kept in sync between both repos.
