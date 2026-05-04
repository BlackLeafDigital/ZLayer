//! Daemon-wide event bus.
//!
//! Provides a broadcast channel for daemon lifecycle events: container
//! start/die/oom/health, image pull/push/delete/tag, network
//! create/delete/connect/disconnect, and volume create/delete/mount/unmount.
//! Subscribers (notably the NDJSON `GET /api/v1/events` handler and the
//! Docker-compat `/events` endpoint) receive every event published by the
//! API-layer handlers as it happens.
//!
//! # Architecture
//!
//! The bus wraps [`tokio::sync::broadcast::Sender`] with a fixed buffer
//! capacity (1024). Events are published from API-layer lifecycle handlers
//! and fan out to any active subscribers. Slow subscribers that lag beyond
//! the buffer are dropped with a `close` event by the stream handler -- they
//! are expected to reconnect.
//!
//! # Backpressure
//!
//! The channel never blocks publishers. When the buffer is full,
//! [`tokio::sync::broadcast`] silently overwrites the oldest messages and
//! the subscriber receives
//! [`tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged`]
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
/// events will be closed by the stream handler.
pub const EVENT_BUS_CAPACITY: usize = 1024;

// ---------------------------------------------------------------------------
// Container events
// ---------------------------------------------------------------------------

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
    /// Returns the wire `event` field name for this kind.
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
    /// Serialized as an RFC 3339 string on the wire.
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

// ---------------------------------------------------------------------------
// Image events
// ---------------------------------------------------------------------------

/// Kind of image lifecycle event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ImageEventKind {
    /// An image was pulled into the local cache.
    Pull,
    /// An image was pushed to a remote registry.
    Push,
    /// An image was removed from the local cache.
    Delete,
    /// A new tag was attached to an existing image.
    Tag,
}

impl ImageEventKind {
    #[must_use]
    pub const fn sse_name(self) -> &'static str {
        match self {
            Self::Pull => "image.pull",
            Self::Push => "image.push",
            Self::Delete => "image.delete",
            Self::Tag => "image.tag",
        }
    }
}

/// An image lifecycle event.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ImageEvent {
    pub kind: ImageEventKind,
    /// Image reference (e.g. `nginx:latest` or `registry/foo/bar@sha256:...`).
    pub reference: String,
    /// Optional content digest (`sha256:...`) when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// Source reference for `Tag` events (the image being aliased).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Wall-clock time.
    #[schema(value_type = String, format = DateTime)]
    pub at: DateTime<Utc>,
}

impl ImageEvent {
    #[must_use]
    pub fn pull(reference: impl Into<String>, digest: Option<String>) -> Self {
        Self {
            kind: ImageEventKind::Pull,
            reference: reference.into(),
            digest,
            source: None,
            at: Utc::now(),
        }
    }

    #[must_use]
    pub fn push(reference: impl Into<String>, digest: Option<String>) -> Self {
        Self {
            kind: ImageEventKind::Push,
            reference: reference.into(),
            digest,
            source: None,
            at: Utc::now(),
        }
    }

    #[must_use]
    pub fn delete(reference: impl Into<String>) -> Self {
        Self {
            kind: ImageEventKind::Delete,
            reference: reference.into(),
            digest: None,
            source: None,
            at: Utc::now(),
        }
    }

    #[must_use]
    pub fn tag(source: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            kind: ImageEventKind::Tag,
            reference: target.into(),
            digest: None,
            source: Some(source.into()),
            at: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// Network events
// ---------------------------------------------------------------------------

/// Kind of network lifecycle event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum NetworkEventKind {
    Create,
    Delete,
    Connect,
    Disconnect,
}

impl NetworkEventKind {
    #[must_use]
    pub const fn sse_name(self) -> &'static str {
        match self {
            Self::Create => "network.create",
            Self::Delete => "network.destroy",
            Self::Connect => "network.connect",
            Self::Disconnect => "network.disconnect",
        }
    }
}

