## Phase G — SBOM, provenance, advanced mounts

Polish phase for the native Rust OCI build executor. Adds supply-chain attestations (SBOM + SLSA), the two remaining `--mount` types BuildKit users actually rely on (`secret`, `ssh`), and finishes out the cache backend matrix that Phase C only stubbed in. Assumes A–F have landed: native executor, layer writer, mount dispatch for `bind/cache/tmpfs`, registry push, and `--cache-{to,from}=type=registry` end-to-end.

### Goals

1. Generate an SBOM for every `zlayer build` output and attach it via the OCI Referrers API ([spec](https://github.com/opencontainers/image-spec/blob/main/manifest.md), [distribution](https://github.com/opencontainers/distribution-spec/blob/main/spec.md)).
2. Generate and attach a SLSA v1.0 provenance attestation ([spec](https://slsa.dev/spec/v1.0/provenance)) as an in-toto statement. Unsigned in this phase; cosign signing tracked as the immediately following effort (user must approve descope; not declared out-of-scope here).
3. Implement `RUN --mount=type=secret` and `RUN --mount=type=ssh` end-to-end in the native executor, matching BuildKit semantics ([Dockerfile ref](https://docs.docker.com/reference/dockerfile/)).
4. Finish the cache backend matrix: `--cache-to=type=s3`, `--cache-to=type=local,dest=<path>`, `--cache-to=type=inline`. The internal `CacheBackendConfig` enum at `crates/zlayer-builder/src/builder.rs:194` already has `Persistent`, `Memory`, `S3` variants; this surfaces them through the CLI cache-spec parser added in Phase C.

### Steps

Each step is one agent, ≤2 files, ≤~200 LOC. Sequencing respects the types-and-API-first rule.

1. **G1 — Types in `zlayer-types`** (3h). Add `SbomFormat { Spdx, CycloneDx }`, `SbomDescriptor`, `ProvenancePredicate` (SLSA v1.0 shape), `BuildAttestation` to `crates/zlayer-types/src/build/`. No serde-of-spec types yet — those wrap the chosen ecosystem crates. Files: `crates/zlayer-types/src/build/attestation.rs` (new), `crates/zlayer-types/src/build/mod.rs`.

2. **G2 — API surface in `zlayer-api`** (3h). Extend the build endpoint request/response to carry `sbom: Option<SbomConfig>`, `provenance: Option<ProvenanceMode>` (`none|min|max`), and return referrer digests in the build result. Files: `crates/zlayer-api/src/handlers/build.rs`, `crates/zlayer-api/src/openapi.rs`.

3. **G3 — Cache spec parser completion** (4h). Extend the existing `--cache-to=` parser (Phase C) to recognize `type=s3,bucket=…,region=…,prefix=…`, `type=local,dest=<path>,mode={min,max}`, and `type=inline`. Map each to the matching `CacheBackendConfig` variant. Inline cache is a new variant — emit cache manifest annotations on the image manifest itself rather than a side artifact. Files: `crates/zlayer-builder/src/builder.rs` (CacheBackendConfig enum + parser).

4. **G4 — Secret mount executor** (5h). Wire `RunMount::Secret` (already parsed at `crates/zlayer-builder/src/dockerfile/instruction.rs:405`) into the native executor: source from `--secret id=…,src=<path>` CLI args OR `id=…,env=<NAME>` env vars; mount tmpfs-backed file at `target` (default `/run/secrets/<id>`) with `mode=0400`, `uid=0`, `gid=0` defaults per [Docker spec](https://docs.docker.com/reference/dockerfile/#run---mounttypesecret); cache key MUST hash only `id` (not contents) so cached layers replay across secret rotations. Files: `crates/zlayer-builder/src/backend/executor.rs` (or current native executor module), `crates/zlayer-builder/src/dockerfile/instruction.rs`.

5. **G5 — SSH mount executor** (4h). Wire `RunMount::Ssh` similarly: socket forwarded into container at `/run/buildkit/ssh_agent.<N>` (numbered per Dockerfile spec), `mode=0600`, set `SSH_AUTH_SOCK` env var inside the RUN container. Source from `--ssh default=$SSH_AUTH_SOCK` or `--ssh <id>=<sock-path>`. Cache key hashes `id` only. Files: same as G4 (one module each).

6. **G6 — SBOM generator (chosen path from "open questions")** (6–10h depending on user choice). Native path: walk the assembled rootfs, parse package manager dbs (`/var/lib/dpkg/status`, `/var/lib/apk/db/installed`, `/var/lib/rpm/Packages`, language lockfiles), emit a CycloneDX 1.5 BOM via `cyclonedx-bom` crate. Shell-out path: exec `syft <rootfs-dir> -o cyclonedx-json` and capture stdout. Files: `crates/zlayer-builder/src/sbom.rs` (new), `crates/zlayer-builder/src/lib.rs`.

7. **G7 — SLSA provenance generator** (5h). Build an in-toto v1.0 Statement with `predicateType: "https://slsa.dev/provenance/v1"` and a SLSA `Provenance` predicate. Required: `buildDefinition.buildType` (`"https://zlayer.dev/build/v1"`), `buildDefinition.externalParameters` (the Dockerfile/ZImagefile path, build args, target platform), `runDetails.builder.id` (zlayer node URI + version). Recommended: `resolvedDependencies` (base image digests + git source commit when available), `runDetails.metadata` (`invocationId`, `startedOn`, `finishedOn`). Use `in_toto_attestation` crate for envelope. Files: `crates/zlayer-builder/src/provenance.rs` (new), `crates/zlayer-builder/src/builder.rs` (call site).

8. **G8 — Referrer push** (5h). Push SBOM and provenance as OCI artifacts whose manifests carry `subject: {digest: <image digest>}` and unique `artifactType` values (`application/spdx+json`, `application/vnd.cyclonedx+json`, `application/vnd.in-toto+json`). Try the referrers API (`GET /v2/<name>/referrers/<digest>` first to detect support); on `404 Not Found` use the fallback referrers tag schema (`<alg>-<encoded>` truncated). Files: `crates/zlayer-registry/src/client.rs` (extend with `push_referrer` + `list_referrers`), `crates/zlayer-builder/src/builder.rs`.

9. **G9 — Inline cache embedding** (4h). For `--cache-to=type=inline`: serialize cache metadata (per-layer source digests + buildkit-compatible `containerimage.buildinfo` shape) into the OCI image manifest's `annotations` map under keys `org.opencontainers.image.ref.buildinfo` and `containerimage.buildinfo`. Reader side (`--cache-from`) already exists from Phase C; only writer-side annotation emission is new here. Files: `crates/zlayer-builder/src/builder.rs`, `crates/zlayer-registry/src/oci_export.rs`.

10. **G10 — S3 / local cache backends wiring** (4h). The `CacheBackendConfig::S3` and `::Persistent` variants exist but aren't reachable from `--cache-to`. Wire the parser output from G3 through to the actual write path so blobs land in S3 (via existing `s3_cache.rs`) or local dir (via existing `persistent_cache.rs`). Files: `crates/zlayer-builder/src/builder.rs`, `crates/zlayer-builder/src/backend/sandbox.rs` (or current executor).

11. **G11 — CLI surface** (2h). Add `--sbom={none,spdx,cyclonedx}` (default `cyclonedx`), `--provenance={none,min,max}` (default `min`), `--secret id=…[,src=…|env=…]` (repeatable), `--ssh <id>=<sock>` (repeatable) to `zlayer build` and propagate through DaemonClient. Files: `bin/zlayer/src/commands/build.rs`, `bin/zlayer/src/cli.rs`.

12. **G12 — Tests + CHANGELOG** (4h). Unit tests for: SBOM emission (fixture rootfs with known dpkg db → expected CycloneDX), SLSA predicate field shape (round-trip with `in_toto_attestation`), secret mount cache-key stability across content changes, inline-cache annotation read-back, referrer push with simulated 404 fallback. Update CHANGELOG.md under `[Unreleased]` per project rule. Files: tests in `crates/zlayer-builder/tests/`, `CHANGELOG.md`.

### SBOM format choice (SPDX vs CycloneDX)

| Aspect | SPDX 2.3 ([spdx.dev](https://spdx.dev)) | CycloneDX 1.5 ([cyclonedx.org](https://cyclonedx.org)) |
|---|---|---|
| Origin | Linux Foundation, ISO/IEC 5962:2021 standard | OWASP, de-facto industry standard for container SBOMs |
| Rust crate | `spdx-rs` (parse-focused), `serde-spdx` | `cyclonedx-bom` (full read+write, actively maintained) |
| OCI media type | `application/spdx+json` | `application/vnd.cyclonedx+json` |
| Tooling alignment | GitHub dependency graph, Linux Foundation | Docker buildx default, grype, trivy, dependency-track |
| Verbosity | Higher; relationship-graph heavy | More compact; component-list flat |

**Recommendation: default to CycloneDX 1.5.** It matches what `docker buildx` produces today, the `cyclonedx-bom` Rust crate is the most complete BOM library on crates.io, and most downstream scanners (grype, trivy, dependency-track) consume it natively. Keep SPDX behind `--sbom=spdx` for users who need it for compliance (NTIA minimum elements / U.S. Executive Order 14028).

### SLSA provenance fields

Per [slsa.dev/spec/v1.0/provenance](https://slsa.dev/spec/v1.0/provenance), the in-toto Statement uses `predicateType: "https://slsa.dev/provenance/v1"`. Required:

- `buildDefinition.buildType` — opaque URI identifying our build template. Use `"https://zlayer.dev/build/v1"`.
- `buildDefinition.externalParameters` — user-controlled inputs that MUST be verified by a consumer. Includes: Dockerfile/ZImagefile path, `--build-arg` values, `--platform`, base image references as written.
- `runDetails.builder.id` — trusted builder identity. Use `"https://zlayer.dev/builder/{node-id}@{zlayer-version}"`.

Recommended (emit when `--provenance=max`):

- `buildDefinition.internalParameters` — platform-set inputs (default flags, executor backend choice).
- `buildDefinition.resolvedDependencies` — resolved base-image digests, fetched git commit SHA + remote URL, any `ADD <url>` fetched URLs with checksums.
- `runDetails.metadata.invocationId`, `startedOn`, `finishedOn` — UTC RFC 3339 timestamps.
- `runDetails.builder.version` — `zlayer-builder` crate version map.
- `runDetails.byproducts` — pointer to logs blob digest if we persist build logs.

`--provenance=min` emits required fields only; `max` adds all recommended fields; `none` skips emission entirely.

### Mount-type-secret/ssh specs

Per [Dockerfile reference](https://docs.docker.com/reference/dockerfile/):

**`--mount=type=secret`** — options: `id`, `target`/`dst`/`destination` (default `/run/secrets/<id>`), `required` (default `false`), `mode` (octal, default `0400`), `uid` (default `0`), `gid` (default `0`), `env` (mount to env var instead of / in addition to file, v1.10.0+). Sourced from `docker build --secret id=foo,src=/path` or `--secret id=foo,env=VAR`. Secrets do NOT invalidate layer cache — only the secret `id` enters the cache key, not contents.

**`--mount=type=ssh`** — options: `id` (default `"default"`), `target`/`dst` (default `/run/buildkit/ssh_agent.<N>` where N counts ssh mounts per RUN), `required` (default `false`), `mode` (default `0600`), `uid` (default `0`), `gid` (default `0`). Sourced from `docker build --ssh default=$SSH_AUTH_SOCK` or `--ssh <id>=<key-path>`. SSH state never enters the cache key.

The `RunMount::Secret` and `RunMount::Ssh` enum arms at `crates/zlayer-builder/src/dockerfile/instruction.rs:405,411` exist (Phase A parsing); G4/G5 add executor wiring + the `--secret`/`--ssh` CLI flags + the cache-key isolation property.

### Files to create

- `crates/zlayer-types/src/build/attestation.rs` — attestation types.
- `crates/zlayer-builder/src/sbom.rs` — SBOM generation.
- `crates/zlayer-builder/src/provenance.rs` — SLSA predicate construction.
- `crates/zlayer-builder/tests/sbom_test.rs` — SBOM fixture test.
- `crates/zlayer-builder/tests/provenance_test.rs` — provenance shape test.
- `crates/zlayer-builder/tests/referrer_push_test.rs` — referrer fallback test.

### Files to modify

- `crates/zlayer-types/src/build/mod.rs` — export new types.
- `crates/zlayer-api/src/handlers/build.rs` + `openapi.rs` — request/response.
- `crates/zlayer-builder/src/builder.rs` — orchestration, cache parser, inline annotations.
- `crates/zlayer-builder/src/backend/executor.rs` (whichever native executor module Phases A–F landed) — secret/ssh mount dispatch.
- `crates/zlayer-builder/src/dockerfile/instruction.rs` — only if cache-key logic needs adjustment to exclude secret content.
- `crates/zlayer-registry/src/client.rs` — `push_referrer`, `list_referrers`, fallback tag.
- `crates/zlayer-registry/src/oci_export.rs` — inline cache annotations.
- `bin/zlayer/src/cli.rs` + `bin/zlayer/src/commands/build.rs` — new CLI flags.
- `CHANGELOG.md` — `[Unreleased]` entry.

### Tests

Unit (in each module's `#[cfg(test)]`):

- Secret mount cache key invariant — same `id`, different `src` content → same hash.
- SSH mount cache key invariant — same `id`, agent off vs on → same hash.
- Cache spec parser — all four `type=` forms parse to correct `CacheBackendConfig`.
- Inline cache annotation round-trip — write then read recovers source digests.
- CycloneDX BOM fixture — apt-style fixture rootfs → exact expected component list.
- SLSA predicate — `buildDefinition.buildType == "https://zlayer.dev/build/v1"`, required fields present at `min`, all recommended fields present at `max`.

Integration (`crates/zlayer-builder/tests/`):

- End-to-end: `zlayer build --sbom=cyclonedx --provenance=min -t test:g` against a fixture context, push to a local registry (existing test infra), confirm `GET /v2/test/referrers/<digest>` returns both attestations.
- Referrer 404 fallback: same flow against a registry that returns 404 on `/referrers/`, confirm fallback tag is created.

### Risks

- **Native SBOM scope creep.** Writing accurate package-db parsers for apt/apk/rpm/npm/pip/cargo/go is multi-engineer-month work; syft has a multi-year head start. If user picks native, scope must shrink to apt + apk + a handful of language lockfiles, with explicit gaps. This is the largest single risk in the phase.
- **Provenance field accuracy.** Wrong `resolvedDependencies` is worse than absent — verifiers will trust it. Need to be conservative: only emit dependencies we actually resolved (pulled base image digests, git SHAs we read), never best-guess.
- **Inline cache compat.** BuildKit's `containerimage.buildinfo` annotation is under-specified outside Moby source. Risk: our format reads back in zlayer but doesn't interop with buildx. Acceptable if user agrees zlayer-only interop is fine for inline; otherwise targets `--cache-to=type=registry` (already shipped) as the cross-tool path.
- **Referrers API support varies.** ghcr.io, ECR Public, and Harbor ≥2.8 support it; older Harbor and some self-hosted Distribution deployments don't. Fallback tag scheme must be exercised in tests.
- **`spdx-rs` is parse-focused** — emitting SPDX from scratch in Rust is less ergonomic than CycloneDX. If user requires SPDX-first, scope grows.

### Open questions for the user

1. **SBOM generation strategy** — pick one:
   - **(a) Shell out to `syft`** (Apache-2.0, Go binary, one-shot scanner, no daemon): smallest scope, broadest ecosystem coverage day one, but reintroduces a Go binary dependency in the build path. Honest take: this is genuinely different from the buildah situation — syft is a stateless scanner invoked once per build that we can vendor as a release-time download, not a long-running daemon participating in the container lifecycle. User has been firm on no-Go; this is the case to revisit that with eyes open.
   - **(b) Native Rust SBOM** via `cyclonedx-bom`: zero external binaries, but coverage will be narrow (start with dpkg + apk + a few language lockfiles), and matching syft's accuracy is a multi-phase effort. Honest take: ideologically clean, materially less complete on day one.
2. **Provenance signing in this phase or next?** Plan currently emits unsigned in-toto statements. Cosign signing (sigstore keyless + Fulcio) is ~1 additional week. Defer to Phase G+1 unless user wants it folded in now.
3. **SBOM/provenance default-on?** Should `zlayer build` produce attestations by default (`--sbom=cyclonedx --provenance=min` as defaults), or opt-in (`--sbom=none --provenance=none`)? Buildx defaults to off; ko/docker-buildx-attest default to on.
4. **Inline cache: BuildKit-compatible or zlayer-only format?** If we want buildx interop on `--cache-to=type=inline`, we must reverse-engineer Moby's `containerimage.buildinfo` annotation precisely. If zlayer-only is acceptable, we can use a cleaner zlayer-native schema.

### Verification

After all 12 agents land, the workspace must be green per project rule (entire workspace, never `-p`):

```
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace
```

End-to-end smoke: `zlayer build examples/<sample> -t test:g --sbom=cyclonedx --provenance=min --push localhost:5000/test:g` then `curl -s http://localhost:5000/v2/test/referrers/<digest> | jq` returns both manifests with correct `artifactType` and `subject` fields. Repeat against a referrers-404 registry to exercise fallback tags.

### Estimated effort

- Sum of step hours: 49–53h (G6 native = 10h, G6 syft = 6h).
- Calendar: **4–5 person-weeks** at typical pace including review, two-pass agent runs, and CHANGELOG/test polish. Toward the lower end of the original 4–6 week estimate if user picks syft (G1a), upper end if user picks native (G1b).
- Risk level: **medium**, dominated by Question 1's answer. Native SBOM (G1b) is the single biggest scope driver.
