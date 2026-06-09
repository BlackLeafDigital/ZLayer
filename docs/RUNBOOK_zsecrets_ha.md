# RUNBOOK — ZLayer Secrets HA (`zsecrets`)

Bring-up runbook for the `zlayer serve --secrets-only` daemon as a 2-node Raft
cluster behind **secrets.blackleafdigital.com**.

Every step is marked:

- **[CODE-done]** — the code/artifact exists in this repo; nothing to author.
- **[NEEDS-OPERATOR]** — a human must run a command, set a secret, flip DNS,
  or make a deployment decision. Not yet executed by this work.

Topology: **leader = BlackLeafCloud**, **follower = DedicatedAlpha**. Ports:
`3669/tcp` (secrets API + Raft-cluster API), `9000/tcp` (Raft consensus),
`51420/udp` (boringtun overlay for inter-node Raft).

> **Quorum note.** A 2-node Raft cluster tolerates **zero** node failures for
> writes (quorum = 2). It gives you HA *reads* and a hot standby, not
> write-availability during a node outage. For real fault tolerance add a
> third secrets node (`role=secrets`) and repeat the join step — quorum
> becomes 2-of-3. **[NEEDS-OPERATOR decision]**

---

## 0. Artifacts in this repo

- **[CODE-done]** `--secrets-only` flag on `zlayer serve`
  (`bin/zlayer/src/cli.rs`, gated router in
  `bin/zlayer/src/commands/serve.rs`, lean base router
  `zlayer_api::build_router_secrets_only_base`).
- **[CODE-done]** `images/Dockerfile.zlayer-secrets` — slim image,
  `CMD ["serve","--secrets-only","--bind","0.0.0.0:3669"]`.
- **[CODE-done]** `zsecrets.zlayer.yml` — 2-replica dedicated-node deployment
  spec (validated with `zlayer validate`).
- **[CODE-done]** `crates/zlayer-secrets-client` + `crates/secrets-client-api`
  — the reqwest client + shared trait consumers read secrets through.

---

## 1. Provision the shared JWT secret  **[NEEDS-OPERATOR]**

The secrets daemons issue HS256 session JWTs. Every node MUST share the same
`ZLAYER_JWT_SECRET`, or a token minted on the leader fails to validate on the
follower.

```bash
# Generate once; store in Vaultwarden (ZStack collection) as zlayer-jwt-secret.
openssl rand -hex 64
```

Then make it resolvable to the `$S:zlayer-jwt-secret` placeholder in
`zsecrets.zlayer.yml` (Komodo Variable today, or `zlayer secret set` once the
secrets cluster is itself the backend). **[NEEDS-OPERATOR]**

---

## 2. Build + publish the image  **[NEEDS-OPERATOR]**

BlackLeafCloud is x86_64 — build natively for amd64 (do NOT cross-build on the
Mac arm64 host; match the zSign deploy gotcha).

```bash
# On an amd64 builder:
cargo build --release --package zlayer
docker build -f images/Dockerfile.zlayer-secrets \
  -t forge.blackleafdigital.com/blackleafdigital/zlayer-secrets:latest .
docker push forge.blackleafdigital.com/blackleafdigital/zlayer-secrets:latest
```

---

## 3. Open the inter-node ports  **[NEEDS-OPERATOR]**

Between BlackLeafCloud and DedicatedAlpha (over NetBird / the overlay fabric,
NOT the public internet):

| Port        | Proto | Purpose                                   |
|-------------|-------|-------------------------------------------|
| `3669/tcp`  | TCP   | Secrets API + Raft-cluster control API    |
| `9000/tcp`  | TCP   | Raft consensus (log replication, votes)   |
| `51420/udp` | UDP   | boringtun overlay (carries Raft traffic)  |

If the two nodes already share an encrypted NetBird mesh, restrict these to
the NetBird interface and pass `--host-network` to skip the ZLayer overlay
entirely (then `51420/udp` and `wireguard-tools` are unnecessary — see the
Dockerfile header). **[NEEDS-OPERATOR decision: ZLayer overlay vs NetBird-only]**

---

## 4. Initialize the leader (BlackLeafCloud)  **[NEEDS-OPERATOR]**

Run on BlackLeafCloud. Label the node `role=secrets` so the deployment's
`node_selector` matches it.

```bash
zlayer node init \
  --advertise-addr <BlackLeafCloud-overlay-IP> \
  --api-port 3669 \
  --raft-port 9000 \
  --overlay-port 51420
# (label the node role=secrets via your node-labeling flow / `--labels role=secrets`
#  on the daemon that serves it)
```

`node init` prints a **join token** — capture it for step 5. It also
establishes this node as the Raft leader and writes `node_config.json` under
the data dir.

> The secrets daemon itself is started by the deployment in step 7
> (`zlayer serve --secrets-only`). `node init` is the cluster-bootstrap step;
> `serve --secrets-only` is the long-running API process. On a single-host
> Komodo stack the same binary does both — init first, then serve.

---

## 5. Join the follower (DedicatedAlpha)  **[NEEDS-OPERATOR]**

Run on DedicatedAlpha with the token from step 4:

