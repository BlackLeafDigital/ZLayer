//! Daemon-side auto-remove subscriber for `--rm` containers.
//!
//! Implements the daemon half of Docker's `HostConfig.AutoRemove` (a.k.a.
//! `docker run --rm`): when a standalone container created with
//! [`zlayer_spec::LifecycleSpec::delete_on_exit`] set to `true` exits, the
//! daemon must stop+remove the runtime bundle and drop the cached metadata
//! without waiting for an explicit `DELETE /api/v1/containers/{id}` from the
//! client.
//!
//! The subscriber attaches to the daemon-wide [`crate::event_bus::DaemonEventBus`],
//! filters for [`crate::event_bus::ContainerEventKind::Die`] events, looks the
//! reported hex id up in [`crate::handlers::container_id_map::ContainerIdMap`],
//! consults the cached
//! [`crate::handlers::containers::StandaloneContainer::delete_on_exit`] flag,
//! and on a match invokes the same delete code path the REST handler uses
//! ([`crate::handlers::containers::delete_standalone_container`]). All other
//! events are dropped.
//!
//! # Idempotence
//!
//! The subscriber is safe against:
//!
//! - Containers already removed by an earlier call (`expect_present = false`
//!   on the shared helper turns the runtime-remove `NotFound` into a debug
//!   log).
//! - Containers whose cache entry has already been evicted (the lookup
//!   returns `None` and the subscriber logs at `debug!` and continues).
//! - Hex ids that the id-map no longer knows about (same: `debug!` + skip).
//!
//! # Lag recovery
//!
//! The bus uses `tokio::sync::broadcast`, which silently drops messages for
//! laggy subscribers and returns
//! [`tokio::sync::broadcast::error::RecvError::Lagged`] on the next `recv`.
//! Auto-remove cannot tolerate dropped die events — a missed event leaks a
//! `--rm` container — so on `Lagged` the subscriber resubscribes with a fresh
//! receiver. The catch-up window between the lag and the resubscribe is
//! bounded by the bus capacity ([`crate::event_bus::EVENT_BUS_CAPACITY`]); the
//! daemon already provisions that channel large enough for a rolling restart
//! of a medium-sized deployment, so dropped die events in practice mean the
//! daemon is far more degraded than this subscriber alone can rescue.
//!
//! On `RecvError::Closed` the bus has been dropped (daemon shutdown) and the
//! subscriber exits cleanly.

use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::event_bus::{ContainerEventKind, DaemonEvent};
use crate::handlers::containers::{delete_standalone_container, ContainerApiState};

/// Spawn the daemon-side `--rm` subscriber and return its [`JoinHandle`].
///
/// The handle is detached-friendly: callers that don't care about the inner
/// task's exit can simply drop it. The task itself only exits when the
/// underlying broadcast channel is closed (i.e. the daemon's [`crate::event_bus::DaemonEventBus`]
/// has been dropped).
///
/// Idempotence: the subscriber is safe to start multiple times against the
/// same state — each clone of [`ContainerApiState`] holds the same
/// `event_bus`, `id_map`, and storage handles, so two subscribers racing on
/// the same die event will both attempt to delete the container; the second
/// one will see the cache entry gone and log at `debug!` without escalating.
/// In practice the daemon spawns exactly one.
#[must_use = "the returned JoinHandle owns the auto-remove subscriber task; dropping it cancels the task"]
pub fn start_auto_remove_subscriber(state: ContainerApiState) -> JoinHandle<()> {
    tokio::spawn(async move {
        info!("auto-remove subscriber started");
        let mut rx = state.event_bus.subscribe();
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Some((hex_id, exit_code)) = die_payload(&event) {
                        if let Err(e) = handle_die(&state, hex_id.as_str(), exit_code).await {
                            warn!(
                                error = %e,
                                hex = %hex_id,
                                "auto-remove handler returned an error",
                            );
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    // Auto-remove cannot tolerate dropped die events; resync
                    // with a fresh receiver. We do *not* try to fish missed
                    // events out of the bus — the broadcast contract drops
                    // them — so this is best-effort recovery only.
                    warn!(
                        skipped,
                        "auto-remove subscriber lagged behind the event bus; resubscribing",
                    );
                    rx = state.event_bus.subscribe();
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    debug!("auto-remove subscriber: event bus closed, exiting");
                    return;
                }
            }
        }
    })
}

