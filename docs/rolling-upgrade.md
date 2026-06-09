# Rolling Upgrade Runbook

## Overview

This runbook is for operators of a multi-node ZLayer cluster (two or more voting
nodes participating in Raft) who need to roll out a new ZLayer release without
taking the cluster offline. It walks through the order of operations, the
wire-format guarantees that make zero-downtime upgrades safe, and the failure
modes worth knowing about before you start.

The audience is whoever runs the cluster: SRE, platform engineering, or the
single operator on a small-team deployment. The assumed familiarity is
"comfortable with systemd, curl, and reading `journalctl`." No knowledge of
Raft internals is required, but a high-level mental model of "one leader, N
followers, majority must agree before a write commits" is useful.

If you are running a single-node install, you do not need this document. Just
`sudo systemctl stop zlayer && sudo install.sh && sudo systemctl start zlayer`
and you are done. The procedure below only matters when there is a Raft quorum
to preserve and workloads that must keep running while individual nodes
restart.

Use this runbook when you have a new ZLayer release staged and an existing
cluster of two or more voters serving live workloads. If your cluster is empty
or its workloads are tolerant of a full shutdown window, the much simpler
"stop everything, upgrade everything, start everything" path is faster and
less error-prone — the procedure here pays its complexity cost only when
zero-downtime is an actual requirement.

## Upgrade Philosophy

- **postcard2 wire/disk format is positional, append-only safe.** New Raft
  log variants must be appended to `Request` in
  `crates/zlayer-scheduler/src/raft.rs` — never inserted or reordered. The
  `// Appended last` comment at the end of `Request` enforces this
  convention. Variants encode by position, so inserting in the middle
  silently shifts every following tag and corrupts decode on any peer
  running a different binary. The append-only rule turns "what could be a
  silent corruption" into "the node noisily refuses to decode" — which is
  exactly what you want during a rolling upgrade.
- **Protocol-version handshake.** Every Raft RPC carries
  `X-ZLayer-Raft-Protocol: 1`. A server that doesn't recognize the version
  returns `426 Upgrade Required`. Major-version mismatches are rejected at
  the transport layer — they cannot corrupt state. This means a botched
  upgrade where two incompatible major versions try to talk to each other
  will stall replication rather than poison the log. Stalled replication is
  visible (`zlayer node list` shows lagging followers); poisoned state is
  invisible until it explodes.
- **Snapshot/log format is stable within a major.** Within a `0.11.x`
  series, redb's on-disk format and the snapshot bytes are compatible
  across patch versions. A new patch binary can read storage written by any
  prior patch in the same major series; rollback to a prior patch within
  the same major is safe (subject to the additive-variant rule below).
  Across majors, snapshot migration is explicit and one-way — a 0.12 binary
  reading 0.11 state may need to rewrite the snapshot, and going back is
  not supported. Always read the release notes when bumping the major
  version digit.

## Pre-Flight Checks

Run from any host that can reach the cluster API:

```bash
zlayer node list                  # confirm all nodes status: ready
# - identify the leader (role: leader)
# - note each follower's id
```

You should see every node reporting `status: ready` and exactly one node
reporting `role: leader`. Record the leader's id and each follower's id — you
will need them in the steps below. Write them down somewhere durable; do not
rely on scrolling back through terminal history mid-upgrade.

If any node shows `status: draining` or `status: dead`, resolve that BEFORE
upgrading. A drain that did not finish, or a dead node that has not been
removed from the voter set, both mean the cluster is operating below its
expected fault tolerance. Starting a rolling upgrade in that state risks
losing quorum the moment you stop the next daemon. A three-voter cluster
tolerates one missing node; if one is already `dead`, stopping a second to
upgrade it drops you to one voter — below the 2/3 majority — and writes will
block until at least two voters are back.

Other things to check before you begin:

- The new binary or tarball is staged on every node (or `install.sh` can
  reach the release artifact). Test the staging path on one node first;
  partial deploys where some nodes can fetch and others can't are a common
  source of half-finished upgrades.
- You have the `ZLAYER_JWT` exported in the shell you will run drain/undrain
  commands from. The token must have cluster-admin scope.
- The upgrade is patch-level within the current major (e.g. `0.11.23` →
  `0.11.24`). Cross-major upgrades have additional steps not covered here.
- You have a recent snapshot. The leader writes snapshots periodically; you
  can force one with `zlayer cluster snapshot` if you want a fresh restore
  point. Snapshot location is `$ZLAYER_DATA_DIR/raft/snapshots/`; copy it
  off-host if you want extra insurance.
