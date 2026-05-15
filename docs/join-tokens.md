# Cluster Join Tokens

How a new node authenticates itself to an existing ZLayer cluster.

## Overview

A join token is a base64-encoded credential a new node presents when it
calls `POST /api/v1/cluster/join`. The token tells the leader: "I'm
authorized to join this cluster, and here's the verifying material so
you can prove it." Three formats coexist today:

| Format          | Default in v0.12.0? | Deprecation                          | When to use                                                              |
|-----------------|---------------------|--------------------------------------|--------------------------------------------------------------------------|
| Ed25519-signed  | yes                 | n/a                                  | Default for all new deployments.                                         |
| HS256-JWT       | yes                 | n/a (Wave 11 will provide migration) | Backward compat with v0.11.x and earlier; symmetric secret.              |
| Legacy plaintext| no                  | v0.13.0 (Wave 6)                     | Emergency rollback only (`--legacy-plaintext`).                          |

If you are doing a fresh install on v0.12.0, you do not need to think
about the table above: take the first token the CLI prints (the signed
one) and feed it to the joining node. The rest of this document
exists for operators who need to understand the trust model, mix old
and new daemons during an upgrade, or plan for the v0.13.0 removal of
plaintext.

## Minting a token

Run on any node that is already a cluster member (typically the
leader, but any voter works):

```bash
zlayer node generate-join-token --ttl 24h
```

Output (default, two tokens):

```
Signed token (recommended, expires 2026-05-15T17:55:00Z):
  <signed-token-b64>

HS256 token (modern alternative):
  <hs256-jwt>
```

The `--ttl` flag accepts humantime syntax (`30m`, `24h`, `7d`, etc.)
and is applied to the signed token's `exp` claim. The default is
`24h`. The HS256 token uses the same TTL when present in the claim
set; the legacy plaintext token (see below) has no expiry at all.

To ALSO emit the legacy plaintext token — for example, when one of
your peers is still running v0.11.x and cannot verify a signed
token — pass `--legacy-plaintext`:

```bash
zlayer node generate-join-token --ttl 24h --legacy-plaintext
```

The plaintext form is deprecated and will be rejected entirely
starting v0.13.0 (Wave 6). Do not script around it; treat any use as
a temporary workaround during a rolling upgrade.

## Token format details

### Ed25519-signed (recommended)

A minimal self-described envelope (NOT a standard JWS, though Wave 11
will provide one):

```
{
  "v": 1,
  "kid": "<key id, first 16 hex chars of pubkey SHA-256>",
  "claims": {
    "iss": "<issuer node UUID>",
    "exp": <unix seconds>,
    "cluster_id": "<cluster UUID>",
    "jti": "<random 16 bytes hex>"
  },
  "sig": "<base64 Ed25519 signature over the canonical claims bytes>"
}
```

Signing is done over the claims serialized with deterministic field
ordering so verifiers do not need a canonical-JSON library — they
re-serialize with the same ordering rule and verify against `sig`.

The joining node fetches the cluster's verifying key from
`GET /api/v1/cluster/signing-pubkey` (introduced in Wave 1,
unauthenticated by design — the response is a public key) and matches
the response's `kid` against the token's `kid`. If they match and the
signature verifies and `exp` is in the future, the join is admitted.

Key rotation (Wave 5, not yet shipped) will use the `kid` to support
a grace-period window where both the old and new keys verify.

### HS256-JWT (modern)

Standard JWT with HS256 signature. The HMAC key is derived from
`{data_dir}/join_secret` (a 32-byte random secret generated once per
cluster at bootstrap). Claims:

```
{
  "iss": "<issuer node UUID>",
  "exp": <unix seconds>,
  "cluster_id": "<cluster UUID>",
  "jti": "<random 16 bytes hex>"
}
```

The `jti` is replay-protected: the leader records used `jti`s for the
remainder of their `exp` window and rejects a second use.

HS256 is symmetric: anyone who can read `{data_dir}/join_secret` on
any voter can mint tokens. Treat that file as a secret on par with the
admin API token. Wave 11 will provide an opt-in path to retire HS256
entirely; fresh installs from Wave 11 onward will have no
`join_secret` file.

### Legacy plaintext

A base64-encoded JSON blob containing the cluster `auth_secret` and
cluster metadata, with no signature and no expiration. Servers accept
it for backward compat with pre-v0.12.0 deployments but now emit a
deprecation warning both server-side (`tracing::warn`) and in the API
response body. The `--legacy-plaintext` flag gates its emission on
the CLI.

Removal: v0.13.0 (Wave 6). Mid-Wave-6 daemons will reject these
tokens with HTTP 410 Gone.

## Trust model

The signed-token format defends against three concrete threats:

1. **Forged tokens from a non-cluster party.** Without the cluster's
   Ed25519 signing key, an attacker cannot produce a valid token.
2. **Replay of an old token after policy change.** The `exp` claim
   bounds the window; combined with rotation (Wave 5) and revocation
   (Wave 7), an operator can invalidate tokens issued before a known
   compromise.
3. **MitM substitution of the verifying key during fetch.** Wave 2
   adds TLS to the daemon API listener so a network attacker cannot
   replace the `/cluster/signing-pubkey` response with their own
   public key as the joiner fetches it.

The signed-token format does NOT defend against:

- A compromised cluster member with read access to
  `{data_dir}/cluster_signing.key` — that key is the cluster's
  identity. Wave 8 adds optional hardware backing (TPM 2.0, YubiHSM 2)
  to mitigate this.
- A compromised operator who can run `zlayer node generate-join-token`
  on a real cluster node — there is no audit log of token issuance
  yet. Treat shell access on voters as cluster-admin equivalent.

## TLS for the verifying-key endpoint

`GET /api/v1/cluster/signing-pubkey` is unauthenticated by design
(the response is a public key), but it MUST be served over TLS in
production to prevent an on-path attacker from substituting their own
public key as the joining node fetches it.

Wave 2 added three flags to the daemon:

- `--api-tls-cert <PATH>` and `--api-tls-key <PATH>` load a static
  PEM keypair. Use this when you have your own certificate management
  pipeline (e.g., an internal CA).
- `--api-tls-acme <DOMAIN>` delegates to the proxy crate's
  ACME-capable `CertManager` and obtains a cert automatically. Use
  this when the daemon's API endpoint has a stable public DNS name.

Default is off (plain HTTP) so a single-node `cargo run -p zlayer --
serve` still works zero-config; do not run a production cluster
without one of the two flags above.

## Roadmap

- v0.12.x — current. Wave 4 ships this doc and flips the default
  token output to signed + HS256 only.
- v0.13.0 — plaintext token removal (Wave 6). The
  `--legacy-plaintext` flag and the server-side acceptance path are
  both deleted.
- Future waves —
  - Wave 5: key rotation with a grace-period window.
  - Wave 7: revocation list (signed, cluster-replicated).
  - Wave 8: HW-backed signing keys (TPM 2.0, YubiHSM 2),
    auto-detected at startup.
  - Wave 9: cluster-to-cluster federation via SPIFFE-style
    identities.
  - Wave 11: HS256 retirement path; fresh installs drop
    `{data_dir}/join_secret` entirely.

## See also

- [`docs/adr/0001-signed-join-tokens.md`](adr/0001-signed-join-tokens.md) — architectural decision record.
- [`CHANGELOG.md`](../CHANGELOG.md) — release-by-release behavior changes.
- [`docs/rolling-upgrade.md`](rolling-upgrade.md) — how to upgrade a
  multi-node cluster without taking it offline (relevant when mixing
  pre-v0.12.0 and v0.12.0 daemons during the transition).
