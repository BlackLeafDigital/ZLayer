# TODO_PGP.md — Signed, rotatable cluster join tokens

Follow-up to the cluster-join-token fix that landed in this PR. The
current cluster-join flow uses a long-lived **symmetric shared secret**
(`auth_secret`, persisted at `{data_dir}/join_secret`) baked into every
join token. Anyone with the secret can mint a valid token; anyone with
a single valid token can replay it forever; rotation requires manual
redistribution.

Goal: replace that with **Ed25519-signed tokens** that the leader
mints, that carry an `exp` timestamp, and whose verification key can
be rotated without coordinating with every node out-of-band.

The work breaks into 5 sequential waves. Each wave ships independently
green (`cargo fmt --all && cargo clippy --workspace --all-targets -- -D
warnings && cargo test --workspace && cargo check --workspace` in BOTH
ZLayer and zlayer-zql) and leaves the cluster operable. No wave assumes
its successor; you can stop after any wave.

The dual-repo invariant ([[feedback_dual_repo_mirroring]]) holds
throughout: every Rust change to `bin/zlayer/` or
`crates/zlayer-{api,scheduler,consensus,types}/` lands in **both**
ZLayer and zlayer-zql via parallel agents.

---

## Wave 0 — Threat model & design pin (no code)

Before Wave 1, write down what we actually want.

- **Confidentiality.** Tokens are NOT secrets after issue — they encode
  cluster topology (raft endpoint, leader WG pubkey, overlay CIDR).
  We're protecting **integrity** and **freshness**, not secrecy. A
  leaked token is fine if the signature has expired and the signing
  key has been rotated.
- **Threat model.** An attacker on the network can sniff tokens in
  transit (Slack / wiki / `zlayer node generate-join-token` stdout).
  We assume they can NOT compromise the leader's filesystem; if they
  can, they own the cluster already.
- **Goals.**
  1. Bound token validity in time (default `exp = now + 24h`,
     configurable per-issue via `--ttl 1h`).
  2. Allow operator to rotate the signing keypair WITHOUT downtime
     and WITHOUT invalidating in-flight legitimate joins.
  3. Joining nodes verify a token without needing prior shared state
     beyond the leader's API endpoint (the public verifying key is
     fetched over the same TLS-protected channel the join itself uses).
  4. The verifying key is bound to the leader's identity (e.g. signed
     by a long-lived CA key, or transported alongside a TLS cert pin)
     so an MitM can't substitute their own pubkey.
- **Non-goals.**
  - Federated trust between unrelated clusters.
  - Revoking already-issued un-expired tokens (cluster operators
    rotate keys instead — see Wave 4).
  - Token JWS/JOSE compliance. We use a minimal self-described format,
    not RFC 7519.

**Deliverable for Wave 0:** a short ADR (architecture decision record)
in `docs/adr/0001-signed-join-tokens.md` (ZLayer-only — docs aren't
mirrored to zql) capturing the above. Then move on.

---

## Wave 1 — Plumb a per-cluster Ed25519 signing keypair

Add storage and lifecycle for the signing key. **No verification logic
yet** — this wave only persists the keypair and exposes it via a
read-side API.

### Tasks

1. **New crate module: `crates/zlayer-secrets/src/cluster_signer.rs`.**
   Add `ClusterSigner { signing: ed25519_dalek::SigningKey, public:
   ed25519_dalek::VerifyingKey }`. Implement:
   - `ClusterSigner::generate() -> Self` (CSPRNG, fresh key).
   - `ClusterSigner::load_or_generate(path: &Path) -> Result<Self>` —
     reads `{path}` if it exists (32-byte raw seed), else generates and
     persists with 0600 perms (mirror the existing pattern from
     `crates/zlayer-secrets/src/key_manager.rs` line ~85). The file is
     `{data_dir}/cluster_signing.key`.
   - `ClusterSigner::verifying_key(&self) -> VerifyingKey`.
   - `ClusterSigner::public_key_b64(&self) -> String` (URL-safe no-pad
     base64 of the 32-byte verifying key).

2. **Daemon bootstrap.** In `bin/zlayer/src/commands/serve.rs` (mirror
   into zql), right after the existing `Generated and persisted new
   cluster join secret` block (~line 1422-1442), do the same dance for
   the signer:
   - Call `ClusterSigner::load_or_generate(&data_dir.join("cluster_signing.key"))`.
   - Log `Loaded cluster signing keypair, public_key_b64: ...`.
   - Store the signer in the daemon's state (likely on whatever struct
     `DaemonState` / `AppState` is; grep for `auth_secret` to find where
     the symmetric secret lives today and put the signer alongside).