- You have read the release notes for the version you are upgrading to,
  paying particular attention to any mention of "wire format," "Raft
  protocol," "snapshot compatibility," or "breaking change." If any of those
  appear, do not start a rolling upgrade until you understand the impact.

## Step-by-Step: Upgrading a 3-Node Cluster

The procedure is: drain a follower, upgrade it, bring it back, repeat for each
follower, then handle the leader last. With three voters this preserves a 2/3
quorum at every step. The same shape generalizes to five-voter clusters
(upgrade four followers one at a time, then the leader) — the rule is "never
have more than one voter offline simultaneously."

Plan on roughly five minutes per node for a routine patch upgrade: a minute
or two to drain, seconds to stop and replace the binary, a few more to start
and verify. A 3-node cluster takes 15-20 minutes end to end. Skipping the
drain step makes it faster but trades the disruption-free property for raw
speed, and is not what this runbook is for.

### Step 1: Drain a follower

```bash
# Replace <id> with the follower's node id from `zlayer node list`.
curl -X PUT http://<leader-host>:3669/api/v1/cluster/nodes/<id>/drain \
     -H "Authorization: Bearer $ZLAYER_JWT"
```

Wait for `zlayer ps --node <id>` to show empty (workloads evacuated to other
nodes). Draining is asynchronous; the scheduler reassigns the node's services
and jobs to the remaining voters. Cron resources reschedule on the next fire.
Do not proceed until the node is empty — stopping the daemon while it still
owns containers will cause those containers to be killed and rescheduled
abruptly, which is exactly what the drain step exists to avoid.

Drain progress is observable in two places: `zlayer ps --node <id>` lists
remaining instances on the node, and `zlayer node show <id>` reports the
draining state plus an evicted-count. If drain stalls (workloads still listed
after several minutes), see the Failure Mode Reference below for "Drain hangs"
— the most common cause is the remaining voters not having enough free
capacity to host the evicted instances.

### Step 2: Stop the daemon

```bash
sudo systemctl stop zlayer
```

The unit file sends SIGTERM and waits for graceful shutdown. The node will exit the Raft voter set's active replication loop but remains a configured voter — the leader will keep trying to replicate to it and will mark it `dead` after the heartbeat timeout.

### Step 3: Replace the binary

```bash
sudo install.sh ZLAYER_VERSION=0.11.24
# Or download the tarball manually and place in /usr/local/bin/zlayer
```

Verify: `zlayer --version` reports the new version. `install.sh` is idempotent — running it on an already-installed host updates the binary in place without touching the data directory, systemd unit, or configuration.

### Step 4: Start the daemon

```bash
sudo systemctl start zlayer
```

Watch the journal for a few seconds: `journalctl -u zlayer -f`. You should see the node log that it joined the cluster, accepted heartbeats, and caught up on any log entries it missed during the restart. Catch-up is typically sub-second; longer indicates the leader had to ship a snapshot, which is fine but worth noting.

### Step 5: Un-drain

```bash
curl -X PUT http://<leader-host>:3669/api/v1/cluster/nodes/<id>/undrain \
     -H "Authorization: Bearer $ZLAYER_JWT"
```

This re-enables the node as a scheduling target. The scheduler does not aggressively rebalance — workloads stay where they are until natural events (scale, redeploy, failure) place new instances on the freshly upgraded node. If you want immediate rebalance, redeploy the affected services.

### Step 6: Verify

```bash
zlayer node list   # the upgraded follower should show status: ready, correct version
```

Confirm the version column matches the release you installed and the status is `ready`. Do not move on to the next node until this is true.

### Step 7: Repeat for the second follower

Same procedure: drain, stop, install, start, undrain, verify. With two followers upgraded the cluster is now running the new binary on the majority of voters; the leader (still on the old version) is the only outlier.

### Step 8: Upgrade the leader

There is no graceful `transfer_leader` subcommand today. Two options:

- **Drain + restart approach (recommended for routine upgrades).** Drain the
  leader, stop the daemon. The surviving voters elect a new leader within
  ~500ms (the default election timeout). Upgrade the old leader's binary,
  start the daemon — it rejoins as a follower. Un-drain. During the ~500ms
  election window, write operations to the API will retry (the client
  library handles this) but may surface as brief 503s if you are watching
  closely. Read operations against followers are unaffected.
- **Force transfer (not recommended for routine upgrades).** `zlayer node
  force-leader --confirm CONFIRM_FORCE_LEADER` on a follower. This is
  DESTRUCTIVE — it wipes Raft storage on the recovery node and re-bootstraps
  as a single-node leader. Only use when the leader is unreachable AND won't
  come back. See README §"Disaster Recovery".

