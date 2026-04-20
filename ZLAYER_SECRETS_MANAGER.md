# ZLayer Secrets Manager — Bootstrap Auth Env-Var Plan

Explicit plan for the work needed to support **headless / non-interactive
creation of the initial Manager admin** via environment variables. Covers the
gap where the current design is browser-form-only (`/auth/bootstrap`), which
makes CI, IaC, Kubernetes, Helm, and any automated deploy unsafe: whoever
hits the Manager URL first becomes root.

## 1. Current state (as of 0.10.104, working tree)

- **Secrets store**: `PersistentSecretsStore` + `CredentialStore` in
  `crates/zlayer-secrets/`. Encrypted sqlite at `{data_dir}/secrets.sqlite`.
  `CredentialStore::create_api_key(api_key, password, roles)` and
  `CredentialStore::ensure_admin(api_key, password)` (idempotent — skips if
  present, returns `true` on new create) at
  `crates/zlayer-secrets/src/credentials.rs:132` and `:198`.
- **User store** (separate): `SqlxUserStore` (trait `UserStorage`) in
  `crates/zlayer-api/src/storage/users.rs:19-230`. SQLite file `users.db`
  opened via `StorageBundle::open(Some(&data_dir))` in
  `bin/zlayer/src/commands/serve.rs:1193`. `count()` at `users.rs:64` is
  used by the bootstrap handler to gate "first-run" behavior. `store()` at
  `users.rs:137` is UPSERT by id.
- **Manager bootstrap handler**: `zlayer_api::handlers::auth::bootstrap`
  at `crates/zlayer-api/src/handlers/auth.rs:174-221`, mounted at
  `POST /auth/bootstrap`. Body is
  `BootstrapRequest { email, password, display_name }` (auth.rs:115-121).
  Does four things in order:
    1. `user_store.count()` → must be `0`, else `409 Conflict`.
    2. Build `StoredUser::new(email_lc, display_name, UserRole::Admin)`.
    3. `user_store.store(&user).await`.
    4. `cred_store.create_api_key(&email_lc, &password, &["admin"])` (Argon2id).
    5. `issue_session(...)` to return the session cookie + CSRF token.
- **Daemon startup**: `bin/zlayer/src/commands/serve.rs:500` →
  `init_daemon()` at `bin/zlayer/src/daemon.rs:508-606`.
  - `init_daemon` already has a "Phase 11: Credential store + admin
    bootstrap" block that runs `credential_store.ensure_admin("admin",
    generate_admin_password())` and writes the one-shot password to
    `{data_dir}/admin_password` (0o644). **This only seeds the
    `CredentialStore` "admin" API-key**; it does NOT create a
    corresponding `StoredUser` row, so the Manager login flow can't use
    it. That existing block stays as-is for non-Manager API access.
  - `StorageBundle::open` (which creates `users.db`) runs later in
    `serve.rs:1193`, inside `serve()` after `init_daemon` returns.
- **ZImagefile for the Manager image**:
  `images/ZImagefile.zlayer-manager` env block (lines 103-114) — only
  Leptos + addr + log-level vars. No auth knobs.
- **Deployment spec**: emitted by `zlayer manager init` at
  `bin/zlayer/src/commands/manager.rs:37-64`. The generated
  `manager.zlayer.yml` has **no `env:` block**. Must be edited to add one.
- **Env-var idiom in the codebase**: direct `std::env::var(...)` in
  `serve.rs` (see S3 reads at `serve.rs:523-530`) or `clap`'s
  `#[arg(long, env = "ZLAYER_FOO")]` in `cli.rs` (e.g. line 509-510 for
  `ZLAYER_JWT_SECRET`). No `envy`/`figment`/`config` crate — don't
  introduce one.

**Mirror reminder**: every code edit below also lands in
`/var/home/zach/github/zlayer-zql` (same crate names, same paths). UI
crates (`zlayer-manager`, `zlayer-web`, `zlayer-desktop`) are not mirrored,
but `zlayer-api`, `zlayer-secrets`, and `bin/zlayer` ARE.

## 2. Env vars to add

| Var | Required | Notes |
|---|---|---|
| `ZLAYER_BOOTSTRAP_EMAIL` | if bootstrap desired | RFC-5321 lowercased before use |
| `ZLAYER_BOOTSTRAP_PASSWORD` | mutually exclusive with `_FILE` | string; warned if <12 chars |
| `ZLAYER_BOOTSTRAP_PASSWORD_FILE` | mutually exclusive with above | path readable by daemon user, trimmed of trailing `\n` |
| `ZLAYER_BOOTSTRAP_DISPLAY_NAME` | optional | defaults to the email local-part |

