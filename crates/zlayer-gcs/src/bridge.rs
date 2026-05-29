//! High-level GCS bridge.
//!
//! Connects to a UVM's in-guest GCS over hvsock, negotiates the protocol,
//! and dispatches typed RPCs. A background reader task delivers responses
//! to waiting callers via message-id-correlated oneshot channels.
//!
//! The bridge state machine pairs requests with responses and demuxes
//! asynchronous notifications from the in-guest GCS. It owns the hvsock
//! transport, runs a background task that decodes frames and routes them
//! to the right oneshot waiter, and exposes a cheap-to-clone `GcsBridge`
//! handle that callers use to issue RPCs.

#![cfg(target_os = "windows")]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{oneshot, Mutex};

use crate::error::{GcsError, GcsResult};
use crate::frame::{self, RpcMessageType, HEADER_LEN, RESPONSE_TYPE_OFFSET};
use crate::protocol::{NegotiateProtocolRequest, NegotiateProtocolResponse, RequestBase};
use crate::transport::{HvSockListener, HvSockStream, GCS_SERVICE_GUID};

/// Default min/max GCS protocol version we negotiate. v4 is the version
/// hcsshim's `internal/gcs/guestconnection.go` declares as of May 2026.
pub const GCS_PROTOCOL_VERSION: u32 = 4;

/// Map of message-id → oneshot sender awaiting the matching response frame.
/// Entries are inserted by [`GcsBridge::send_rpc_json`] just before the
/// request frame is written, and removed by the background reader when the
/// response arrives (or by the sender path on write failure).
type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<(u32, Vec<u8>)>>>>;

/// Host-side GCS bridge to a single UVM.
///
/// Cheap to clone — all internal state is `Arc`-shared so multiple tasks may
/// issue RPCs concurrently. The background reader task is spawned exactly
/// once in [`PendingGcsBridge::accept`] and lives until the underlying stream
/// is closed (peer hangup, transport error, or process exit).
#[derive(Clone, Debug)]
pub struct GcsBridge {
    /// Shared, cloneable hvsock stream — both this handle and the background
    /// reader task hold a clone.
    stream: HvSockStream,
    /// Monotonically increasing message-id allocator. Starts at 1 so we can
    /// reserve 0 as "no message id" should we ever need it.
    next_id: Arc<AtomicU64>,
    /// In-flight RPC waiters, keyed by message id.
    pending: PendingMap,
}

impl GcsBridge {
    /// Bind the host hvsock listener for the UVM identified by `vm_id` (its
    /// runtime GUID). Must be called BEFORE `HcsStartComputeSystem` so the
    /// host is listening when the in-guest GCS boots and dials out. Returns a
    /// [`PendingGcsBridge`]; call [`PendingGcsBridge::accept`] after start.
    pub async fn listen(vm_id: windows::core::GUID) -> GcsResult<PendingGcsBridge> {
        let listener = HvSockListener::bind(vm_id, GCS_SERVICE_GUID).await?;
        Ok(PendingGcsBridge { listener })
    }

    /// Negotiate the GCS protocol version (returns the chosen version).
    ///
    /// Fails with [`GcsError::Negotiation`] if the guest returns a non-zero
    /// HRESULT or chooses a version outside the host's supported range.
    pub async fn negotiate_protocol(&self) -> GcsResult<u32> {
        let req = NegotiateProtocolRequest {
            base: RequestBase {
                activity_id: uuid::Uuid::new_v4(),
                container_id: String::new(),
            },
            minimum_version: GCS_PROTOCOL_VERSION,
            maximum_version: GCS_PROTOCOL_VERSION,
        };
        let resp: NegotiateProtocolResponse = self
            .send_rpc_json(RpcMessageType::NegotiateProtocol, &req)
            .await?;
        if resp.base.result != 0 {
            // HRESULT is conventionally rendered as an unsigned 32-bit hex
            // value (top bit = severity). Reinterpret the i32 bit pattern
            // rather than going through `as u32`, which clippy flags.
            let hresult = u32::from_ne_bytes(resp.base.result.to_ne_bytes());
            return Err(GcsError::Negotiation(format!(
                "guest returned HRESULT {hresult:#x}: {}",
                resp.base.error_message
            )));
        }
        if resp.version != GCS_PROTOCOL_VERSION {
            return Err(GcsError::Negotiation(format!(
                "guest chose version {} (host wanted {GCS_PROTOCOL_VERSION})",
                resp.version
            )));
        }
        Ok(resp.version)
    }

