//! Startup reconciliation for standalone containers.
//!
//! On daemon boot the API server holds three pieces of state that can drift
//! out of sync with reality between restarts:
//!
//! 1. The persistent [`StandaloneContainerStorage`] — survives restarts.
//! 2. The in-memory [`ContainerApiState::containers`] cache — empty on boot.
//! 3. The [`ContainerIdMap`] — also empty on boot.
//!
//! Meanwhile the underlying container runtime keeps its own inventory (Docker
//! tracks containers in its daemon, youki keeps state on disk under
//! `/run/youki`, etc.), and that inventory is the source of truth for whether
//! a previously-created standalone container actually still exists.
//!
//! This module implements [`reconcile_standalone_containers`], which:
//!
//! - Calls [`Runtime::list_containers`] to learn what the runtime currently
//!   reports, indexed by the value of the `com.zlayer.container_id` label
//!   (see [`ZLAYER_CONTAINER_ID_LABEL`]).
//! - Walks every entry in the standalone-container storage, computes its
//!   expected hex id from the daemon UUID + native [`ContainerId`], and:
//!     - Prunes the storage entry (and clears the cache + id-map) when the
//!       runtime no longer reports a container with that hex label.
//!     - Otherwise registers the entry in the [`ContainerIdMap`]
//!       idempotently (a no-op when already mapped).
//! - Repopulates [`ContainerApiState::containers`] from the surviving
//!   storage records via the existing
//!   [`ContainerApiState::repopulate_cache_from_storage`] helper so list /
//!   inspect / delete handlers see the post-reconcile state.
//! - Counts runtime containers that carry a `ZLayer` label but have no
//!   matching storage row — "orphans". These are logged but **not**
//!   automatically registered: the storage layer is the system of record
//!   for what `ZLayer` "owns", and silently adopting an orphan would let a
//!   stale runtime container masquerade as a fresh standalone request.
//!
//! The reconcile pass is intended to run exactly once during daemon
//! startup, after [`ContainerApiState::with_standalone_storage`] has
//! attached the persistent backend.

use tracing::{debug, info, warn};
use zlayer_agent::AgentError;

use crate::error::{ApiError, Result};
use crate::handlers::container_id_map::{compute_hex, ZLAYER_CONTAINER_ID_LABEL};
use crate::handlers::containers::ContainerApiState;

/// Outcome of a single reconcile pass.
///
/// All counts are post-reconcile and refer to the standalone-container
/// storage list only — they intentionally exclude runtime containers that
/// don't carry a `com.zlayer.container_id` label (those are foreign and
/// completely invisible to `ZLayer`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Storage entries whose hex label was found in the runtime listing.
    /// These were re-registered in the [`ContainerIdMap`].
    pub matched: usize,
    /// Storage entries pruned because the runtime no longer reports a
    /// container with the matching hex label. The corresponding
    /// `ContainerIdMap` entry was also removed if present.
    pub pruned: usize,
    /// Runtime containers that carry a `com.zlayer.container_id` label but
    /// have no matching storage row. These were observed and logged but
    /// **not** registered in the [`ContainerIdMap`] — adopting unknown
    /// runtime state at boot is not safe.
    pub orphans_seen: usize,
}