/// A network lifecycle event.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct NetworkEvent {
    pub kind: NetworkEventKind,
    /// Network identifier (registry id).
    pub id: String,
    /// Human-readable network name.
    pub name: String,
    /// Network driver (e.g. `bridge`, `overlay`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub driver: String,
    /// Container id, populated for `Connect`/`Disconnect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_id: Option<String>,
    /// Wall-clock time.
    #[schema(value_type = String, format = DateTime)]
    pub at: DateTime<Utc>,
}

impl NetworkEvent {
    #[must_use]
    pub fn create(
        id: impl Into<String>,
        name: impl Into<String>,
        driver: impl Into<String>,
    ) -> Self {
        Self {
            kind: NetworkEventKind::Create,
            id: id.into(),
            name: name.into(),
            driver: driver.into(),
            container_id: None,
            at: Utc::now(),
        }
    }

    #[must_use]
    pub fn delete(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            kind: NetworkEventKind::Delete,
            id: id.into(),
            name: name.into(),
            driver: String::new(),
            container_id: None,
            at: Utc::now(),
        }
    }

    #[must_use]
    pub fn connect(
        network_id: impl Into<String>,
        network_name: impl Into<String>,
        container_id: impl Into<String>,
    ) -> Self {
        Self {
            kind: NetworkEventKind::Connect,
            id: network_id.into(),
            name: network_name.into(),
            driver: String::new(),
            container_id: Some(container_id.into()),
            at: Utc::now(),
        }
    }

    #[must_use]
    pub fn disconnect(
        network_id: impl Into<String>,
        network_name: impl Into<String>,
        container_id: impl Into<String>,
    ) -> Self {
        Self {
            kind: NetworkEventKind::Disconnect,
            id: network_id.into(),
            name: network_name.into(),
            driver: String::new(),
            container_id: Some(container_id.into()),
            at: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// Volume events
// ---------------------------------------------------------------------------

/// Kind of volume lifecycle event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum VolumeEventKind {
    Create,
    Delete,
    Mount,
    Unmount,
}

impl VolumeEventKind {
    #[must_use]
    pub const fn sse_name(self) -> &'static str {
        match self {
            Self::Create => "volume.create",
            Self::Delete => "volume.destroy",
            Self::Mount => "volume.mount",
            Self::Unmount => "volume.unmount",
        }
    }
}

/// A volume lifecycle event.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VolumeEvent {
    pub kind: VolumeEventKind,
    /// Volume name.
    pub name: String,
    /// Volume driver (e.g. `local`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub driver: String,
    /// Container id, populated for `Mount`/`Unmount`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_id: Option<String>,
    /// Wall-clock time.
    #[schema(value_type = String, format = DateTime)]
    pub at: DateTime<Utc>,
}

impl VolumeEvent {
    #[must_use]
    pub fn create(name: impl Into<String>, driver: impl Into<String>) -> Self {
        Self {
            kind: VolumeEventKind::Create,
            name: name.into(),
            driver: driver.into(),
            container_id: None,
            at: Utc::now(),
        }
    }

    #[must_use]
    pub fn delete(name: impl Into<String>) -> Self {
        Self {
            kind: VolumeEventKind::Delete,
            name: name.into(),
            driver: String::new(),
            container_id: None,
            at: Utc::now(),
        }
    }

    #[must_use]
    pub fn mount(name: impl Into<String>, container_id: impl Into<String>) -> Self {
        Self {
            kind: VolumeEventKind::Mount,
            name: name.into(),
            driver: String::new(),
            container_id: Some(container_id.into()),
            at: Utc::now(),
        }
    }