**Semantics**:
- If the user table is empty AND `ZLAYER_BOOTSTRAP_EMAIL` is set AND a
  password source is present → create the admin before the HTTP listener
  accepts.
- If the user table is non-empty → all four vars are no-ops (idempotent).
  Log one INFO line and continue.
- If the user table is empty AND the envs are NOT set → log a WARN with
  `"no bootstrap credentials configured — first HTTP client to reach
  /auth/bootstrap will become admin"`. Don't block startup.
- If email is set but no password source → **fail startup** with a clear
  error. Half-configured is worse than unset.
- Both `ZLAYER_BOOTSTRAP_PASSWORD` and `_PASSWORD_FILE` set → fail startup.
- `_PASSWORD_FILE` unreadable / empty → fail startup.

**Why not a flag on `zlayer serve`?** The Manager is usually deployed as a
container via `zlayer deploy`. Env vars are the universal contract across
docker-compose, Kubernetes Secrets, HashiCorp Vault Agent, Doppler, SOPS,
etc. Flags don't flow through those layers.

## 3. Code changes

### 3a. New module: `bin/zlayer/src/bootstrap_admin.rs`

Single public function that runs after both stores exist. Idempotent.

```rust
// bin/zlayer/src/bootstrap_admin.rs
use anyhow::{bail, Context, Result};
use std::sync::Arc;
use tracing::{info, warn};
use zlayer_api::storage::users::{StoredUser, UserRole, UserStorage};
use zlayer_secrets::{CredentialStore, PersistentSecretsStore};

const ENV_EMAIL: &str = "ZLAYER_BOOTSTRAP_EMAIL";
const ENV_PASSWORD: &str = "ZLAYER_BOOTSTRAP_PASSWORD";
const ENV_PASSWORD_FILE: &str = "ZLAYER_BOOTSTRAP_PASSWORD_FILE";
const ENV_DISPLAY_NAME: &str = "ZLAYER_BOOTSTRAP_DISPLAY_NAME";

/// Runs **before** the HTTP listener starts accepting. Safe to call on
/// every startup — idempotent if the users table is non-empty or if envs
/// are unset.
pub async fn maybe_bootstrap_admin(
    users: &Arc<dyn UserStorage>,
    credentials: &Arc<CredentialStore<Arc<PersistentSecretsStore>>>,
) -> Result<()> {
    let count = users.count().await.context("user store count failed")?;
    let email = std::env::var(ENV_EMAIL).ok();

    if count > 0 {
        if email.is_some() {
            info!("users table non-empty; ignoring {ENV_EMAIL} (already bootstrapped)");
        }
        return Ok(());
    }

    let Some(email) = email else {
        warn!(
            "no bootstrap credentials configured — first HTTP client to \
             reach /auth/bootstrap will become admin. Set \
             ZLAYER_BOOTSTRAP_EMAIL + ZLAYER_BOOTSTRAP_PASSWORD(_FILE) to \
             provision non-interactively."
        );
        return Ok(());
    };

    let password = load_password()?;
    if password.len() < 12 {
        warn!("{ENV_PASSWORD}{{,_FILE}} is shorter than 12 chars — strongly consider longer");
    }

    let email_lc = email.trim().to_lowercase();
    let display_name = std::env::var(ENV_DISPLAY_NAME)
        .ok()
        .or_else(|| email_lc.split('@').next().map(str::to_owned))
        .unwrap_or_else(|| "admin".to_string());

    let user = StoredUser::new(email_lc.clone(), display_name, UserRole::Admin);
    users.store(&user).await.context("user_store.store failed")?;

    credentials
        .create_api_key(&email_lc, &password, &[UserRole::Admin.as_str()])
        .await
        .context("credential_store.create_api_key failed")?;

    info!(email = %email_lc, "bootstrap admin created from env");
    Ok(())
}

fn load_password() -> Result<String> {
    let inline = std::env::var(ENV_PASSWORD).ok();
    let path = std::env::var(ENV_PASSWORD_FILE).ok();
    match (inline, path) {
        (Some(_), Some(_)) => bail!(
            "{ENV_PASSWORD} and {ENV_PASSWORD_FILE} are mutually exclusive"
        ),
        (Some(p), None) if !p.is_empty() => Ok(p),
        (Some(_), None) => bail!("{ENV_PASSWORD} is set but empty"),
        (None, Some(path)) => {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("read {ENV_PASSWORD_FILE}={path}"))?;
            let trimmed = raw.trim_end_matches(&['\n', '\r'][..]).to_owned();
            if trimmed.is_empty() {
                bail!("{ENV_PASSWORD_FILE}={path} is empty");
            }
            Ok(trimmed)
        }
        (None, None) => bail!(
            "{ENV_EMAIL} is set but neither {ENV_PASSWORD} nor {ENV_PASSWORD_FILE} is"
        ),
    }
}
```

