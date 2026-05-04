//! Daemon-wide event stream.
//!
//! Exposes `GET /api/v1/events` as a Newline-Delimited-JSON (NDJSON) stream
//! of daemon lifecycle events: container start/die/oom/health, image
//! pull/push/delete/tag, network create/delete/connect/disconnect, and
//! volume create/delete/mount/unmount. Clients subscribe to the broadcast
//! bus carried on [`ContainerApiState::event_bus`] and receive events as
//! they are published by lifecycle handlers.
//!
//! NDJSON is the format Docker's `/events` API uses: one JSON object per
//! line, `Content-Type: application/json`, no `data:` prefix and no
//! `event:` field. The Docker-compat layer in `zlayer-docker` proxies this
//! stream verbatim onto its own NDJSON `/events` endpoint.
//!
//! # Query parameters
//!
//! - `follow` (bool, default `true`): reserved for parity with the Docker
//!   events API. Today the endpoint is always streaming; when `follow=false`
//!   the handler emits an empty body and closes immediately.
//! - `label` (repeated, `k=v`): AND-filter against container labels. Only
//!   container events whose `labels` map contains every `(k, v)` pair are
//!   delivered; non-container events bypass the filter when the filter is
//!   empty and are excluded when it is non-empty.
//!
//! # Backpressure
//!
//! If a subscriber lags behind [`crate::event_bus::EVENT_BUS_CAPACITY`]
//! events, the broadcast channel reports [`BroadcastStreamRecvError::Lagged`].
//! The handler emits a terminal `{"close": ..., "dropped": N}` line and
//! ends the stream -- the client is expected to reconnect.

use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio_stream::wrappers::{errors::BroadcastStreamRecvError, BroadcastStream};
use tracing::{debug, warn};
pub use zlayer_types::api::events::*;

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};
use crate::event_bus::DaemonEvent;
use crate::handlers::containers::ContainerApiState;

/// Parse `label=k=v` query strings into a vector of `(key, value)` pairs.
///
/// Rejects empty keys. A malformed entry (missing `=` or empty key) is
/// returned as a [`ApiError::BadRequest`].
fn parse_label_filters(raw: &[String]) -> Result<Vec<(String, String)>> {
    let mut out = Vec::with_capacity(raw.len());
    for entry in raw {
        let (k, v) = entry.split_once('=').ok_or_else(|| {
            ApiError::BadRequest(format!(
                "Invalid label filter '{entry}': expected 'key=value'"
            ))
        })?;
        if k.is_empty() {
            return Err(ApiError::BadRequest(format!(
                "Invalid label filter '{entry}': key must not be empty"
            )));
        }
        out.push((k.to_string(), v.to_string()));
    }
    Ok(out)
}

/// Encode a [`DaemonEvent`] as a single NDJSON line: JSON body + trailing
/// `\n`. Returns `None` if serialization fails (which never happens in
/// practice for the well-typed event variants).
fn ndjson_line(ev: &DaemonEvent) -> Option<Bytes> {
    let mut bytes = serde_json::to_vec(ev).ok()?;
    bytes.push(b'\n');
    Some(Bytes::from(bytes))
}

/// One of the two stream outputs this handler produces: a normal NDJSON
/// line, or a terminal close marker that causes the stream to end.
enum StreamItem {
    Line(Bytes),
    Close(Bytes),
}

/// Stream adapter that yields lines until a `Close` item is produced, then
/// ends. Used to drop laggy subscribers after a single terminal close line --
/// `BroadcastStream` otherwise stays open indefinitely after a `Lagged`
/// error and the client would silently miss events.
struct CloseOnTerminal<S> {
    inner: S,
    terminated: bool,
}

impl<S> CloseOnTerminal<S> {
    fn new(inner: S) -> Self {
        Self {
            inner,
            terminated: false,
        }
    }
}

