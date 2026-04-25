# zlayer-secrets

Secure secrets management for ZLayer container workloads.

## Overview

`zlayer-secrets` is the secrets subsystem used across ZLayer to store, encrypt,
resolve, and inject sensitive values for services, jobs, and crons. Secrets are
encrypted at rest with XChaCha20-Poly1305 and persisted in a SQLite-backed
store. Higher layers (the API server, the daemon, and the agent) consume
[`SecretsStore`] / [`SecretsProvider`] trait objects from this crate to
implement the `zlayer secret` CLI verbs (`create`, `get`, `set`, `rm`,
`rotate`, `unset`, `import`, `export`, `grant`, `revoke`, `permissions`) and
to inject resolved environment variables at container start.

## Public API

The library is organised around a small set of traits and types that
downstream crates compose:

- `zlayer_secrets::SecretsProvider` (trait) — read-only secret access
  (`get_secret`, `get_secrets` for batch fetch, `list_secrets`, `exists`).
- `zlayer_secrets::SecretsStore` (trait) — read-write extension with
  `set_secret`, `delete_secret`, and a default `rotate_secret` impl that
  returns a [`RotationResult`] describing the version before and after.
- `zlayer_secrets::SecretsResolver<P>` — resolves `$S:` and `$secret://`
  references in env-var maps. Use [`SecretsResolver::with_env_resolver`]
  to wire in an [`EnvScopeProvider`] when environment-scoped lookups are
  needed.
- `zlayer_secrets::SecretRef` — parser for the reference syntax (see
  below). Round-trip safe via `Display`.
- `zlayer_secrets::Secret` — `Zeroize`-able, `Debug`-redacted wrapper around
  `secrecy::SecretString`.
- `zlayer_secrets::PersistentSecretsStore` — SQLite-backed `SecretsStore`
  (feature `persistent`, on by default). Encrypts each value with
  XChaCha20-Poly1305 keyed by an [`EncryptionKey`].
- `zlayer_secrets::KeyManager` — encryption-key resolution: env var
  `ZLAYER_SECRETS_KEY` (hex), then `ZLAYER_SECRETS_PASSWORD` (Argon2id KDF),
  then `{base_dir}/secrets_{deployment}.key`, otherwise auto-generated and
  written.
- `zlayer_secrets::credentials::CredentialStore` — Argon2id-hashed API key
  records used for ZLayer API authentication.
- `zlayer_secrets::registry_credentials::{RegistryCredentialStore, RegistryAuthType, RegistryCredential}`
  — typed store for OCI registry auth (basic / token).
- `zlayer_secrets::git_credentials::{GitCredentialStore, GitCredentialKind, GitCredential}`
  — typed store for Git PATs and SSH keys.

### Reference syntax

`SecretRef::parse` recognises:

- `$S:name` — deployment-level
- `$S:name/field` — deployment-level with JSON field extraction
- `$S:@service/name[/field]` — service-scoped
- `$S::env/name[/field]` — environment-scoped (no project)
- `$S:project:env/name[/field]` — project-scoped environment

The resolver also accepts the URL-style `$secret://<env>/<KEY>[/<field>]`
form, which requires an `EnvScopeProvider` to map an env name-or-id to the
backend scope string.

## Use from ZLayer

- `crates/zlayer-api` — exposes the secret CRUD endpoints, environment-aware
  routing, registry-credential management, and credential-based auth.
- `bin/zlayer` — implements the `zlayer secret` CLI subcommands and the
  daemon's secret-injection path.
- `crates/zlayer-agent` — calls `SecretsResolver::resolve_env` to materialise
  env vars at container start.

## Cargo features

| Feature      | Default | Effect                                                           |
|--------------|---------|------------------------------------------------------------------|
| `persistent` | yes     | Enables `PersistentSecretsStore` and the credential store types via `sqlx` (SQLite). |
| `vault`      | no      | Enables `VaultSecretsProvider` (HashiCorp Vault) via `vaultrs`.   |

Disable default features to use only the in-memory traits and parser.

## Platform notes

- The SQLite backend uses WAL journal mode and runs on every supported
  platform (Linux, macOS, Windows).
- `KeyManager` writes the on-disk key file with `0600` permissions on Unix;
  on Windows the file inherits the parent ACL.
- The default key directory is `zlayer_paths::ZLayerDirs::system_default().secrets()`.

## When to edit this crate

Touch `zlayer-secrets` when changing the on-disk format, adding a new
backend (e.g. cloud KMS), evolving the `$S:` / `$secret://` reference
grammar, or extending the trait surface used by the API and agent.
Invariants worth preserving: round-trip stability of `SecretRef` parsing,
zeroization of `Secret` on drop, and the WAL-mode SQLite assumptions in
`PersistentSecretsStore`.

Repository: <https://github.com/BlackLeafDigital/ZLayer>
