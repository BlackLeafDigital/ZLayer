//! Daemon-wide container event bus.
//!
//! Provides a broadcast channel for container lifecycle events (start, die,
//! oom, health) that streams to SSE subscribers at `GET /api/v1/events`.
//!
//! # Architecture
//!
//! The bus wraps [`tokio::sync::broadcast::Sender`] with a fixed buffer
//! capacity (1024). Events are published from API-layer container lifecycle
//! handlers (create/start/stop/kill/delete) and fan out to any active
//! subscribers. Slow subscribers that lag beyond the buffer are dropped with
//! a `close` event by the SSE handler -- they are expected to reconnect.
//!
//! # Backpressure
//!
//! The channel never blocks publishers. When the buffer is full,
//! [`tokio::sync::broadcast`] silently overwrites the oldest messages and the
//! subscriber receives [`tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged`]
//! instead of the dropped events. Publishers use `send` and ignore the
//! `SendError` that indicates no active subscribers -- the bus is fire-and-
//! forget.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use utoipa::ToSchema;

/// Buffer capacity for the broadcast channel.
///
/// Sized to absorb bursts of lifecycle events during a rolling restart
/// of a medium-sized deployment without dropping messages for attentive
/// subscribers. A laggy subscriber that falls behind more than this many
/// events will be closed by the SSE handler.
pub const EVENT_BUS_CAPACITY: usize = 1024;

/// Kind of container lifecycle event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ContainerEventKind {
    /// Container transitioned into the running state.
    Start,
    /// Container exited (graceful or signaled).
    Die,
    /// Container was killed by the OOM killer.
    Oom,
    /// Container health-check status changed.
    Health,
}

impl ContainerEventKind {
    /// Returns the SSE `event:` field name for this kind.
    ///
    /// Mirrors the Docker-compat wire format: `container.start`,
    /// `container.die`, `container.oom`, `container.health`.
    #[must_use]
    pub const fn sse_name(self) -> &'static str {
        match self {
            Self::Start => "container.start",
            Self::Die => "container.die",
            Self::Oom => "container.oom",
            Self::Health => "container.health",
        }
    }
}

/// A container lifecycle event published on the bus.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ContainerEvent {
    /// What kind of transition this event represents.
    pub kind: ContainerEventKind,
    /// Container identifier (the API's id string, not the raw runtime id).
    pub id: String,
    /// Labels on the container at the time of the event. Used by subscribers
    /// to filter via the `label=k=v` query param (AND semantics).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
    /// Exit code, when known. Populated for `Die` events where a wait has
    /// already resolved; otherwise `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Free-form human-readable reason. For `Die`, may indicate "stopped",
    /// "killed", "oom-killed"; for `Oom`, the OOM detail; for `Health`, may
    /// echo the probe failure message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Health status, only populated for `Health` events (e.g. "healthy",
    /// "unhealthy", "starting").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Wall-clock time the event was emitted.
    ///
    /// Serialized as an RFC 3339 string on the wire. Typed as
    /// [`chrono::DateTime<Utc>`] in-process for ergonomic manipulation;
    /// the `#[schema(value_type = String, format = DateTime)]` attribute
    /// teaches `utoipa` to render a proper string schema in the `OpenAPI`
    /// output.
    #[schema(value_type = String, format = DateTime)]
    pub at: DateTime<Utc>,
}

impl ContainerEvent {
    /// Build a `Start` event with the given id and labels.
    #[must_use]
    pub fn start(id: impl Into<String>, labels: HashMap<String, String>) -> Self {
        Self {
            kind: ContainerEventKind::Start,
            id: id.into(),
            labels,
            exit_code: None,
            reason: None,
            status: None,
            at: Utc::now(),
        }
    }

    /// Build a `Die` event with the given id, labels, and optional exit code
    /// / reason.
    #[must_use]
    pub fn die(
        id: impl Into<String>,
        labels: HashMap<String, String>,
        exit_code: Option<i32>,
        reason: Option<String>,
    ) -> Self {
        Self {
            kind: ContainerEventKind::Die,
            id: id.into(),
            labels,
            exit_code,
            reason,
            status: None,
            at: Utc::now(),
        }
    }

    /// Build an `Oom` event with the given id, labels, and optional exit code
    /// / reason. Emitted alongside `Die` when the runtime reports
    /// `state.oom_killed == true` from the wait-outcome path.
    #[must_use]
    pub fn oom(
        id: impl Into<String>,
        labels: HashMap<String, String>,
        exit_code: Option<i32>,
        reason: Option<String>,
    ) -> Self {
        Self {
            kind: ContainerEventKind::Oom,
            id: id.into(),
            labels,
            exit_code,
            reason,
            status: None,
            at: Utc::now(),
        }
    }