```bash
zlayer node join <BlackLeafCloud-overlay-IP>:3669 \
  --token <JOIN_TOKEN_FROM_STEP_4> \
  --advertise-addr <DedicatedAlpha-overlay-IP> \
  --mode replicate \
  --api-port 3669 \
  --raft-port 9000 \
  --overlay-port 51420
# label this node role=secrets as well.
```

`--mode replicate` makes this a replicating follower. The join handler seals
the cluster DEK to this node's keypair and writes **`wrapped_dek.bin`** into
`{data_dir}/secrets/`.

---

## 6. Verify the sealed DEK on every node  **[NEEDS-OPERATOR]**

`select_secrets_store` switches a node to `RaftSecretsStore` **iff**
`{data_dir}/secrets/wrapped_dek.bin` exists. Confirm it on BOTH nodes:

```bash
ls -l /var/lib/zlayer/secrets/wrapped_dek.bin   # must exist on leader AND follower
```

If it is missing on a node, that node is still in standalone
`PersistentSecretsStore` mode and is NOT participating in HA secret
replication. Re-run / re-check the join before proceeding. Also confirm the
node keypair and SQLite DB live alongside it under `{data_dir}/secrets/` (the
named volume in `zsecrets.zlayer.yml` persists exactly this directory).

Sanity-check cluster health:

```bash
curl -fsS http://<node>:3669/health/ready   # 200 on each node
# /api/v1/cluster (mounted in secrets-only mode) reports peers + leader.
```

---

## 7. Deploy the steady-state daemons  **[NEEDS-OPERATOR]**

With the cluster formed and `wrapped_dek.bin` present on both nodes:

```bash
zlayer deploy -f zsecrets.zlayer.yml
```

This runs the `zlayer-secrets` image (`serve --secrets-only`) as 2
dedicated-node replicas pinned by `role=secrets`, each on its own
`zsecrets-data` volume, with `/health/ready` health checks.

---

## 8. Point the edge at the secrets API  **[NEEDS-OPERATOR]**

Repoint **secrets.blackleafdigital.com** at the secrets endpoint.

Per the `blackleafcloud wildcard DNS → NetBird` gotcha: `*.blackleafcloud.com`
CNAMEs to NetBird, but a public hostname like `secrets.blackleafdigital.com`
needs an **explicit proxied A record → 65.109.61.41** (the BlackLeafCloud edge)
or it will 502.

- **[NEEDS-OPERATOR]** Cloudflare: create/repoint
  `secrets.blackleafdigital.com` → proxied A `65.109.61.41` (orange cloud).
- The edge proxy routes `:443 secrets.blackleafdigital.com` → the `zsecrets`
  service `https` endpoint (`:3669`). The secrets-only router serves
  `/api/v1/secrets` and the RBAC/health surface there.

Verify end-to-end:

```bash
curl -fsS https://secrets.blackleafdigital.com/health/ready   # expect 200
```

---

## 9. Auth reconciliation — ZAuth ES256 vs ZLayer HS256  **[NEEDS-OPERATOR decision]**

**This is a required decision before consumers (ZBilling, ZRegistry, …) can
read through `ZLayerSecretsClient`.**

The split:

- **ZLayer Secrets** authenticates every `/api/v1/*` request (except
  `/health`) with its **own HS256 JWT** (`ZLAYER_JWT_SECRET`, the shared
  secret from step 1). `ZLayerSecretsClient` presents this as
  `Authorization: Bearer <token>`.
- **ZAuth** issues **ES256** (asymmetric, JWKS-verified) tokens — a different
  algorithm and a different trust root. ZLayer does NOT today validate ZAuth
  ES256 tokens on its secrets routes.

So a ZAuth-issued ES256 token will NOT authenticate against the ZLayer secrets
API as-is. Pick ONE:

- **Option A — Front-proxy bridge (recommended for go-live).** Terminate
  ZAuth ES256 at the edge in front of `secrets.blackleafdigital.com`; the
  proxy validates the ZAuth token (JWKS) and injects a short-lived ZLayer
  HS256 bearer (minted with the shared secret) on the upstream hop. Consumers
  authenticate with ZAuth; ZLayer never sees ES256.
- **Option B — Service bearer per consumer.** Issue each consumer a long-lived
  ZLayer HS256 bearer (its own identity, scoped grants via the RBAC routes)
  and hand it to `ZLayerSecretsClient::new(base_url, bearer)`. Simplest path;
  no ZAuth involvement on the secrets read path. Token lives in Vaultwarden +
  the consumer's Komodo Variables.
- **Option C — Teach ZLayer to validate ES256/JWKS.** Add ZAuth's JWKS as a
  trusted verifier on the ZLayer auth middleware. Most work; only worth it if
  unifying on ZAuth tokens cluster-wide.

**[NEEDS-OPERATOR decision]** — the code supports B out of the box (bearer
token in, no changes). A and C require additional wiring (edge config for A; a
ZLayer auth-middleware change for C).

---

## Rollback

- `zlayer deploy` is declarative — redeploy a previous image tag to roll back
  the daemons.
- DNS: repoint `secrets.blackleafdigital.com` back to its prior target.
- The Raft cluster + sealed DEKs persist on the named volumes; tearing down
  the deployment does NOT wipe `{data_dir}/secrets/` unless the volume is
  explicitly deleted.