impl<S> Stream for CloseOnTerminal<S>
where
    S: Stream<Item = StreamItem> + Unpin,
{
    type Item = std::result::Result<Bytes, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.terminated {
            return Poll::Ready(None);
        }
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(StreamItem::Line(b))) => Poll::Ready(Some(Ok(b))),
            Poll::Ready(Some(StreamItem::Close(b))) => {
                self.terminated = true;
                Poll::Ready(Some(Ok(b)))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Build the NDJSON response headers. `Content-Type: application/json` (NOT
/// `text/event-stream`); intermediaries are told not to cache or buffer.
fn ndjson_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-store, must-revalidate"),
    );
    // Hint to nginx-style proxies not to buffer the streaming body.
    headers.insert("X-Accel-Buffering", HeaderValue::from_static("no"));
    headers
}

/// Stream daemon lifecycle events as NDJSON.
///
/// Each event is one JSON object on its own line, terminated with `\n`. The
/// response advertises `Content-Type: application/json` (matching Docker's
/// `/events` endpoint contract).
///
/// # Errors
///
/// Returns `400` if a `label` query parameter is malformed, `401` if the
/// caller is not authenticated.
#[utoipa::path(
    get,
    path = "/api/v1/events",
    params(EventsQuery),
    responses(
        (status = 200, description = "NDJSON stream of daemon lifecycle events"),
        (status = 400, description = "Invalid label filter"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Events"
)]
pub async fn stream_events(
    _user: AuthUser,
    State(state): State<ContainerApiState>,
    Query(query): Query<EventsQuery>,
) -> Result<Response> {
    let filters = parse_label_filters(&query.label)?;
    let follow = query.follow.unwrap_or(true);

    debug!(
        filters = ?filters,
        follow,
        "Subscribing to daemon event stream (NDJSON)"
    );

    let rx = state.event_bus.subscribe();

    let body = if follow {
        let inner = BroadcastStream::new(rx).filter_map(move |res| {
            let filters = filters.clone();
            async move {
                match res {
                    Ok(ev) => {
                        if !ev.matches_labels(&filters) {
                            return None;
                        }
                        ndjson_line(&ev).map(StreamItem::Line)
                    }
                    Err(BroadcastStreamRecvError::Lagged(n)) => {
                        warn!(
                            dropped = n,
                            "Daemon event subscriber lagged; closing stream"
                        );
                        // Emit a single terminal close marker so the client
                        // can distinguish a deliberate termination from a
                        // dropped TCP connection; CloseOnTerminal then ends
                        // the stream.
                        let payload = serde_json::json!({
                            "close": "lagged",
                            "dropped": n,
                        });
                        let mut buf =
                            serde_json::to_vec(&payload).unwrap_or_else(|_| b"{}".to_vec());
                        buf.push(b'\n');
                        Some(StreamItem::Close(Bytes::from(buf)))
                    }
                }
            }
        });
        let stream = CloseOnTerminal::new(Box::pin(inner));
        Body::from_stream(stream)
    } else {
        Body::empty()
    };

    let mut response = Response::builder().status(StatusCode::OK);
    if let Some(headers) = response.headers_mut() {
        headers.extend(ndjson_headers());
    }
    response
        .body(body)
        .map_err(|e| ApiError::Internal(format!("failed to build events response: {e}")))
        .map(IntoResponse::into_response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_bus::{ContainerEvent, DaemonEventBus, ImageEvent, ImageEventKind};
    use std::collections::HashMap;
    use std::time::Duration;

    fn labels(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn parse_label_filters_accepts_valid() {
        let input = vec![
            "app=web".to_string(),
            "env=prod".to_string(),
            "tier=api".to_string(),
        ];
        let parsed = parse_label_filters(&input).expect("valid filters");
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0], ("app".to_string(), "web".to_string()));
        assert_eq!(parsed[2], ("tier".to_string(), "api".to_string()));
    }

    #[test]
    fn parse_label_filters_accepts_empty_value() {
        let input = vec!["app=".to_string()];
        let parsed = parse_label_filters(&input).expect("empty value is fine");
        assert_eq!(parsed, vec![("app".to_string(), String::new())]);
    }

    #[test]
    fn parse_label_filters_rejects_missing_equals() {
        let input = vec!["malformed".to_string()];
        let err = parse_label_filters(&input).unwrap_err();
        match err {
            ApiError::BadRequest(msg) => assert!(msg.contains("malformed")),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn parse_label_filters_rejects_empty_key() {
        let input = vec!["=value".to_string()];
        let err = parse_label_filters(&input).unwrap_err();
        match err {
            ApiError::BadRequest(msg) => assert!(msg.contains("key must not be empty")),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn label_filter_matches_and_rejects() {
        let bus = DaemonEventBus::new();
        let mut rx = bus.subscribe();

        bus.publish(ContainerEvent::start(
            "c1",
            labels(&[("app", "web"), ("env", "prod")]),
        ));
        bus.publish(ContainerEvent::start(
            "c2",
            labels(&[("app", "worker"), ("env", "prod")]),
        ));

        let ev1 = rx.recv().await.expect("event 1");
        let ev2 = rx.recv().await.expect("event 2");

        let filter = vec![("app".to_string(), "web".to_string())];
        assert!(ev1.matches_labels(&filter));
        assert!(!ev2.matches_labels(&filter));
    }

    #[test]
    fn ndjson_line_is_one_object_terminated_by_newline() {
        let ev = DaemonEvent::Container(ContainerEvent::start("c1", labels(&[("app", "web")])));
        let line = ndjson_line(&ev).expect("encoded line");
        let s = std::str::from_utf8(&line).expect("utf8");

        // Exactly one trailing newline; nothing else after it.
        assert!(s.ends_with('\n'));
        assert_eq!(s.matches('\n').count(), 1);
        // Body itself is parseable JSON.
        let trimmed = s.trim_end_matches('\n');
        let parsed: serde_json::Value = serde_json::from_str(trimmed).expect("valid json");
        assert_eq!(parsed["resource"], "container");
        // No SSE-specific framing fields.
        assert!(!s.contains("data:"));
        assert!(!s.contains("event:"));
        assert!(!s.contains("\n\n"));
    }

    #[test]
    fn ndjson_line_for_image_event() {
        let ev = DaemonEvent::Image(ImageEvent::pull("nginx:latest", None));
        let line = ndjson_line(&ev).expect("encoded line");
        let s = std::str::from_utf8(&line).expect("utf8");
        assert!(s.ends_with('\n'));
        let parsed: serde_json::Value =
            serde_json::from_str(s.trim_end_matches('\n')).expect("valid json");
        assert_eq!(parsed["resource"], "image");
        assert_eq!(parsed["reference"], "nginx:latest");
    }

    #[test]
    fn ndjson_headers_advertise_application_json() {
        let h = ndjson_headers();
        assert_eq!(h.get(header::CONTENT_TYPE).unwrap(), "application/json");
        // Must NOT be SSE.
        assert_ne!(h.get(header::CONTENT_TYPE).unwrap(), "text/event-stream");
    }

    #[tokio::test]
    async fn stream_delivers_filtered_events_only() {
        // Smoke-test: build the underlying filter stream the handler uses,
        // publish events, and confirm only matching ones arrive.
        let bus = DaemonEventBus::new();
        let rx = bus.subscribe();

        let filters = vec![("app".to_string(), "web".to_string())];
        let stream = BroadcastStream::new(rx).filter_map(move |res| {
            let filters = filters.clone();
            async move {
                match res {
                    Ok(ev) if ev.matches_labels(&filters) => ndjson_line(&ev),
                    _ => None,
                }
            }
        });
        let mut stream = Box::pin(stream);

        bus.publish(ContainerEvent::start("c1", labels(&[("app", "web")])));
        bus.publish(ContainerEvent::start("c2", labels(&[("app", "worker")])));
        bus.publish(ContainerEvent::start("c3", labels(&[("app", "web")])));

        let first = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("first event before timeout")
            .expect("non-empty stream");
        let parsed: serde_json::Value =
            serde_json::from_slice(&first[..first.len() - 1]).expect("json");
        assert_eq!(parsed["id"], "c1");

        let second = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("second event before timeout")
            .expect("non-empty stream");
        let parsed: serde_json::Value =
            serde_json::from_slice(&second[..second.len() - 1]).expect("json");
        assert_eq!(parsed["id"], "c3");
    }

    #[test]
    fn image_event_kind_wire_names() {
        // Sanity check that `image.pull` etc. are the wire names the
        // Docker-compat translator expects.
        assert_eq!(ImageEventKind::Pull.sse_name(), "image.pull");
    }
}