    /// Returns true if this event passes the given label filter.
    ///
    /// Filter semantics: AND -- an event passes only if for every `(k, v)`
    /// pair in `filter`, the event's labels contain `k` mapped to `v`. An
    /// empty filter accepts all events.
    #[must_use]
    pub fn matches_labels(&self, filter: &[(String, String)]) -> bool {
        filter
            .iter()
            .all(|(k, v)| self.labels.get(k).is_some_and(|actual| actual == v))
    }
}

/// Broadcast bus for container lifecycle events.
///
/// Cheap to clone (it wraps an `Arc` internally). Stored on
/// [`crate::handlers::containers::ContainerApiState`] so every handler has
/// access.
#[derive(Clone)]
pub struct ContainerEventBus {
    sender: Arc<broadcast::Sender<ContainerEvent>>,
}

impl ContainerEventBus {
    /// Create a new bus with the default capacity of [`EVENT_BUS_CAPACITY`].
    #[must_use]
    pub fn new() -> Self {
        let (sender, _rx) = broadcast::channel(EVENT_BUS_CAPACITY);
        Self {
            sender: Arc::new(sender),
        }
    }

    /// Create a new bus with an explicit capacity. Useful for tests.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        let (sender, _rx) = broadcast::channel(capacity);
        Self {
            sender: Arc::new(sender),
        }
    }

    /// Subscribe to all future events on the bus.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<ContainerEvent> {
        self.sender.subscribe()
    }

    /// Publish an event on the bus.
    ///
    /// Fire-and-forget: errors from [`broadcast::Sender::send`] (no active
    /// subscribers) are intentionally ignored.
    pub fn publish(&self, event: ContainerEvent) {
        let _ = self.sender.send(event);
    }

    /// Returns the current number of active subscribers. Useful for tests
    /// and diagnostics.
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for ContainerEventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ContainerEventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContainerEventBus")
            .field("subscriber_count", &self.subscriber_count())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn matches_labels_empty_filter_passes() {
        let ev = ContainerEvent::start("c1", labels(&[("app", "web")]));
        assert!(ev.matches_labels(&[]));
    }

    #[test]
    fn matches_labels_and_semantics() {
        let ev = ContainerEvent::start(
            "c1",
            labels(&[("app", "web"), ("env", "prod"), ("tier", "api")]),
        );

        // Single-match filters pass.
        assert!(ev.matches_labels(&[("app".to_string(), "web".to_string())]));

        // Multi-key filter: all must match (AND).
        assert!(ev.matches_labels(&[
            ("app".to_string(), "web".to_string()),
            ("env".to_string(), "prod".to_string()),
        ]));

        // Miss on any key fails the whole filter.
        assert!(!ev.matches_labels(&[
            ("app".to_string(), "web".to_string()),
            ("env".to_string(), "dev".to_string()),
        ]));

        // Missing key fails.
        assert!(!ev.matches_labels(&[("region".to_string(), "us-east".to_string())]));
    }

    #[test]
    fn matches_labels_value_mismatch() {
        let ev = ContainerEvent::start("c1", labels(&[("app", "web")]));
        assert!(!ev.matches_labels(&[("app".to_string(), "worker".to_string())]));
    }

    #[tokio::test]
    async fn publish_and_subscribe_roundtrip() {
        let bus = ContainerEventBus::new();
        let mut rx = bus.subscribe();

        let ev = ContainerEvent::start("c1", labels(&[("app", "web")]));
        bus.publish(ev.clone());

        let received = rx.recv().await.expect("event delivered");
        assert_eq!(received.id, "c1");
        assert_eq!(received.kind, ContainerEventKind::Start);
    }

    #[tokio::test]
    async fn publish_without_subscribers_is_noop() {
        let bus = ContainerEventBus::new();
        // No subscribers: publish must not panic.
        bus.publish(ContainerEvent::start("c1", HashMap::new()));
    }

    #[test]
    fn sse_event_names() {
        assert_eq!(ContainerEventKind::Start.sse_name(), "container.start");
        assert_eq!(ContainerEventKind::Die.sse_name(), "container.die");
        assert_eq!(ContainerEventKind::Oom.sse_name(), "container.oom");
        assert_eq!(ContainerEventKind::Health.sse_name(), "container.health");
    }
}