/// Perform a single startup reconciliation pass for standalone containers.
///
/// Idempotent: callers may invoke this multiple times safely, though in
/// production it is expected to run exactly once during daemon boot.
///
/// # Errors
///
/// Returns [`ApiError::Internal`] if the runtime's
/// [`Runtime::list_containers`](zlayer_agent::runtime::Runtime::list_containers)
/// fails for any reason other than a per-container `NotFound` (which is
/// treated as "the runtime really doesn't have this container, prune it").
/// Storage errors are surfaced via the existing
/// [`From<StorageError> for ApiError`](crate::error::ApiError) conversion.
pub async fn reconcile_standalone_containers(state: &ContainerApiState) -> Result<ReconcileReport> {
    // 1. Snapshot what the runtime currently reports. The default trait
    //    implementation returns an empty Vec for runtimes that don't
    //    support enumeration; in that case orphan detection degrades to a
    //    no-op and we fall back to per-id `container_state` probes for
    //    pruning.
    let runtime_listing = state
        .runtime
        .list_containers()
        .await
        .map_err(|e| ApiError::Internal(format!("runtime list_containers failed: {e}")))?;

    let mut runtime_zlayer_labels: std::collections::HashSet<String> =
        std::collections::HashSet::with_capacity(runtime_listing.len());
    for summary in &runtime_listing {
        if let Some(label) = &summary.zlayer_container_id_label {
            runtime_zlayer_labels.insert(label.clone());
        }
    }

    // 2. Walk every storage entry and decide prune-vs-keep.
    let entries = state.standalone_storage.list().await?;
    let mut report = ReconcileReport::default();
    let daemon_uuid = state.id_map.daemon_uuid().to_string();

    for entry in &entries {
        let expected_hex = compute_hex(&daemon_uuid, &entry.container_id);

        // Two-stage existence check:
        //
        //   (a) Runtime listing reports a container with the matching
        //       label — fast path, no extra runtime call needed.
        //   (b) Listing didn't include it (either because the backend
        //       returns an empty default list, or because the container
        //       genuinely isn't there). Probe `container_state` to
        //       distinguish "really gone" from "runtime can't enumerate".
        let exists = if runtime_zlayer_labels.contains(&expected_hex) {
            true
        } else {
            match state.runtime.container_state(&entry.container_id).await {
                Ok(_) => true,
                Err(AgentError::NotFound { .. }) => false,
                Err(e) => {
                    // Don't punish the entire reconcile pass for a transient
                    // per-container error — log and keep the entry. Worst
                    // case the next reconcile or a subsequent API call will
                    // re-surface the issue.
                    warn!(
                        container_id = %entry.container_id,
                        error = %e,
                        "container_state probe failed during reconcile; keeping storage entry"
                    );
                    true
                }
            }
        };

        if exists {
            // Idempotent registration: returns the existing hex when the
            // container is already mapped under the same daemon UUID, so
            // this is safe to call regardless of map state.
            let registered_hex = state.id_map.register(&daemon_uuid, &entry.container_id);
            debug_assert_eq!(registered_hex, expected_hex);
            report.matched += 1;
            debug!(
                container_id = %entry.container_id,
                hex = %registered_hex,
                "reconcile: matched standalone container against runtime"
            );
        } else {
            // Storage and id-map both need cleaning so subsequent handlers
            // don't observe a half-registered ghost.
            let key = entry.container_id.to_string();
            let removed = state.standalone_storage.delete(&key).await?;
            state.id_map.unregister_by_container(&entry.container_id);
            // Also drop the in-memory cache entry if it somehow got
            // populated before reconcile ran (defence in depth — boot
            // ordering is supposed to attach storage *before* anything
            // else writes to the cache, but the cache write is cheap).
            {
                let mut cache = state.containers.write().await;
                cache.remove(&entry.container_id.service);
            }
            if removed {
                report.pruned += 1;
                info!(
                    container_id = %entry.container_id,
                    hex = %expected_hex,
                    "reconcile: pruned standalone container missing from runtime"
                );
            } else {
                // Storage already lacked the row by the time we tried to
                // delete it. That can happen if a concurrent admin tool
                // reaped it between `list` and `delete`; treat as a
                // successful prune.
                report.pruned += 1;
                debug!(
                    container_id = %entry.container_id,
                    "reconcile: storage row already gone before delete (race tolerated)"
                );
            }
        }
    }

    // 3. Count orphans: runtime containers that carry a `ZLayer` label but
    //    don't correspond to any (post-prune) storage entry. Build the
    //    "known hex" set from the surviving storage rows + their freshly
    //    computed hexes so we don't double-count entries we just pruned.
    let mut known_hexes: std::collections::HashSet<String> =
        std::collections::HashSet::with_capacity(entries.len());
    let surviving = state.standalone_storage.list().await?;
    for entry in &surviving {
        known_hexes.insert(compute_hex(&daemon_uuid, &entry.container_id));
    }
    for summary in &runtime_listing {
        if let Some(label) = &summary.zlayer_container_id_label {
            if !known_hexes.contains(label) {
                report.orphans_seen += 1;
                warn!(
                    runtime_id = %summary.runtime_id,
                    label = %label,
                    "reconcile: runtime container carries `ZLayer` label but has no storage row (orphan, not registered)"
                );
            }
        }
    }

    // 4. Repopulate the in-memory cache from the (post-prune) storage so
    //    list / inspect handlers see the reconciled view. Uses the
    //    existing helper to keep the cache-key convention in one place.
    state.repopulate_cache_from_storage().await?;

    info!(
        matched = report.matched,
        pruned = report.pruned,
        orphans_seen = report.orphans_seen,
        runtime_label_count = runtime_zlayer_labels.len(),
        "standalone-container reconcile complete"
    );

    // The label "constant must be stable" assertion is enforced by the
    // unit test in `container_id_map.rs`; reference the symbol here so
    // clippy doesn't flag the import as unused on the off chance future
    // refactoring drops the inline comparison.
    let _: &str = ZLAYER_CONTAINER_ID_LABEL;

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::net::IpAddr;
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use tokio::sync::RwLock;

    use crate::handlers::container_id_map::ContainerIdMap;
    use crate::handlers::containers::StandaloneContainer;
    use crate::storage::{InMemoryStandaloneContainerStorage, StandaloneContainerStorage};
    use zlayer_agent::cgroups_stats::ContainerStats;
    use zlayer_agent::error::{AgentError, Result as AgentResult};
    use zlayer_agent::runtime::{ContainerId, ContainerState, Runtime, RuntimeContainerSummary};
    use zlayer_observability::logs::LogEntry;
    use zlayer_spec::{PullPolicy, RegistryAuth, ServiceSpec};

    /// Test runtime that lets each test seed an explicit list of
    /// "containers I claim to have" plus a set of native `ContainerId`s
    /// that `container_state` should answer "yes, this exists" for.
    ///
    /// All other Runtime methods unwind with `Unsupported` — none of the
    /// reconcile paths exercised by these tests should call them.
    struct FakeListingRuntime {
        listing: Vec<RuntimeContainerSummary>,
        live_native_ids: std::collections::HashSet<ContainerId>,
    }

    #[async_trait]
    impl Runtime for FakeListingRuntime {
        async fn pull_image(&self, _image: &str) -> AgentResult<()> {
            Err(AgentError::Unsupported("pull_image".into()))
        }

        async fn pull_image_with_policy(
            &self,
            _image: &str,
            _policy: PullPolicy,
            _auth: Option<&RegistryAuth>,
        ) -> AgentResult<()> {
            Err(AgentError::Unsupported("pull_image_with_policy".into()))
        }

        async fn create_container(
            &self,
            _id: &ContainerId,
            _spec: &ServiceSpec,
        ) -> AgentResult<()> {
            Err(AgentError::Unsupported("create_container".into()))
        }

        async fn start_container(&self, _id: &ContainerId) -> AgentResult<()> {
            Err(AgentError::Unsupported("start_container".into()))
        }

        async fn stop_container(&self, _id: &ContainerId, _timeout: Duration) -> AgentResult<()> {
            Err(AgentError::Unsupported("stop_container".into()))
        }

        async fn remove_container(&self, _id: &ContainerId) -> AgentResult<()> {
            Err(AgentError::Unsupported("remove_container".into()))
        }

        async fn container_state(&self, id: &ContainerId) -> AgentResult<ContainerState> {
            if self.live_native_ids.contains(id) {
                Ok(ContainerState::Running)
            } else {
                Err(AgentError::NotFound {
                    container: id.to_string(),
                    reason: "not in fake runtime".into(),
                })
            }
        }

        async fn container_logs(
            &self,
            _id: &ContainerId,
            _tail: usize,
        ) -> AgentResult<Vec<LogEntry>> {
            Err(AgentError::Unsupported("container_logs".into()))
        }

        async fn exec(
            &self,
            _id: &ContainerId,
            _cmd: &[String],
        ) -> AgentResult<(i32, String, String)> {
            Err(AgentError::Unsupported("exec".into()))
        }

        async fn get_container_stats(&self, _id: &ContainerId) -> AgentResult<ContainerStats> {
            Err(AgentError::Unsupported("get_container_stats".into()))
        }

        async fn wait_container(&self, _id: &ContainerId) -> AgentResult<i32> {
            Err(AgentError::Unsupported("wait_container".into()))
        }

        async fn get_logs(&self, _id: &ContainerId) -> AgentResult<Vec<LogEntry>> {
            Err(AgentError::Unsupported("get_logs".into()))
        }

        async fn get_container_pid(&self, _id: &ContainerId) -> AgentResult<Option<u32>> {
            Ok(None)
        }

        async fn get_container_ip(&self, _id: &ContainerId) -> AgentResult<Option<IpAddr>> {
            Ok(None)
        }

        async fn list_containers(&self) -> AgentResult<Vec<RuntimeContainerSummary>> {
            Ok(self.listing.clone())
        }
    }

    fn make_state(
        runtime: Arc<dyn Runtime + Send + Sync>,
        daemon_uuid: &str,
        storage: Arc<dyn StandaloneContainerStorage>,
    ) -> ContainerApiState {
        let id_map = Arc::new(ContainerIdMap::new(daemon_uuid.to_string()));
        ContainerApiState {
            runtime,
            containers: Arc::new(RwLock::new(HashMap::new())),
            event_bus: crate::event_bus::ContainerEventBus::new(),
            bridge_networks: None,
            registry_store: None,
            id_map,
            standalone_storage: storage,
            compose_storage: Arc::new(crate::storage::InMemoryComposeProjectStorage::new()),
            exec_instances: Arc::new(crate::handlers::exec_instances::ExecInstances::new()),
            container_pty_resizers: Arc::new(dashmap::DashMap::new()),
        }
    }

    fn make_entry(service: &str, replica: u32) -> StandaloneContainer {
        StandaloneContainer {
            container_id: ContainerId::new(service.to_string(), replica),
            image: format!("registry.example.com/{service}:latest"),
            name: Some(format!("{service}-{replica}")),
            labels: HashMap::new(),
            created_at: "2026-05-03T00:00:00Z".to_string(),
            delete_on_exit: false,
        }
    }

    #[tokio::test]
    async fn prunes_storage_entry_missing_from_runtime() {
        let daemon_uuid = "daemon-test-1";
        let storage: Arc<dyn StandaloneContainerStorage> =
            Arc::new(InMemoryStandaloneContainerStorage::new());

        // One persisted standalone container that the runtime knows
        // nothing about — neither in `list_containers` nor in
        // `container_state`. Reconcile must prune it.
        let entry = make_entry("ghost-svc", 0);
        let storage_key = entry.container_id.to_string();
        storage.insert(entry).await.unwrap();

        let runtime = Arc::new(FakeListingRuntime {
            listing: Vec::new(),
            live_native_ids: std::collections::HashSet::new(),
        });
        let state = make_state(runtime, daemon_uuid, Arc::clone(&storage));

        let report = reconcile_standalone_containers(&state).await.unwrap();

        assert_eq!(report.matched, 0, "no entry should match");
        assert_eq!(report.pruned, 1, "the missing entry must be pruned");
        assert_eq!(report.orphans_seen, 0, "no orphans expected");

        // Storage row gone.
        assert!(
            storage.get(&storage_key).await.unwrap().is_none(),
            "storage row must be deleted"
        );
        // ContainerIdMap empty for that container.
        let cid = ContainerId::new("ghost-svc", 0);
        assert!(
            state.id_map.lookup_container(&cid).is_none(),
            "id_map must not have a stale registration"
        );
        // Cache empty.
        let cache = state.containers.read().await;
        assert!(cache.is_empty(), "cache must be empty after pruning");
    }

    #[tokio::test]
    async fn registers_storage_entry_present_in_runtime() {
        let daemon_uuid = "daemon-test-2";
        let storage: Arc<dyn StandaloneContainerStorage> =
            Arc::new(InMemoryStandaloneContainerStorage::new());

        let entry = make_entry("alive-svc", 3);
        let cid = entry.container_id.clone();
        storage.insert(entry.clone()).await.unwrap();

        let expected_hex = compute_hex(daemon_uuid, &cid);
        let runtime = Arc::new(FakeListingRuntime {
            listing: vec![RuntimeContainerSummary {
                runtime_id: "abc123".into(),
                zlayer_container_id_label: Some(expected_hex.clone()),
            }],
            live_native_ids: std::collections::HashSet::new(),
        });
        let state = make_state(runtime, daemon_uuid, Arc::clone(&storage));

        let report = reconcile_standalone_containers(&state).await.unwrap();

        assert_eq!(report.matched, 1);
        assert_eq!(report.pruned, 0);
        assert_eq!(report.orphans_seen, 0);

        // Storage row preserved.
        let storage_key = cid.to_string();
        assert!(
            storage.get(&storage_key).await.unwrap().is_some(),
            "storage row must survive reconcile"
        );
        // Id-map registration matches the expected hex.
        let mapped = state
            .id_map
            .lookup_container(&cid)
            .expect("id_map must have a registration");
        assert_eq!(mapped, expected_hex);
        // Cache populated by repopulate_cache_from_storage.
        let cache = state.containers.read().await;
        assert!(
            cache.contains_key(&cid.service),
            "cache must contain the surviving entry by service name"
        );
    }

    #[tokio::test]
    async fn orphan_runtime_container_is_counted_but_not_registered() {
        let daemon_uuid = "daemon-test-3";
        let storage: Arc<dyn StandaloneContainerStorage> =
            Arc::new(InMemoryStandaloneContainerStorage::new());

        // No storage rows at all.
        // Runtime reports two containers:
        //   - one with a `ZLayer` label that storage doesn't know about (orphan)
        //   - one without a `ZLayer` label at all (foreign Docker container,
        //     must be ignored entirely)
        let orphan_label =
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef".to_string();
        let runtime = Arc::new(FakeListingRuntime {
            listing: vec![
                RuntimeContainerSummary {
                    runtime_id: "orphan-runtime-id".into(),
                    zlayer_container_id_label: Some(orphan_label.clone()),
                },
                RuntimeContainerSummary {
                    runtime_id: "foreign-runtime-id".into(),
                    zlayer_container_id_label: None,
                },
            ],
            live_native_ids: std::collections::HashSet::new(),
        });
        let state = make_state(runtime, daemon_uuid, Arc::clone(&storage));

        let report = reconcile_standalone_containers(&state).await.unwrap();

        assert_eq!(report.matched, 0);
        assert_eq!(report.pruned, 0);
        assert_eq!(
            report.orphans_seen, 1,
            "exactly one orphan (foreign no-label container ignored)"
        );

        // Crucially: the orphan label must NOT have been adopted into the
        // id-map. Adopting unknown runtime containers at boot would let a
        // stale daemon-side container masquerade as a live `ZLayer` record.
        assert!(
            state.id_map.lookup_hex(&orphan_label).is_none(),
            "orphan must not be registered in id_map"
        );

        // No storage entry was created either.
        assert!(
            storage.list().await.unwrap().is_empty(),
            "storage must remain empty"
        );
    }
}