3. **API endpoint to expose the verifying key.** Add
   `GET /cluster/signing-pubkey` to `crates/zlayer-api/src/handlers/cluster.rs`,
   returning JSON:
   ```json
   { "public_key_b64": "...", "key_id": "..." }
   ```
   `key_id` is the first 8 hex chars of `SHA-256(verifying_key)` —
   enough to disambiguate during rotation, short enough to grep
   for in logs. This endpoint MUST be reachable on the *plain* HTTP
   API (no auth) because joining nodes need it before they have any
   credential. Add a brief audit note: it's safe to expose the public
   key on an unauthed endpoint — that's the whole point.

4. **Workspace green.** Run the full quad (`fmt`, `clippy --workspace
   --all-targets -- -D warnings`, `test --workspace`, `check
   --workspace`) in BOTH repos. No new dep beyond `ed25519-dalek`
   (workspace dep, check `Cargo.toml` to see if it's already there).

### Wave 1 exit criteria

- `{data_dir}/cluster_signing.key` is created on first daemon start
  and reused on subsequent restarts.
- `curl http://leader:3669/cluster/signing-pubkey` returns the b64
  pubkey + key_id.
- No behavior change for existing `auth_secret`-based tokens — they
  still mint, still validate. The signer is dark until Wave 2 wires
  it into the token format.

---

## Wave 2 — Signed token format alongside the existing one

Add a new token shape but keep the old one accepted. Joiners advertise
which formats they understand; leaders accept either; default to
unsigned until Wave 3 flips the default.

### Tasks

1. **New struct.** In `bin/zlayer/src/commands/node.rs` (mirror to zql),
   add `SignedClusterJoinToken`:
   ```rust
   #[derive(Serialize, Deserialize)]
   pub struct SignedClusterJoinToken {
       /// Format version. Always 1 for this wave. Bump on incompatible
       /// payload changes.
       pub v: u32,
       /// First 8 hex of SHA-256(verifying_key). Lets the joiner pick
       /// the correct pubkey during key rotation.
       pub kid: String,
       /// Payload — the existing ClusterJoinToken fields, plus exp.
       pub claims: ClusterJoinClaims,
       /// Ed25519 signature over the canonical JSON of `claims`
       /// (b64 url-safe no-pad).
       pub sig: String,
   }

   #[derive(Serialize, Deserialize)]
   pub struct ClusterJoinClaims {
       pub api_endpoint: String,
       pub raft_endpoint: String,
       pub leader_wg_pubkey: String,
       pub overlay_cidr: String,
       /// RFC3339 expiration timestamp.
       pub exp: String,
       /// RFC3339 issue timestamp.
       pub iat: String,
       /// Issuing leader node id (UUID string).
       pub iss: String,
   }
   ```
   Note: `auth_secret` is gone from this struct. With signatures, the
   shared secret is redundant.

2. **Canonical serialization for signing.** Sign over
   `serde_json::to_vec(&claims)?` with `serde_json`'s default field
   order being deterministic given the field declaration order.
   Document this in a comment on `ClusterJoinClaims`: "Field declaration
   order is the canonical signing order. Do NOT reorder without
   bumping `v` and adding a migration."

3. **Token mint flow (`build_cluster_join_token_from_disk` and
   `handle_node_generate_join_token`).** Add a `--ttl` CLI flag
   (default 24h). Build `ClusterJoinClaims`, sign with the daemon's
   `ClusterSigner`, wrap in `SignedClusterJoinToken`, base64-encode
   the whole envelope. Emit BOTH the legacy unsigned token (for the
   transition window) and the new signed token in the CLI output:
   ```
   Legacy token (unsigned, deprecated):
     <legacy>
   Signed token (recommended):
     <signed>
   ```
   For the transition window, the CLI also accepts both formats and
   the server-side parser tries `parse_signed_cluster_join_token` first
   then falls back to `parse_cluster_join_token`. Add the parser at
   `node.rs` next to the existing `parse_cluster_join_token`.

4. **Server-side accept of both.** In
   `crates/zlayer-api/src/handlers/cluster.rs` `validate_join_token`
   (line ~1481-1510), branch on token shape:
   - If it parses as `SignedClusterJoinToken`: fetch the daemon's
     verifying key by `kid`, verify signature over canonical
     `claims` bytes, check `now < exp`, accept.
   - Else parse as legacy `ClusterJoinToken`, validate against
     `auth_secret` as today.
   Emit a structured log at INFO that records which format was used:
   `accepted join token, format: signed|legacy, kid: xxx, iss: xxx`.

5. **Tests.** In `node.rs` test module:
   - `signed_token_round_trips_through_verify` — mint, parse, verify, OK.
   - `signed_token_rejects_when_expired` — mint with `exp = now - 1s`,
     verify, expect `TokenExpired`.
   - `signed_token_rejects_when_signature_mangled` — flip one bit of
     the sig, expect `SignatureMismatch`.
   - `legacy_token_still_validates_when_signed_parser_fails_first` —
     transition-window safety.

6. **Workspace green** in both repos.

### Wave 2 exit criteria

- `zlayer node generate-join-token --ttl 1h` emits a signed token
  ~340 chars (vs ~280 for legacy).
- `zlayer node join … --token <signed>` succeeds on a leader.
- `zlayer node join … --token <legacy>` still succeeds (transition).
- Expired or tampered tokens are rejected with actionable errors.

---

## Wave 3 — Default to signed; warn on legacy

Switch the default and start nagging.

### Tasks

1. **Flip the CLI default** in
   `bin/zlayer/src/commands/node.rs::handle_node_generate_join_token`:
   emit only the signed token by default. Legacy is emitted only
   under `--legacy` flag (kept for one release for emergency
   rollback). Print:
   ```
   Token (signed, expires 2026-05-15T17:55Z):
     <signed>
   ```

2. **Server-side deprecation warn.** In `validate_join_token`, when a
   legacy token validates successfully, emit `warn!("accepted legacy
   unsigned join token from {peer_ip}. Switch issuers to signed tokens
   before next ZLayer release; legacy support will be removed.")` and
   include the warning in the API response body so the joining node's
   CLI surfaces it too.

3. **Document.** Update `CHANGELOG.md` and any operator-facing docs
   under `docs/` to mention the deprecation. (ZLayer only; zql excludes
   `docs/` per the cross-repo split? — confirm by checking whether
   `docs/` exists in zql.)

4. **Workspace green** in both repos.

### Wave 3 exit criteria

- Default issuance is signed.
- Legacy still works but logs warn on both sides.
- Cluster operators have one release cycle to migrate.

---

## Wave 4 — Key rotation flow

The whole point of asymmetric is rotation. Add the CLI and runtime
support.

### Tasks

1. **Storage shape.** Switch `cluster_signing.key` to a JSON file
   storing **multiple** keys with one marked active:
   ```json
   {
     "keys": [
       { "id": "abc12345", "seed_b64": "...", "created_at": "..." },
       { "id": "def67890", "seed_b64": "...", "created_at": "..." }
     ],
     "active": "def67890",
     "retired_grace_until": {
       "abc12345": "2026-06-15T00:00:00Z"
     }
   }
   ```
   - `keys` — all currently-valid signing keypairs.
   - `active` — the one new tokens are signed with.
   - `retired_grace_until` — keys we'll accept token verifications
     against until the timestamp. Lets in-flight legitimate tokens
     still resolve after rotation.
   Migration from Wave-1's raw-seed file: on daemon startup, if the
   old format is detected, wrap it in the new JSON with `active`
   pointing at it. One-shot, idempotent.

2. **CLI command.** Add `zlayer cluster rotate-signing-key`:
   - Generates a new keypair.
   - Inserts it into `keys`, sets `active` to its `id`.
   - Records the previous active key in `retired_grace_until` with
     `now + 7 days` (configurable via `--grace 24h`).
   - Logs the new pubkey + kid.

3. **API surface.** `GET /cluster/signing-pubkeys` (plural) now
   returns ALL non-retired pubkeys plus their kids and statuses:
   ```json
   {
     "keys": [
       { "kid": "abc12345", "public_key_b64": "...", "status": "retired", "valid_until": "..." },
       { "kid": "def67890", "public_key_b64": "...", "status": "active" }
     ]
   }
   ```
   The Wave-1 singular endpoint stays as an alias returning just the
   active key.

4. **Verification side.** `validate_join_token` looks up the pubkey
   by `kid` from the rotation state, checks active OR
   not-yet-expired-grace, accepts.

5. **Background sweep.** Daemon spawns a tokio task that prunes keys
   from `retired_grace_until` once expired (and removes them from
   `keys` too). One sweep per hour is fine.

6. **Tests.**
   - `rotation_flips_active_and_old_keeps_grace`.
   - `expired_grace_key_rejects_verification`.
   - `migration_from_v1_raw_seed_file_works_once`.

7. **Workspace green** in both repos.

### Wave 4 exit criteria

- `zlayer cluster rotate-signing-key` produces a fresh active key
  without invalidating in-flight legitimate tokens.
- Retired keys are pruned automatically once their grace expires.
- The wire format/endpoint can evolve forward without breaking
  existing nodes during the grace window.

---

## Wave 5 — Remove legacy unsigned token support

Once Wave 4 has shipped and at least one full release has passed
where Wave 3 emitted deprecation warnings.

### Tasks

1. **Stop minting legacy.** Remove `--legacy` from the CLI surface.
2. **Stop accepting legacy.** In `validate_join_token`, return an
   actionable error: `"unsigned join token rejected. Re-issue with
   'zlayer node generate-join-token' (signed, expires per --ttl).
   See CHANGELOG version X.Y for migration notes."`
3. **Delete unused code.** Remove `auth_secret` from
   `ClusterJoinToken` callers and `{data_dir}/join_secret` file
   creation. Old `join_secret` files become inert — daemon ignores
   them but doesn't delete (operator can clean up via `--vacuum-secrets`).
4. **Document.** Final CHANGELOG entry + migration notes.
5. **Workspace green** in both repos.

### Wave 5 exit criteria

- Symmetric `auth_secret` is no longer minted, no longer accepted.
- All join paths require a signed, expiring, rotatable token.

---

## Cross-cutting concerns to watch through every wave

- **`bin/zlayer/src/commands/node.rs` and `bin/zlayer/src/daemon.rs`
  always mirror to zql.** Use parallel foreground agents, never `cp`.
- **`crates/zlayer-secrets/`, `crates/zlayer-api/`,
  `crates/zlayer-consensus/`, `crates/zlayer-scheduler/` exist in both
  repos with the same module shape; mirror them too.**
- **`crates/zlayer-manager/`, `crates/zlayer-web/`, `docs/`,
  `CHANGELOG.md`, `TODO_PGP.md`, ADRs are ZLayer-only**
  ([[project_zql_excludes_manager_and_web]]).
- **No `git stash` ever** ([[feedback_never_git_stash_or_excessive_git]]).
- **All checks workspace-green, never `-p`**
  ([[feedback_use_agents_for_all_fixes]] applies to lint cleanups too).
- **`/tmp` is for sockets, never state**
  ([[feedback_tmp_ok_for_sockets_never_state]]). The signing key file
  is state; it lives under `data_dir`.

## Out of scope for this whole follow-up

- **Cluster-to-cluster federation / SPIFFE-style identities.** Separate
  effort; would build on this but require a CA-like trust anchor.
- **Hardware-backed key storage (TPM / YubiHSM).** The `Storage shape`
  in Wave 4 is JSON for ease of operator inspection. A future
  `--key-store-backend=tpm` toggle could swap implementations.
- **Token revocation lists.** Intentionally not designed. Operators
  rotate the signing key (Wave 4) instead.
- **Privileged-container E2E runner.** Tracked separately — needed to
  exercise the boringtun/overlay path, unrelated to token signing.

## Critical files (where things will live)

- `crates/zlayer-secrets/src/cluster_signer.rs` (NEW, Wave 1).
- `bin/zlayer/src/commands/serve.rs` — bootstrap the signer alongside
  the existing `join_secret` setup (Wave 1).
- `bin/zlayer/src/commands/node.rs` — `SignedClusterJoinToken` struct,
  mint/parse/verify functions, CLI flags (Waves 2–3, 5).
- `crates/zlayer-api/src/handlers/cluster.rs` — `validate_join_token`
  dual-format support → signed-only, plus new
  `/cluster/signing-pubkey(s)` endpoints (Waves 1, 2, 4, 5).
- `bin/zlayer/src/commands/cluster.rs` (or wherever cluster admin
  CLIs live — grep) — `rotate-signing-key` subcommand (Wave 4).
- `docs/adr/0001-signed-join-tokens.md` (NEW, Wave 0; ZLayer-only).
- `CHANGELOG.md` — every wave (ZLayer-only).
