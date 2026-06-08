# ADR 0001 — Signed cluster join tokens

**Status:** Accepted (2026-05-14)
**Wave plan:** /home/zach/.claude/plans/please-investigate-using-opus-stateless-wreath.md

## Context

Today, cluster join uses a symmetric `join_secret` plus a plaintext base64-JSON
token (with an HS256-JWT alternative on a parallel code path). Tokens have no
per-token expiry, there is no rotation mechanism, no revocation, and the
verifying-key fetch is performed over plaintext HTTP, so a network attacker can
trivially MitM a new node into trusting an attacker-controlled cluster identity.

## Decision

Replace the plaintext token format with Ed25519-signed tokens carrying `exp`
and `kid` claims. Keep HS256 as a parallel modern format until Wave 11 provides
an opt-in retirement path. Add TLS to the daemon API in Wave 2 so the
verifying-key fetch isn't trivially MitM-able. Subsequent waves:

- Wave 5: rotation with a grace-period window.
- Wave 7: revocation list (signed, cluster-replicated).
- Wave 8: HW-backed keys (TPM 2.0, YubiHSM 2), auto-detected at startup.
- Wave 9: cluster-to-cluster federation via SPIFFE-style identities.
- Wave 10: privileged-container E2E coverage.
- Wave 11: HS256 retirement path; fresh installs from this point drop
  `{data_dir}/join_secret` entirely.

## Consequences

- New `ed25519-dalek` workspace dep (this wave).
- Fresh installs from Wave 11 onward will have no `{data_dir}/join_secret` file.
- Operators can opt into TPM 2.0 or YubiHSM 2 for signing-key storage.
  Auto-detection makes this zero-config when HW is reachable.
- Cross-cluster federation becomes opt-in via mutually imported trust bundles.

## Out of scope

- JWS/JOSE strict wire compatibility (Wave 3 uses a minimal self-described
  envelope; Wave 11 EdDSA-JWT IS standard JWT).
- Browser-style certificate transparency / OCSP / Merkle logs for trust bundles.

## References

- Full wave plan: see plan file above.