Add `mod bootstrap_admin;` to `bin/zlayer/src/main.rs` (or wherever modules
are declared — search for the existing `mod daemon;` declaration).

### 3b. Wire the call site in `bin/zlayer/src/commands/serve.rs`

After `StorageBundle::open` returns (currently at `serve.rs:1193`) and
before `zlayer_api::serve_bound(...)` (currently at `serve.rs:1052`).
Both stores must be in scope.

```rust
// Bounded region: around serve.rs:1193 — after StorageBundle::open, before
// the API router is built / serve_bound is called.
let storage_bundle = StorageBundle::open(Some(&data_dir)).await?;

// NEW: bootstrap admin from env if users table is empty
crate::bootstrap_admin::maybe_bootstrap_admin(
    &storage_bundle.users,
    &credential_store, // already in scope from init_daemon return
)
.await?;
```

Exact line numbers will drift — anchor on `StorageBundle::open` and the
first `zlayer_api::serve_bound` call, not on absolute lines.

### 3c. Nothing changes in `zlayer-secrets`

`CredentialStore::create_api_key` is already the right primitive. Do NOT
add env-var reading to this crate — it's a pure library. All env reading
stays in `bin/zlayer`.

### 3d. Nothing changes in `zlayer-api::handlers::auth::bootstrap`

The `POST /auth/bootstrap` handler already correctly `409`s when the
table is non-empty. After our env bootstrap runs, the table is
non-empty, so the browser flow naturally refuses to re-bootstrap. No
handler edits needed.

### 3e. Optional: log a startup banner summarizing auth state

In `init_daemon` or `serve` right before `serve_bound`, log once:

```
auth: users.count()=N, credentials store opened, bootstrap={env|browser|done}
```

Makes `journalctl -u zlayer` diagnosis trivial.

## 4. YAML changes

### 4a. `images/ZImagefile.zlayer-manager`

Add an optional, commented-out block documenting the contract. Do NOT
hardcode values.

```yaml
    env:
      ZLAYER_MANAGER_ADDR: "0.0.0.0:6677"
      LEPTOS_SITE_ADDR: "0.0.0.0:6677"
      LEPTOS_SITE_ROOT: "/app/target/site"
      LEPTOS_OUTPUT_NAME: "zlayer-manager"
      LEPTOS_SITE_PKG_DIR: "pkg"
      LEPTOS_HASH_FILES: "true"
      RUST_LOG: "info"
      # --- Optional: non-interactive admin bootstrap --------------------
      # Set at deploy time (docker-compose env, k8s Secret, etc.), NOT in
      # the image. These are only read when the users table is empty;
      # subsequent restarts ignore them.
      #
      # ZLAYER_BOOTSTRAP_EMAIL: "admin@example.com"
      # ZLAYER_BOOTSTRAP_PASSWORD: "<long-random-string>"
      # ZLAYER_BOOTSTRAP_PASSWORD_FILE: "/run/secrets/zlayer_admin_pw"
      # ZLAYER_BOOTSTRAP_DISPLAY_NAME: "Admin"
```

### 4b. `bin/zlayer/src/commands/manager.rs` generated spec

Update `handle_manager_init` (lines 37-64) so `zlayer manager init` emits a
spec with a commented `env:` block documenting the same four vars. This
is the user-visible template people will copy and customize.

Minimum: add an `env:` map with the `ZLAYER_MANAGER_ADDR` key (already
implicit) and four commented bootstrap lines. Use the same `\n#` comment
style already used elsewhere in that file so the emitted YAML is
self-documenting.

## 5. Security notes

- **Do NOT log the password**, even at DEBUG. `tracing::info!` calls use
  `email = %email_lc` only.
- **`_PASSWORD_FILE` is the recommended path for production** — docker/k8s
  secrets mount there as a file, avoiding the env-var leakage surface
  (ps / /proc/<pid>/environ / crash dumps).