    /// Generic JSON-over-frame RPC dispatch with message-id correlation.
    ///
    /// Serializes `req`, frames it with a freshly-allocated message id,
    /// installs a oneshot waiter in `pending`, writes the frame, and awaits
    /// the matching response from the background reader. On any send failure
    /// the waiter is removed eagerly so we don't leak entries in `pending`.
    pub async fn send_rpc_json<Req, Resp>(&self, rpc: RpcMessageType, req: &Req) -> GcsResult<Resp>
    where
        // `Sync` is required so the captured `&Req` argument is `Send` —
        // the returned future is held across `.await` points and tokio
        // requires it to cross thread boundaries on a multi-threaded runtime.
        Req: serde::Serialize + Sync,
        Resp: serde::de::DeserializeOwned,
    {
        let message_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let payload = serde_json::to_vec(req)?;
        let mut frame = Vec::with_capacity(HEADER_LEN + payload.len());
        frame::encode_frame(rpc.as_request_type(), message_id, &payload, &mut frame);

        let (tx, rx) = oneshot::channel();
        {
            // Scoped lock — do not hold across the stream.write_all().await
            // below. This keeps the pending map available to the background
            // reader task while the write is in flight.
            let mut guard = self.pending.lock().await;
            guard.insert(message_id, tx);
        }

        if let Err(e) = self.stream.write_all(&frame).await {
            // Eagerly drop the waiter so the entry doesn't linger and so the
            // caller sees the underlying write error instead of Closed.
            self.pending.lock().await.remove(&message_id);
            return Err(e);
        }

        let (resp_type, resp_payload) = rx.await.map_err(|_| GcsError::Closed)?;
        let expected = rpc.as_response_type();
        if resp_type != expected {
            return Err(GcsError::Protocol(format!(
                "unexpected response type {resp_type:#x} (expected {expected:#x}) for message {message_id}"
            )));
        }
        let resp: Resp = serde_json::from_slice(&resp_payload)?;
        Ok(resp)
    }

    /// Spawn the background reader task. Reads frames forever; on any error
    /// the task exits and all pending requests are dropped (their oneshot
    /// senders are dropped → `recv()` returns `Err` → callers see
    /// [`GcsError::Closed`]).
    fn spawn_reader(&self) {
        let stream = self.stream.clone();
        let pending = Arc::clone(&self.pending);
        tokio::spawn(async move {
            loop {
                let mut hdr_buf = [0u8; HEADER_LEN];
                if stream.read_exact(&mut hdr_buf).await.is_err() {
                    break;
                }
                let Ok(hdr) = frame::decode_header(&hdr_buf) else {
                    break;
                };
                let body_len = (hdr.size as usize) - HEADER_LEN;
                let mut body = vec![0u8; body_len];
                if body_len > 0 && stream.read_exact(&mut body).await.is_err() {
                    break;
                }
                // Stream events (CATEGORY_STREAM, asynchronous notifications)
                // have no waiting request — currently we drop them. A future
                // task can route them to a notification channel on the bridge.
                if hdr.r#type & RESPONSE_TYPE_OFFSET == 0 {
                    continue;
                }
                let waiter = {
                    let mut guard = pending.lock().await;
                    guard.remove(&hdr.message_id)
                };
                if let Some(tx) = waiter {
                    // Receiver may have been dropped (caller cancelled);
                    // ignore the send error in that case.
                    let _ = tx.send((hdr.r#type, body));
                }
            }
            // On reader exit, drop every pending sender to wake stuck RPCs
            // with GcsError::Closed.
            pending.lock().await.clear();
        });
    }
}

/// A bound-but-not-yet-accepted GCS listener.
///
/// Created by
/// [`GcsBridge::listen`] BEFORE the UVM is started; the host must be
/// listening when the in-guest GCS boots and dials out. Call
/// [`PendingGcsBridge::accept`] AFTER `HcsStartComputeSystem` to accept the
/// guest's inbound connection and finish bringing up the bridge.
pub struct PendingGcsBridge {
    listener: HvSockListener,
}

impl PendingGcsBridge {
    /// Accept the in-guest GCS's outbound connection (it dials the host after
    /// boot), then spawn the reader task and negotiate the protocol version.
    ///
    /// `timeout` bounds how long we wait for the guest GCS to come up and
    /// connect; hcsshim uses a multi-minute `GCSConnectionTimeout`.
    pub async fn accept(self, timeout: Duration) -> GcsResult<GcsBridge> {
        let stream = tokio::time::timeout(timeout, self.listener.accept())
            .await
            .map_err(|_| {
                GcsError::Hvsock(format!(
                    "timed out after {timeout:?} waiting for in-guest GCS to connect"
                ))
            })??;
        let bridge = GcsBridge {
            stream,
            next_id: Arc::new(AtomicU64::new(1)),
            pending: Arc::new(Mutex::new(HashMap::new())),
        };
        bridge.spawn_reader();
        bridge.negotiate_protocol().await?;
        Ok(bridge)
    }
}

#[cfg(test)]
mod tests {
    use super::GcsBridge;

    /// Compile-time assertion that `GcsBridge` is safe to share across
    /// threads — RPC dispatch relies on cloning the bridge into spawned
    /// tasks.
    #[test]
    fn bridge_is_clone_send_sync() {
        const fn assert_clone_send_sync<T: Clone + Send + Sync>() {}
        assert_clone_send_sync::<GcsBridge>();
    }
}
