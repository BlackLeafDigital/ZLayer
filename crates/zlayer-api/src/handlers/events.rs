//! Daemon-wide container event stream.
//!
//! Exposes `GET /api/v1/events` as a Server-Sent Events stream of container
//! lifecycle events (start, die, oom, health). Clients subscribe to the
//! broadcast bus carried on [`ContainerApiState::event_bus`] and receive
//! events as they are published by lifecycle handlers.
//!
//! # Query parameters
//!
//! - `follow` (bool, default `true`): reserved for parity with the Docker
//!   events API. Today the endpoint is always streaming; when `follow=false`
//!   the handler emits a single keep-alive and closes immediately.
//! - `label` (repeated, `k=v`): AND-filter. Only events whose `labels` map
//!   contains every `(k, v)` pair are delivered.
//!
//! # Backpressure
//!
//! If a subscriber lags behind [`crate::event_bus::EVENT_BUS_CAPACITY`]
//! events, the broadcast channel reports [`BroadcastStreamRecvError::Lagged`].
//! The handler emits a terminal `close` event naming the lag and ends the
//! stream -- the client is expected to reconnect.

use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::{
    extract::{Query, State},
    response::sse::{Event, KeepAlive, Sse},
};
use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use tokio_stream::wrappers::{errors::BroadcastStreamRecvError, BroadcastStream};
use tracing::{debug, warn};
use utoipa::IntoParams;

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};
use crate::event_bus::ContainerEvent;
use crate::handlers::containers::ContainerApiState;

/// Query parameters for `GET /api/v1/events`.
#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct EventsQuery {
    /// Follow the event stream. Default: `true`. Reserved for parity with
    /// Docker-compat tooling; this endpoint is always streaming.
    #[serde(default)]
    pub follow: Option<bool>,
    /// Label filter in `k=v` form. Repeatable. An event passes only if all
    /// filters match (AND semantics).
    #[serde(default)]
    pub label: Vec<String>,
}

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

/// Serialize a [`ContainerEvent`] into an SSE [`Event`]. Uses the event's
/// kind as the SSE `event:` name (e.g. `container.start`) and the full event
/// body as JSON data.
fn to_sse_event(ev: &ContainerEvent) -> Event {
    Event::default()
        .event(ev.kind.sse_name())
        .json_data(ev)
        .unwrap_or_else(|err| {
            // Extremely unlikely -- ContainerEvent is trivially serializable.
            warn!(error = %err, "Failed to serialize ContainerEvent");
            Event::default().event("error").data("serialization_error")
        })
}

/// One of the two stream outputs this handler produces: a normal SSE event,
/// or a terminal `close` marker that causes the stream to end.
enum StreamItem {
    Event(Event),
    Close(Event),
}

/// Stream adapter that yields events until a `Close` item is produced, then
/// ends. Used to drop laggy subscribers after a single terminal `close` SSE
/// event -- `BroadcastStream` otherwise stays open indefinitely after a
/// `Lagged` error and the client would silently miss events.
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
    type Item = std::result::Result<Event, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.terminated {
            return Poll::Ready(None);
        }
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(StreamItem::Event(ev))) => Poll::Ready(Some(Ok(ev))),
            Poll::Ready(Some(StreamItem::Close(ev))) => {
                self.terminated = true;
                Poll::Ready(Some(Ok(ev)))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Stream container lifecycle events as Server-Sent Events.
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
        (status = 200, description = "SSE stream of container lifecycle events"),
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
) -> Result<Sse<impl Stream<Item = std::result::Result<Event, Infallible>> + Send>> {
    let filters = parse_label_filters(&query.label)?;
    let follow = query.follow.unwrap_or(true);

    debug!(
        filters = ?filters,
        follow,
        "Subscribing to container event stream"
    );

    let rx = state.event_bus.subscribe();

    // Build a single unified stream that either:
    //  - emits filtered container events and a terminal `close` on lag
    //    (follow=true), or
    //  - immediately ends (follow=false).
    // Using one return shape keeps the `impl Stream` signature happy.
    let boxed: Pin<Box<dyn Stream<Item = std::result::Result<Event, Infallible>> + Send>> =
        if follow {
            let inner = BroadcastStream::new(rx).filter_map(move |res| {
                let filters = filters.clone();
                async move {
                    match res {
                        Ok(ev) => {
                            if ev.matches_labels(&filters) {
                                Some(StreamItem::Event(to_sse_event(&ev)))
                            } else {
                                None
                            }
                        }
                        Err(BroadcastStreamRecvError::Lagged(n)) => {
                            warn!(
                                dropped = n,
                                "Container event subscriber lagged; closing stream"
                            );
                            // Emit a single terminal `close` event so the
                            // client can distinguish a deliberate termination
                            // from a broken connection; CloseOnTerminal then
                            // ends the stream.
                            let payload = serde_json::json!({
                                "reason": "lagged",
                                "dropped": n,
                            });
                            let close = Event::default()
                                .event("close")
                                .json_data(&payload)
                                .unwrap_or_else(|_| Event::default().event("close").data("lagged"));
                            Some(StreamItem::Close(close))
                        }
                    }
                }
            });
            // Box-pin the filter_map (which is !Unpin due to the inner async
            // block), then wrap with CloseOnTerminal which requires its
            // inner stream to be Unpin (Pin<Box<_>> always is).
            Box::pin(CloseOnTerminal::new(Box::pin(inner)))
        } else {
            Box::pin(futures_util::stream::empty())
        };

    Ok(Sse::new(boxed).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_bus::{ContainerEvent, ContainerEventBus};
    use std::collections::HashMap;

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
        // k= is a valid filter -- used to match labels whose value is literally
        // the empty string. Some Docker tooling uses this.
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
        let bus = ContainerEventBus::new();
        let mut rx = bus.subscribe();

        let web = ContainerEvent::start("c1", labels(&[("app", "web"), ("env", "prod")]));
        let worker = ContainerEvent::start("c2", labels(&[("app", "worker"), ("env", "prod")]));
        bus.publish(web.clone());
        bus.publish(worker.clone());

        // Both published; the AND filter on ("app", "web") should only match
        // `web`.
        let ev1 = rx.recv().await.expect("event 1");
        let ev2 = rx.recv().await.expect("event 2");

        let filter = vec![("app".to_string(), "web".to_string())];
        assert!(ev1.matches_labels(&filter));
        assert!(!ev2.matches_labels(&filter));
    }

    #[tokio::test]
    async fn sse_stream_delivers_filtered_events() {
        // Smoke-test: build the underlying filter_map stream the handler uses,
        // publish events, and confirm only matching ones arrive.
        let bus = ContainerEventBus::new();
        let rx = bus.subscribe();

        let filters = vec![("app".to_string(), "web".to_string())];
        let stream = BroadcastStream::new(rx).filter_map(move |res| {
            let filters = filters.clone();
            async move {
                match res {
                    Ok(ev) if ev.matches_labels(&filters) => Some(Ok::<_, Infallible>(ev)),
                    _ => None,
                }
            }
        });
        let mut stream = Box::pin(stream);

        bus.publish(ContainerEvent::start("c1", labels(&[("app", "web")])));
        bus.publish(ContainerEvent::start("c2", labels(&[("app", "worker")])));
        bus.publish(ContainerEvent::start("c3", labels(&[("app", "web")])));

        // Bounded wait: we expect exactly two matches.
        let first = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("first event before timeout")
            .expect("non-empty stream")
            .expect("no error");
        assert_eq!(first.id, "c1");

        let second = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("second event before timeout")
            .expect("non-empty stream")
            .expect("no error");
        assert_eq!(second.id, "c3");
    }
}