    #[must_use]
    pub fn unmount(name: impl Into<String>, container_id: impl Into<String>) -> Self {
        Self {
            kind: VolumeEventKind::Unmount,
            name: name.into(),
            driver: String::new(),
            container_id: Some(container_id.into()),
            at: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// Daemon event union + bus
// ---------------------------------------------------------------------------

/// A daemon lifecycle event. One of container, image, network, or volume.
///
/// Events are tagged on the wire with `"resource"` (`container` | `image` |
/// `network` | `volume`) so consumers can route on resource type without
/// matching the variant shape.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "resource", rename_all = "lowercase")]
pub enum DaemonEvent {
    Container(ContainerEvent),
    Image(ImageEvent),
    Network(NetworkEvent),
    Volume(VolumeEvent),
}

impl DaemonEvent {
    /// Resource family (`"container"`, `"image"`, `"network"`, `"volume"`).
    /// Used by subscribers to filter on Docker's `?filters={"type":...}` form.
    #[must_use]
    pub const fn resource(&self) -> &'static str {
        match self {
            Self::Container(_) => "container",
            Self::Image(_) => "image",
            Self::Network(_) => "network",
            Self::Volume(_) => "volume",
        }
    }

    /// Wire event name (e.g. `container.start`, `image.pull`).
    #[must_use]
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::Container(e) => e.kind.sse_name(),
            Self::Image(e) => e.kind.sse_name(),
            Self::Network(e) => e.kind.sse_name(),
            Self::Volume(e) => e.kind.sse_name(),
        }
    }

    /// Bare action portion of the event name (everything after the resource
    /// dot). For container events this aligns with the Docker `Action` field
    /// (`start`, `die`, ...).
    #[must_use]
    pub fn action(&self) -> &'static str {
        let full = self.event_name();
        full.split_once('.').map_or(full, |(_, rest)| rest)
    }

    /// Wall-clock time the event was emitted.
    #[must_use]
    pub fn at(&self) -> DateTime<Utc> {
        match self {
            Self::Container(e) => e.at,
            Self::Image(e) => e.at,
            Self::Network(e) => e.at,
            Self::Volume(e) => e.at,
        }
    }

    /// Returns true if this event passes the given label filter. Only
    /// container events carry labels today; non-container events ignore the
    /// filter when it is empty and never match a non-empty filter.
    #[must_use]
    pub fn matches_labels(&self, filter: &[(String, String)]) -> bool {
        match self {
            Self::Container(e) => e.matches_labels(filter),
            _ => filter.is_empty(),
        }
    }
}

impl From<ContainerEvent> for DaemonEvent {
    fn from(e: ContainerEvent) -> Self {
        Self::Container(e)
    }
}

impl From<ImageEvent> for DaemonEvent {
    fn from(e: ImageEvent) -> Self {
        Self::Image(e)
    }
}

impl From<NetworkEvent> for DaemonEvent {
    fn from(e: NetworkEvent) -> Self {
        Self::Network(e)
    }
}

impl From<VolumeEvent> for DaemonEvent {
    fn from(e: VolumeEvent) -> Self {
        Self::Volume(e)
    }
}

/// Broadcast bus for daemon lifecycle events.
///
/// Cheap to clone (it wraps an `Arc` internally). Stored on each handler
/// state struct so every handler has access. Backwards-compatible with the
/// historical container-only bus: `publish(ContainerEvent)` still works
/// (auto-wraps into `DaemonEvent::Container`).
#[derive(Clone)]
pub struct DaemonEventBus {
    sender: Arc<broadcast::Sender<DaemonEvent>>,
}

impl DaemonEventBus {
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
    pub fn subscribe(&self) -> broadcast::Receiver<DaemonEvent> {
        self.sender.subscribe()
    }

    /// Publish any event convertible into [`DaemonEvent`] on the bus.
    ///
    /// Fire-and-forget: errors from [`broadcast::Sender::send`] (no active
    /// subscribers) are intentionally ignored. The blanket `Into` bound lets
    /// historical call sites `publish(ContainerEvent::start(...))` continue
    /// to compile while new sites can pass `ImageEvent`, `NetworkEvent`, or
    /// `VolumeEvent` directly.
    pub fn publish<E: Into<DaemonEvent>>(&self, event: E) {
        let _ = self.sender.send(event.into());
    }