After Step 8, run `zlayer node list` one final time. Every node should report
the new version, `status: ready`, and exactly one node should be `role:
leader`. The role assignment is not deterministic — the new leader will most
likely be one of the followers you upgraded earlier in the run, not the node
that was the leader before. That is expected.

If `zlayer node list` shows no leader at all (every node `role: follower` or
`role: candidate`), election has stalled. Check that the upgraded nodes can
reach each other on the Raft transport port. A misconfigured firewall rule
introduced during the upgrade window is a common cause; rolling back the
firewall change is faster than rolling back ZLayer.

## Failure Mode Reference

| Scenario | Behavior | Operator action |
|----------|----------|-----------------|
| Upgrade adds a new `Request` enum variant (additive) | Old nodes can decode prior entries but a new-variant log entry from the new leader fails to deserialize → openraft treats as fatal `StorageError` and shuts the node down | Upgrade the remaining old nodes; cluster keeps quorum as long as the majority is on the new version |
| Upgrade reorders or inserts a `Request` variant (NOT additive) | Old and new nodes disagree on every log entry. Silent decode-offset corruption is possible. **This must never ship.** | If shipped: restore from snapshot on each node, drain through Raft membership change |
| Protocol-version major bump (e.g. `X-ZLayer-Raft-Protocol: 2`) | Server returns 426 on every cross-version RPC. Replication stalls, no corruption. | Upgrade all nodes within the same patch series before bumping the protocol |
| Heartbeat skew across versions | Each node sends heartbeats every 5s; leader marks dead after 30s. Upgrades that take <30s have no impact. Longer upgrades cause a brief `status: dead` flap during restart, then automatic recovery (per README:387). | Ignore unless `dead` persists more than 60s |
| Drain hangs with workloads still on the node | Scheduler cannot place evicted instances because remaining nodes lack resources or have constraints (taints, affinity) that exclude them | Inspect `zlayer ps --node <id>`; either add capacity to other nodes, relax constraints, or accept the disruption and force-stop |
| New binary fails to start after install | Usually a config-incompatibility or a left-over PID file; the data directory itself is not the problem within a major series | Check `journalctl -u zlayer -n 200`; if the daemon refuses to load existing state, restore from snapshot rather than wiping storage |

## Rollback

- Reinstall the prior version: `install.sh ZLAYER_VERSION=<prev>`.
- Rollback works cleanly for additive-variant changes — the old binary's
  `Request` enum will see the new variant tag, fail to decode, and the node
  shuts down rather than corrupting. The surviving cluster majority
  continues. This is the postcard2 append-only invariant doing its job:
  rollback fails loudly, never silently.
- DO NOT rollback across a non-additive change (reordered enum variants,
  breaking schema). Restore from snapshot instead. A non-additive change
  should never have shipped in the first place, but if it did, the recovery
  path is snapshot restore, not rollback.
- Rollback order is the reverse of upgrade order: leader first (via drain +
  restart so a follower becomes the new leader on the old binary), then
  each follower. The same drain/stop/replace/start/undrain procedure
  applies.
- After rollback, run `zlayer node list` and confirm every node is on the
  prior version. Investigate the failure that triggered the rollback before
  attempting a fresh upgrade — re-running the same broken release will
  produce the same result.
- Keep one node un-rolled-back if you can afford to, so post-mortem
  diagnostics have a live host running the buggy version. A failed upgrade
  is the most informative point in time to capture logs, snapshot bytes,
  and `journalctl` output for the engineering team.

## What the runbook does NOT do (yet)

- No `zlayer self-update` subcommand exists. Binary replacement is manual or scripted via `install.sh`. Tracking issue: TBD.
- No graceful leader transfer (`zlayer cluster transfer-leadership <id>`). Drain + restart is the current pattern.
- No automated mid-upgrade canary detection. Watch `zlayer node list` between steps.
- No built-in pre-upgrade compatibility check. Operators are expected to read the release notes for postcard2 enum changes before deploying across a cluster.
- No automated rollback orchestration. Rollback is the same procedure run with an older version number.

## See also

- README §"Multi-Node Clustering" (architecture overview).
- README §"Disaster Recovery" (when the leader is permanently lost).
- `crates/zlayer-scheduler/src/raft.rs` line 113-122 comment (append-only enum rule).
- `crates/zlayer-scheduler/tests/` (in-process multi-node test harness backing these behaviors).
- `crates/zlayer-manager/tests/e2e/scripts/run-suite.py` suites `cluster_3node` and `cluster_failover` (process-level e2e against real `zlayer serve` instances).
