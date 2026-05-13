# zlayer-manager — End-to-End Tests

Intellitester-driven Playwright tests for the manager UI. Tests cover
the login happy path, a navigation smoke pass across every protected
route, and a stale-session regression that exercises cookie clearing
when the underlying user is deleted server-side.

The harness defaults to running against your **locally-installed**
zlayer daemon: it creates a scoped fixture admin via `zlayer user
create` over the daemon's Unix socket (which auto-authenticates the
CLI as `local-admin`), drives the suite, and deletes the fixture on
exit. Pass `--throwaway` to spin up an isolated daemon instead.

## What's here

| File | Purpose |
|------|---------|
| `intellitester.config.yaml` | Per-suite config (baseUrl, headless, healing off) |
| `login.test.yaml` | Sign in as the fixture admin, land on dashboard |
| `nav-smoke.test.yaml` | Visit every protected route, assert page heading renders |
| `stale-session-setup.test.yaml` | Log in as the fixture admin, persist Playwright storage state to `.stale-session.state.json` |
| `stale-session-verify.test.yaml` | With cookies pre-loaded, hit `/`, assert manager_me's expiry `Set-Cookie` + post-bounce empty cookie jar + redirect to `/login` |
| `scripts/run-suite.py` | Hermetic runner — creates fixture user, boots manager, runs suite, tears down |
| `package.json` | Pins intellitester + playwright (npm/pnpm deps) |

## One-time setup

```bash
# Install daisyui (tailwind v4 plugin) for the manager crate.
# Without this, `cargo leptos build` fails at the tailwind step with
# "Error: Can't resolve 'daisyui' in '.../crates/zlayer-manager'".
pnpm install --prefix crates/zlayer-manager

# Install JS deps (intellitester + playwright)
pnpm install --prefix crates/zlayer-manager/tests/e2e

# Install the Chromium browser playwright uses
pnpm dlx playwright install chromium

# Install cargo-leptos so the manager can build
cargo install cargo-leptos --locked

# Install uv (managed by mise locally; `astral-sh/setup-uv` in CI).
# The harness is stdlib-only Python invoked via `uv run`.
mise install uv@latest

# Make sure the daemon is running (skip if you'll pass --throwaway)
zlayer daemon install     # one-time
# or just: zlayer serve --daemon
```

## Run the suite

```bash
# Default — against your locally-installed daemon
uv run crates/zlayer-manager/tests/e2e/scripts/run-suite.py

# Isolated daemon (clean CI, or to keep prod state untouched).
# The throwaway data dir lands at target/zlayer-e2e/<suite-id>/ (gitignored).
# Runs unprivileged — overlay init logs a CAP_NET_ADMIN warning and
# degrades to "cross-node networking disabled", which is harmless for
# the auth/manager flow the suite exercises.
uv run crates/zlayer-manager/tests/e2e/scripts/run-suite.py --throwaway

# Escape hatch for environments where the daemon ever starts requiring
# CAP_NET_ADMIN at startup. Only the daemon subprocess elevates — the
# harness, manager, pnpm, and target/ stay user-owned.
uv run crates/zlayer-manager/tests/e2e/scripts/run-suite.py --throwaway --sudo-daemon

# Single test (skips the stale-session regression)
uv run crates/zlayer-manager/tests/e2e/scripts/run-suite.py --only nav-smoke.test.yaml

# Skip the cargo build phase (useful when target/release is already up to date)
uv run crates/zlayer-manager/tests/e2e/scripts/run-suite.py --no-build
```

The default-mode flow:

1. Resolve the host daemon socket — honors `$ZLAYER_SOCKET`, then
   probes `/var/run/zlayer.sock` and `~/.zlayer/run/zlayer.sock`.
   Exits with a clear message if no socket is found.
2. Build `zlayer-manager` (cargo-leptos release). Skips the `zlayer`
   build if it's already on `$PATH` (it almost always is — you have a
   daemon installed). Skipped entirely with `--no-build`.
3. Create a scoped fixture admin via `zlayer user create
   --email e2e-<suite_id>@test.local --role admin`. Parses the user
   id from the success line.
4. Start the manager on `127.0.0.1:16677` with `ZLAYER_SOCKET=<host
   socket>` so the manager picks up the same Unix-socket auto-admin.
5. Run `login.test.yaml`, then `nav-smoke.test.yaml`.
6. Stale-session regression: `stale-session-setup.test.yaml` (login
   + saveStorageState) → `zlayer user delete <fixture_id> --yes` →
   `stale-session-verify.test.yaml` (loaded with the saved cookies
   via `--storage-state`).