- **Once bootstrapped, clear the envs in the spec.** Document this in the
  `manager init`-generated YAML comment: "after first successful boot,
  these envs are no-ops; leave or remove at your discretion". Leaving
  them is harmless because the idempotency check short-circuits.
- **`ensure_admin` vs `create_api_key`**: use `create_api_key` here, NOT
  `ensure_admin`. We've already established table-emptiness as the gate;
  using `ensure_admin` would mask the case where somebody manually created
  a user row but no credential row (shouldn't happen, but fail-loud is
  better).
- **No secret rotation story in this change.** If the operator wants to
  rotate the admin password, they do it via the Manager UI or a future
  `zlayer admin reset-password --email foo@bar.com` CLI. Don't try to
  make the env vars rotate-capable.

## 6. Test plan

1. **Unit test** in `bin/zlayer/src/bootstrap_admin.rs`:
    - `load_password` — all four branches (`_PASSWORD` set, `_FILE` set,
      both set → err, neither set → err, `_FILE` path missing → err,
      `_FILE` empty → err, `_FILE` with trailing `\n` → trimmed).
    - `maybe_bootstrap_admin` — table non-empty short-circuits; table
      empty + no envs logs and returns Ok; table empty + both envs
      present → user + credential rows exist afterward.
    - Use in-memory `sqlx::SqlitePool::connect(":memory:")` for the
      `UserStorage` impl and a temp-dir `PersistentSecretsStore`.

2. **Integration test** under `bin/zlayer/tests/`:
    - Spawn the daemon (`serve_bound`-equivalent) with envs set and no
      existing `users.db` → assert HTTP `POST /auth/login` with the
      bootstrapped creds returns 200 and a session cookie.
    - Second startup with the same `data_dir` and same envs → no new user
      row, no crash.
    - Second startup with different envs → still no new row (idempotent).

3. **Smoke test via `zlayer deploy`**:
    - `zlayer manager init` → uncomment the three bootstrap envs → `zlayer
      deploy manager.zlayer.yml` → `curl
      http://localhost:6677/auth/login` with the creds.

## 7. Files to create / edit

| Path | Action |
|---|---|
| `bin/zlayer/src/bootstrap_admin.rs` | **create** |
| `bin/zlayer/src/main.rs` (or `lib.rs`, wherever modules live) | add `mod bootstrap_admin;` |
| `bin/zlayer/src/commands/serve.rs` | insert `maybe_bootstrap_admin` call between `StorageBundle::open` and `serve_bound` |
| `bin/zlayer/src/commands/manager.rs` | extend `handle_manager_init` to emit commented-out `env:` block |
| `images/ZImagefile.zlayer-manager` | append commented-out bootstrap env lines |
| `bin/zlayer/tests/bootstrap_admin_e2e.rs` | **create** (integration test) |
| `CHANGELOG.md` | new `### Added` bullet under current `## [0.10.105]` (or next) |
| *(mirror to `zlayer-zql`)* | same paths, same edits — UI crates excluded but `zlayer-api`/`zlayer-secrets`/`bin/zlayer` are all mirrored |

## 8. Changelog entry (both repos)

```markdown
### Added
- `zlayer-manager`: non-interactive admin bootstrap. When the users table
  is empty and `ZLAYER_BOOTSTRAP_EMAIL` + `ZLAYER_BOOTSTRAP_PASSWORD`
  (or `ZLAYER_BOOTSTRAP_PASSWORD_FILE`) are set, the daemon creates the
  initial admin (Argon2id hash + user row) **before** the HTTP listener
  accepts, closing the "first request wins admin" race for CI/IaC/k8s
  deployments. Browser `/auth/bootstrap` flow is unchanged for
  interactive installs. Added `ZLAYER_BOOTSTRAP_DISPLAY_NAME` for the
  profile field. `images/ZImagefile.zlayer-manager` and the spec emitted
  by `zlayer manager init` now document the contract as commented-out env
  entries.
```

## 9. Out of scope for this change (deliberate)

- Secret rotation / password reset flows.
- Multi-admin env-var bootstrap (only seeds one admin; add more via the
  Manager UI or API).
- OIDC / SSO.
- Renaming any existing env var.
- Refactoring `CredentialStore` to own the user store too — they stay
  separate because the API-key path (`admin` key for the old CLI) and
  the user-login path (email+password) have different lifecycles.