    /// Publish an image-pull event.
    pub fn publish_image_pulled(&self, reference: impl Into<String>, digest: Option<String>) {
        self.publish(ImageEvent::pull(reference, digest));
    }

    /// Publish an image-push event.
    pub fn publish_image_pushed(&self, reference: impl Into<String>, digest: Option<String>) {
        self.publish(ImageEvent::push(reference, digest));
    }

    /// Publish an image-delete event.
    pub fn publish_image_deleted(&self, reference: impl Into<String>) {
        self.publish(ImageEvent::delete(reference));
    }

    /// Publish an image-tag event.
    pub fn publish_image_tagged(&self, source: impl Into<String>, target: impl Into<String>) {
        self.publish(ImageEvent::tag(source, target));
    }

    /// Publish a network-create event.
    pub fn publish_network_created(
        &self,
        id: impl Into<String>,
        name: impl Into<String>,
        driver: impl Into<String>,
    ) {
        self.publish(NetworkEvent::create(id, name, driver));
    }

    /// Publish a network-delete event.
    pub fn publish_network_deleted(&self, id: impl Into<String>, name: impl Into<String>) {
        self.publish(NetworkEvent::delete(id, name));
    }

    /// Publish a network-connect event.
    pub fn publish_network_connected(
        &self,
        network_id: impl Into<String>,
        network_name: impl Into<String>,
        container_id: impl Into<String>,
    ) {
        self.publish(NetworkEvent::connect(
            network_id,
            network_name,
            container_id,
        ));
    }

    /// Publish a network-disconnect event.
    pub fn publish_network_disconnected(
        &self,
        network_id: impl Into<String>,
        network_name: impl Into<String>,
        container_id: impl Into<String>,
    ) {
        self.publish(NetworkEvent::disconnect(
            network_id,
            network_name,
            container_id,
        ));
    }

    /// Publish a volume-create event.
    pub fn publish_volume_created(&self, name: impl Into<String>, driver: impl Into<String>) {
        self.publish(VolumeEvent::create(name, driver));
    }

    /// Publish a volume-delete event.
    pub fn publish_volume_deleted(&self, name: impl Into<String>) {
        self.publish(VolumeEvent::delete(name));
    }

    /// Publish a volume-mount event.
    pub fn publish_volume_mounted(&self, name: impl Into<String>, container_id: impl Into<String>) {
        self.publish(VolumeEvent::mount(name, container_id));
    }

    /// Publish a volume-unmount event.
    pub fn publish_volume_unmounted(
        &self,
        name: impl Into<String>,
        container_id: impl Into<String>,
    ) {
        self.publish(VolumeEvent::unmount(name, container_id));
    }

    /// Returns the current number of active subscribers. Useful for tests
    /// and diagnostics.
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for DaemonEventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for DaemonEventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonEventBus")
            .field("subscriber_count", &self.subscriber_count())
            .finish()
    }
}

