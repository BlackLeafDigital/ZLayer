# zlayer-manager — End-to-End Tests

Intellitester-driven Playwright tests for the manager UI. Tests cover
the auth happy path (bootstrap → logout → login) and a navigation
smoke pass across every protected route.

## What's here

| File | Purpose |
|------|---------|
| `intellitester.config.yaml` | Per-suite config (baseUrl, headless, healing off) |
| `bootstrap.test.yaml` | First-run admin creation against an empty `users.db` |
| `login.test.yaml` | Sign in as the bootstrapped admin |
| `logout.test.yaml` | Open user menu, click "Sign out", land on `/login` |
| `nav-smoke.test.yaml` | Visit every protected route, assert page heading renders |
| `auth.workflow.yaml` | Workflow chaining bootstrap → logout → login in one browser session |
| `stale-session-setup.test.yaml` | Bootstrap admin, log in, persist Playwright storage state to `.stale-session.state.json` |
| `stale-session-verify.test.yaml` | With cookies pre-loaded, hit `/`, assert manager_me's expiry Set-Cookie + post-bounce empty cookie jar |
| `scripts/run-suite.sh` | Hermetic runner — boots daemon + manager, runs all tests, tears down |
| `scripts/reset-data-dir.sh` | Helper used by run-suite.sh to wipe the throwaway data dir |
| `package.json` | Pins intellitester + playwright (npm/pnpm deps) |

## One-time setup

```bash
# Install JS deps (intellitester + playwright)
pnpm install --prefix crates/zlayer-manager/tests/e2e

# Install the Chromium browser playwright uses
pnpm dlx playwright install chromium

# Install cargo-leptos so the manager can build
cargo install cargo-leptos --locked
```

## Run the suite

```bash
# Full suite (default)
crates/zlayer-manager/tests/e2e/scripts/run-suite.sh

# Single test (skip the stale-session sqlite step)
crates/zlayer-manager/tests/e2e/scripts/run-suite.sh --only nav-smoke.test.yaml

# Skip the cargo build phase (useful when target/release is already up to date)
crates/zlayer-manager/tests/e2e/scripts/run-suite.sh --no-build

# Both at once
crates/zlayer-manager/tests/e2e/scripts/run-suite.sh --no-build --only auth.workflow.yaml
```

The script will:

1. Allocate a throwaway data dir under `$TMPDIR/zlayer-e2e-XXXXXX`.
2. Build `zlayer` (release) and `zlayer-manager` (cargo-leptos release).
   Skipped if `--no-build` is passed.
3. Start the daemon on `127.0.0.1:13669` against that data dir, with
   `--deployment-name zlayer-e2e` and isolated WireGuard / DNS ports
   so it doesn't fight a system zlayer install for the canonical
   `zl-zlayer-*` link names or 51420/15353 sockets.
4. Start the manager on `127.0.0.1:16677` pointing at the local daemon.
5. If `--only <test.yaml>` is passed, run exactly that intellitester
   file and exit. Otherwise run `auth.workflow.yaml`, then
   `nav-smoke.test.yaml`, then the stale-session regression below.
6. Restart the stack against a freshly wiped data dir, then run the
   stale-session regression: `stale-session-setup.test.yaml` →
   `sqlite3 ... DELETE FROM users WHERE email='admin@e2e.local'` →
   `stale-session-verify.test.yaml` (loaded with the saved cookies
   via `--storage-state`).
7. Kill both processes and remove the data dir on exit.

### Flags

| Flag | Effect |
|------|--------|
| `--only <test.yaml>` | Run a single intellitester file (e.g. `nav-smoke.test.yaml`) instead of the full suite. Skips the stale-session sqlite step. |
| `--no-build` | Skip the `cargo build --release -p zlayer` + `cargo leptos build --release` phase. Assumes `target/release/zlayer` and `target/release/zlayer-manager` already exist. |
| `-h`, `--help` | Print the usage and exit. |

### Environment overrides

The script picks non-default ports so it can run alongside a stock
zlayer install. Override via env vars:

| Variable | Default | Used for |
|----------|---------|----------|
| `ZLAYER_E2E_API_PORT` | `13669` | Daemon REST API bind port (`zlayer serve --bind`) |
| `ZLAYER_E2E_MANAGER_PORT` | `16677` | Manager (Leptos) bind port |
| `ZLAYER_E2E_WG_PORT` | `51421` | Overlay WireGuard UDP port (`zlayer serve --wg-port`) |
| `ZLAYER_E2E_DNS_PORT` | `15354` | Overlay DNS server port (`zlayer serve --dns-port`) |
| `ZLAYER_JWT_SECRET` | hardcoded test value | Forwarded into the daemon |

CI also forwards these — see `.github/workflows/e2e.yml` and
`.forgejo/workflows/e2e.yml` (`manager-intellitester-tests` job) for
the dispatchable single-test runner.

## Authoring new tests

- Prefer DOM selectors with stable IDs (`#login-email`, `#bs-email`) or
  ARIA role + name (`role: button, name: "Sign in"`). Avoid CSS class
  selectors — DaisyUI swaps them on theme tweaks.
- Set `healing.enabled: true` in `intellitester.config.yaml` and export
  `OPENROUTER_API_KEY` while iterating, so broken selectors get
  AI-rewritten on the fly. Turn healing back off before committing.
- Page-level headings are guaranteed to render on every protected
  route (`waitForSelector { role: heading }`) — that's the cheapest
  "page didn't crash" signal.

## Stale-session regression

The stale-session cleanup from `CHANGELOG.md 0.11.22` is covered by
`stale-session-{setup,verify}.test.yaml` plus a `sqlite3` step in
`run-suite.sh`. The setup test bootstraps an admin, logs in, and dumps
the Playwright storage state (including the HttpOnly `zlayer_session`
JWT) to `.stale-session.state.json`. The shell wrapper deletes the
admin row from `users.db`. The verify test runs with
`--storage-state .stale-session.state.json`, makes one navigation to
`/`, and asserts two things:

- **Network layer** — `expectResponse` catches the
  `/api/manager/manager_me` response and asserts its `set-cookie`
  header contains `zlayer_session=; ...; Max-Age=0`. This proves the
  manager hit `/auth/logout` and propagated its expiry headers.
- **Browser jar** — `assertCookies` (which calls
  Playwright's `context.cookies()` and therefore sees HttpOnly
  cookies) asserts both `zlayer_session` and `zlayer_csrf` are absent
  after the bounce.

Either assertion failing localizes the regression: missing
`Set-Cookie` means the manager skipped the logout probe; the
assertion landing but the jar still holding cookies means propagation
broke between `forward_raw` and the Leptos response.
