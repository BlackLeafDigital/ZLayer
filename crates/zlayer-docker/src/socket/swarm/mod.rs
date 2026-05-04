//! Docker Swarm endpoint surface — submodule entry point.
//!
//! Docker clients expect a flat set of endpoints under `/swarm`, `/nodes`,
//! `/services`, `/tasks`, `/secrets`, and `/configs`. `ZLayer` runs its
//! own Raft-based scheduler, but the daemon process is always a
//! cluster-of-one when up, so the handlers project that native state onto
//! Docker's Swarm wire shape rather than emit "swarm-inactive" stubs. The
//! remaining stubs are kept only where there is no `ZLayer` analogue yet.
//!
//! Each endpoint family lives in its own submodule so the bridges can
//! land piecewise. [`shape`] holds the Docker wire-format types shared
//! by all endpoint families.
//!
//! # Behaviour summary (`/swarm` family)
//!
//! * `GET /swarm` returns **200 OK** with a [`shape::Swarm`] envelope
//!   composed from the daemon's `cluster_nodes` + `overlay_status`
//!   responses; only returns 500 if both daemon calls fail.
//! * `POST /swarm/init` returns **200** with the local node id (single-
//!   node bootstrap, since the daemon is already a cluster-of-one).
//! * `POST /swarm/join` bridges to `POST /api/v1/cluster/join`. Already-
//!   a-member returns **503** with stock dockerd's wording.
//! * `POST /swarm/leave` returns **501** with a pointer at
//!   `zlayer cluster leave` (no native leave endpoint yet).
//! * `POST /swarm/update` returns **200** (no-op for spec-only changes).
//! * `GET /swarm/unlockkey` returns **200** `{"UnlockKey": ""}`;
//!   `POST /swarm/unlock` returns **200** (autolock not supported).
//!
//! # Behaviour summary (other families)
//!
//! * `/nodes`, `/secrets`, `/configs`, `/services`, `/tasks` bridge
//!   through to the daemon. See each submodule's docstring for the
//!   per-endpoint status-code map. The `/services` bridge surfaces each
//!   `(deployment, service)` pair as a single Docker Service whose `ID` is
//!   `"{deployment}.{service}"`; `/tasks` surfaces each replica
//!   container under that pair as a single Docker Task whose `ID` is the
//!   container's per-replica id.
//!
//! # Migration path
//!
//! Honouring full Swarm semantics ("real" services / tasks) would overlap
//! almost entirely with `ZLayer`'s native scheduler. The recommended path
//! remains converting Swarm services into `ZLayer` deployments rather
//! than emulating the Swarm control plane wholesale.

use axum::Router;

use super::SocketState;

mod configs;
mod nodes;
mod secrets;
mod services;
mod shape;
// The Swarm `/swarm` endpoint family lives in `swarm/swarm.rs`. Clippy's
// `module_inception` lint flags this naming, but the layout is intentional:
// each endpoint family gets its own file in the `swarm/` submodule, and the
// `/swarm` family is no exception. Allow rather than rename so the file
// names continue to match the URL paths they handle.
#[allow(clippy::module_inception)]
mod swarm;
mod tasks;

/// Re-export the `/info` Swarm-block builder so `socket::system` can
/// populate the `Swarm` field of `GET /info` from the same data source
/// (`cluster_nodes` + `overlay_status`) that drives `GET /swarm`. Keeps
/// the cluster-id derivation and manager-role logic in one place rather
/// than duplicating it on the `/info` side.
pub(in crate::socket) use swarm::build_info_swarm_json;

/// Swarm route table.
///
/// Every family now bridges to real `ZLayer` state via
/// [`zlayer_client::DaemonClient`]; each receives a clone of the shared
/// [`SocketState`].
pub(crate) fn routes(state: SocketState) -> Router {
    Router::new()
        .merge(swarm::routes(state.clone()))
        .merge(nodes::routes(state.clone()))
        .merge(services::routes(state.clone()))
        .merge(tasks::routes(state.clone()))
        .merge(secrets::routes(state.clone()))
        .merge(configs::routes(state))
}