/// Backwards-compatible alias for the historical name. New code should
/// prefer `DaemonEventBus`.
pub type ContainerEventBus = DaemonEventBus;

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
        let bus = DaemonEventBus::new();
        let mut rx = bus.subscribe();

        let ev = ContainerEvent::start("c1", labels(&[("app", "web")]));
        bus.publish(ev);

        let received = rx.recv().await.expect("event delivered");
        match received {
            DaemonEvent::Container(c) => {
                assert_eq!(c.id, "c1");
                assert_eq!(c.kind, ContainerEventKind::Start);
            }
            other => panic!("expected container event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn publish_without_subscribers_is_noop() {
        let bus = DaemonEventBus::new();
        // No subscribers: publish must not panic.
        bus.publish(ContainerEvent::start("c1", HashMap::new()));
    }

    #[test]
    fn sse_event_names() {
        assert_eq!(ContainerEventKind::Start.sse_name(), "container.start");
        assert_eq!(ContainerEventKind::Die.sse_name(), "container.die");
        assert_eq!(ContainerEventKind::Oom.sse_name(), "container.oom");
        assert_eq!(ContainerEventKind::Health.sse_name(), "container.health");
        assert_eq!(ImageEventKind::Pull.sse_name(), "image.pull");
        assert_eq!(ImageEventKind::Push.sse_name(), "image.push");
        assert_eq!(ImageEventKind::Delete.sse_name(), "image.delete");
        assert_eq!(ImageEventKind::Tag.sse_name(), "image.tag");
        assert_eq!(NetworkEventKind::Create.sse_name(), "network.create");
        assert_eq!(NetworkEventKind::Delete.sse_name(), "network.destroy");
        assert_eq!(NetworkEventKind::Connect.sse_name(), "network.connect");
        assert_eq!(
            NetworkEventKind::Disconnect.sse_name(),
            "network.disconnect"
        );
        assert_eq!(VolumeEventKind::Create.sse_name(), "volume.create");
        assert_eq!(VolumeEventKind::Delete.sse_name(), "volume.destroy");
    }

    #[tokio::test]
    async fn publish_image_event() {
        let bus = DaemonEventBus::new();
        let mut rx = bus.subscribe();

        bus.publish_image_pulled("nginx:latest", Some("sha256:deadbeef".to_string()));

        let received = rx.recv().await.expect("image event delivered");
        match received {
            DaemonEvent::Image(img) => {
                assert_eq!(img.kind, ImageEventKind::Pull);
                assert_eq!(img.reference, "nginx:latest");
                assert_eq!(img.digest.as_deref(), Some("sha256:deadbeef"));
            }
            other => panic!("expected image event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn publish_network_event() {
        let bus = DaemonEventBus::new();
        let mut rx = bus.subscribe();

        bus.publish_network_created("net123", "my-net", "bridge");

        let received = rx.recv().await.expect("network event delivered");
        match received {
            DaemonEvent::Network(n) => {
                assert_eq!(n.kind, NetworkEventKind::Create);
                assert_eq!(n.id, "net123");
                assert_eq!(n.name, "my-net");
                assert_eq!(n.driver, "bridge");
            }
            other => panic!("expected network event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn publish_volume_event() {
        let bus = DaemonEventBus::new();
        let mut rx = bus.subscribe();

        bus.publish_volume_created("pg-data", "local");

        let received = rx.recv().await.expect("volume event delivered");
        match received {
            DaemonEvent::Volume(v) => {
                assert_eq!(v.kind, VolumeEventKind::Create);
                assert_eq!(v.name, "pg-data");
                assert_eq!(v.driver, "local");
            }
            other => panic!("expected volume event, got {other:?}"),
        }
    }

    #[test]
    fn daemon_event_resource_and_action() {
        let c = DaemonEvent::Container(ContainerEvent::start("c1", HashMap::new()));
        assert_eq!(c.resource(), "container");
        assert_eq!(c.action(), "start");

        let i = DaemonEvent::Image(ImageEvent::pull("nginx:latest", None));
        assert_eq!(i.resource(), "image");
        assert_eq!(i.action(), "pull");

        let n = DaemonEvent::Network(NetworkEvent::create("id", "name", "bridge"));
        assert_eq!(n.resource(), "network");
        assert_eq!(n.action(), "create");

        let v = DaemonEvent::Volume(VolumeEvent::create("vol", "local"));
        assert_eq!(v.resource(), "volume");
        assert_eq!(v.action(), "create");
    }

    #[test]
    fn daemon_event_label_filter_only_applies_to_containers() {
        let c = DaemonEvent::Container(ContainerEvent::start("c1", labels(&[("app", "web")])));
        assert!(c.matches_labels(&[("app".to_string(), "web".to_string())]));
        assert!(!c.matches_labels(&[("app".to_string(), "worker".to_string())]));

        // Image events: empty filter accepts, any non-empty filter rejects.
        let i = DaemonEvent::Image(ImageEvent::pull("nginx:latest", None));
        assert!(i.matches_labels(&[]));
        assert!(!i.matches_labels(&[("app".to_string(), "web".to_string())]));
    }
}