/// Extract the hex id and optional exit code from a `container.die` event.
/// Returns `None` for any other event kind.
fn die_payload(event: &DaemonEvent) -> Option<(String, Option<i32>)> {
    match event {
        DaemonEvent::Container(c) if c.kind == ContainerEventKind::Die => {
            Some((c.id.clone(), c.exit_code))
        }
        _ => None,
    }
}

/// Handle a single `container.die` event. Looks the hex id up in the id-map,
/// consults the cached [`crate::handlers::containers::StandaloneContainer::delete_on_exit`]
/// flag, and on a match invokes [`delete_standalone_container`].
///
/// Returns `Ok(())` on every "happy path" outcome, including
/// no-such-container (idempotence): the only error variants surfaced are real
/// I/O / runtime failures the caller should log loudly.
async fn handle_die(
    state: &ContainerApiState,
    hex_id: &str,
    exit_code: Option<i32>,
) -> crate::error::Result<()> {
    let Some(container_id) = state.id_map.lookup_hex(hex_id) else {
        debug!(
            hex = %hex_id,
            "auto-remove: hex id no longer mapped (container already gone)",
        );
        return Ok(());
    };

    let storage_key = container_id.service.clone();
    let delete_on_exit = {
        let cache = state.containers.read().await;
        cache.get(&storage_key).is_some_and(|m| m.delete_on_exit)
    };

    if !delete_on_exit {
        debug!(
            hex = %hex_id,
            container = %container_id,
            "auto-remove: container has delete_on_exit=false, retaining",
        );
        return Ok(());
    }

    info!(
        hex = %hex_id,
        container = %container_id,
        exit_code = ?exit_code,
        "auto-remove: deleting container after die event",
    );

    delete_standalone_container(state, &container_id, &storage_key, false).await
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    use zlayer_agent::runtime::{ContainerId, Runtime};

    use super::*;
    use crate::event_bus::ContainerEvent;
    use crate::handlers::containers::StandaloneContainer;

    /// Build a `ContainerApiState` pre-populated with one running container
    /// in both the runtime and the in-memory cache, plus an `id_map`
    /// registration so [`super::handle_die`] can resolve the hex id back to
    /// the [`ContainerId`].
    async fn make_state_with_one(
        service: &str,
        delete_on_exit: bool,
    ) -> (ContainerApiState, ContainerId, String) {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(zlayer_agent::MockRuntime::new());
        let state =
            ContainerApiState::with_daemon_uuid(runtime, "auto-remove-test-uuid".to_string());
        let cid = ContainerId::new(service.to_string(), 0);
        // We deliberately do NOT call `create_container`/`start_container` on
        // the runtime here. `MockRuntime::stop_container` and
        // `MockRuntime::remove_container` are no-ops on unknown ids (they
        // silently succeed rather than returning `NotFound`), and the
        // auto-remove helper passes `expect_present=false` so a `NotFound`
        // would be absorbed anyway. Skipping create/start keeps the test
        // focused on the bookkeeping (cache + storage + id_map) the
        // subscriber actually owns.

        let hex = state.id_map.register(state.id_map.daemon_uuid(), &cid);
        let standalone = StandaloneContainer {
            container_id: cid.clone(),
            image: "alpine:latest".to_string(),
            name: Some(service.to_string()),
            labels: HashMap::new(),
            created_at: "2026-05-03T00:00:00Z".to_string(),
            delete_on_exit,
        };
        state
            .standalone_storage
            .insert(standalone.clone())
            .await
            .unwrap();
        state
            .containers
            .write()
            .await
            .insert(service.to_string(), standalone);

        (state, cid, hex)
    }

    /// Spin until `predicate` returns `true` or `timeout` elapses; fails the
    /// test on timeout. Used to wait for the spawned subscriber to react to
    /// a published die event without racing against a fixed sleep.
    async fn wait_until<F>(timeout: Duration, mut predicate: F)
    where
        F: FnMut() -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>,
    {
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            if predicate().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("wait_until timed out after {timeout:?}");
    }

    #[tokio::test]
    async fn subscriber_deletes_container_on_die_when_delete_on_exit_true() {
        let (state, cid, hex) = make_state_with_one("standalone-rm-true", true).await;

        // Sanity: cache entry, storage row, and id-map entry are all present
        // before the subscriber fires.
        assert!(state
            .containers
            .read()
            .await
            .contains_key("standalone-rm-true"));
        assert!(state
            .standalone_storage
            .get(&cid.to_string())
            .await
            .unwrap()
            .is_some());
        assert!(state.id_map.lookup_hex(&hex).is_some());

        let _handle = start_auto_remove_subscriber(state.clone());
        // Give the subscribe() call inside the spawned task a beat to run
        // before we publish — otherwise the broadcast send will land before
        // the receiver is wired up and disappear into the void.
        wait_until(Duration::from_secs(1), {
            let bus = state.event_bus.clone();
            move || {
                let bus = bus.clone();
                Box::pin(async move { bus.subscriber_count() >= 1 })
            }
        })
        .await;

        state.event_bus.publish(ContainerEvent::die(
            hex.clone(),
            HashMap::new(),
            Some(0),
            Some("exited".to_string()),
        ));

        let containers = state.containers.clone();
        let storage = state.standalone_storage.clone();
        let storage_key = cid.to_string();
        wait_until(Duration::from_secs(2), move || {
            let containers = containers.clone();
            let storage = storage.clone();
            let storage_key = storage_key.clone();
            Box::pin(async move {
                let cache_gone = !containers.read().await.contains_key("standalone-rm-true");
                let storage_gone = storage.get(&storage_key).await.unwrap().is_none();
                cache_gone && storage_gone
            })
        })
        .await;

        // id_map should have been unregistered too — a future container with
        // the same service-name must not pick up the stale hex.
        assert!(
            state.id_map.lookup_hex(&hex).is_none(),
            "id_map entry must be dropped after auto-remove",
        );
    }

    #[tokio::test]
    async fn subscriber_does_not_delete_when_delete_on_exit_false() {
        let (state, cid, hex) = make_state_with_one("standalone-rm-false", false).await;

        let _handle = start_auto_remove_subscriber(state.clone());
        wait_until(Duration::from_secs(1), {
            let bus = state.event_bus.clone();
            move || {
                let bus = bus.clone();
                Box::pin(async move { bus.subscriber_count() >= 1 })
            }
        })
        .await;

        state.event_bus.publish(ContainerEvent::die(
            hex.clone(),
            HashMap::new(),
            Some(0),
            Some("exited".to_string()),
        ));

        // Wait long enough for the subscriber to have processed the event
        // (any work it does runs on the same runtime; 200ms is generous).
        // Then assert the bookkeeping is intact: cache, storage, and id_map
        // must all still hold the original record.
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(
            state
                .containers
                .read()
                .await
                .contains_key("standalone-rm-false"),
            "cache entry must survive die event when delete_on_exit=false",
        );
        assert!(
            state
                .standalone_storage
                .get(&cid.to_string())
                .await
                .unwrap()
                .is_some(),
            "storage row must survive die event when delete_on_exit=false",
        );
        assert!(
            state.id_map.lookup_hex(&hex).is_some(),
            "id_map entry must survive die event when delete_on_exit=false",
        );
    }
}