7. On exit (success or failure), kill the manager and delete the
   fixture user via the CLI. `zlayer user delete` is idempotent on
   the daemon side, so the trap is safe to fire even if the
   stale-session step already deleted the row.

`--throwaway` mode is the same flow but launches a temporary daemon
under `target/zlayer-e2e/<suite-id>/` first, then proceeds identically
(fixture user via the same CLI, same login-based suite). Tears the
daemon + data dir down on exit. `target/` is gitignored so the
working-tree footprint is zero; `cargo clean` sweeps any orphans.

### Flags

| Flag | Effect |
|------|--------|
| `--throwaway` | Spin up an isolated daemon instead of using the host daemon. Equivalent to `ZLAYER_E2E_THROWAWAY=1`. |
| `--sudo-daemon` | Wrap the throwaway daemon subprocess in `sudo -E env PATH=... HOME=...`. Only the daemon is elevated. No-op when EUID is already 0. Equivalent to `ZLAYER_E2E_SUDO_DAEMON=1`. |
| `--only <test.yaml>` | Run a single intellitester file instead of the full suite. Skips the stale-session regression. |
| `--no-build` | Skip the manager (and `zlayer`, in `--throwaway`) build phase. |
| `-h`, `--help` | Print the usage and exit. |

### Environment overrides

| Variable | Default | Used for |
|----------|---------|----------|
| `ZLAYER_SOCKET` | (auto-probed) | Daemon Unix socket. Default mode probes `/var/run/zlayer.sock` then `~/.zlayer/run/zlayer.sock` — override if your daemon lives elsewhere. |
| `ZLAYER_E2E_MANAGER_PORT` | `16677` | Manager (Leptos) bind port. |
| `ZLAYER_E2E_API_PORT` | `13669` | **Throwaway only** — daemon REST API bind port. |
| `ZLAYER_E2E_WG_PORT` | `51421` | **Throwaway only** — overlay WireGuard UDP port. |
| `ZLAYER_E2E_DNS_PORT` | `15354` | **Throwaway only** — overlay DNS server port. |
| `ZLAYER_E2E_THROWAWAY` | unset | Same as passing `--throwaway`. |
| `ZLAYER_E2E_SUDO_DAEMON` | unset | Same as passing `--sudo-daemon`. |

## Authoring new tests

- Prefer DOM selectors with stable IDs (`#login-email`,
  `#login-password`) or ARIA role + name (`role: button, name:
  "Sign in"`). Avoid CSS class selectors — DaisyUI swaps them on
  theme tweaks.
- Reference the fixture creds with `${ADMIN_EMAIL}` /
  `${ADMIN_PASSWORD}` — the harness exports them before launching
  intellitester. The `variables:` block in each yaml provides static
  defaults so the file is also runnable by hand against a daemon
  with `admin@e2e.local` pre-created.
- Page-level headings are guaranteed to render on every protected
  route (`waitForSelector { role: heading }`) — that's the cheapest
  "page didn't crash" signal.

## Stale-session regression

The cleanup landed in `CHANGELOG.md 0.11.22`. The setup test logs in
as the fixture admin and dumps the Playwright storage state
(including the HttpOnly `zlayer_session` JWT) to
`.stale-session.state.json`. The harness then deletes the fixture
user via `zlayer user delete <id> --yes`. The verify test runs with
`--storage-state .stale-session.state.json`, makes one navigation to
`/`, and asserts three things:

- **Network layer** — `expectResponse` catches the
  `/api/manager/manager_me` response and asserts its `set-cookie`
  header contains `zlayer_session=; …; Max-Age=0`. Proves the manager
  hit `/auth/logout` and propagated its expiry headers.
- **Browser jar** — `assertCookies` (which calls Playwright's
  `context.cookies()` and therefore sees HttpOnly cookies) asserts
  both `zlayer_session` and `zlayer_csrf` are absent after the bounce.
- **Redirect target** — `waitForSelector #login-email` confirms
  AuthGuard saw `user: None` and routed us to `/login`. (Other users
  still exist in the daemon's user table — only the fixture was
  deleted — so the redirect is `/login`, not `/bootstrap`.)

Any of the three failing localizes the regression: missing
`Set-Cookie` means the manager skipped the logout probe; the
assertion landing but the jar still holding cookies means propagation
broke between `forward_raw` and the Leptos response; the wrong
redirect target means AuthGuard misread the post-401 state.
