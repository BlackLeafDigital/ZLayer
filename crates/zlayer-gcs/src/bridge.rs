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

use crate::diagnostics::ts_us;

use tokio::sync::{oneshot, Mutex};

use crate::error::{GcsError, GcsResult};
use crate::frame::{self, RpcMessageType, HEADER_LEN, MSG_TYPE_MASK, MSG_TYPE_RESPONSE};
use crate::protocol::{
    NegotiateProtocolRequest, NegotiateProtocolResponse, ProtocolSupport, RequestBase, ResponseBase,
};
use crate::transport::{HvSockListener, HvSockStream, GCS_SERVICE_GUID};

/// Null-GUID container ID sent on bridge-level RPCs that target the UVM
/// itself rather than a hosted container. Mirrors hcsshim's
/// `internal/gcs/guestconnection.go::nullContainerID`.
const NULL_CONTAINER_ID: &str = "00000000-0000-0000-0000-000000000000";

/// Default min/max GCS protocol version we negotiate. v4 is the version
/// hcsshim's `internal/gcs/guestconnection.go` declares as of May 2026.
pub const GCS_PROTOCOL_VERSION: u32 = 4;

/// Reader/writer rendezvous state, guarded by a single mutex so that "reader
/// has exited" and "new waiter inserted" are mutually exclusive.
///
/// Without the `closed` flag, an RPC whose frame is written into a half-open
/// socket *after* the background reader already drained `waiters` would block
/// forever on its oneshot — there is no reader left to deliver a response or
/// drop the sender. We observed exactly that: once the in-guest GCS closed the
/// bridge immediately after `NegotiateProtocol`, the cold-start `Create` hung
/// the whole test until the 1200s outer budget tripped.
#[derive(Debug)]
struct PendingState {
    /// Set once the background reader exits (peer hangup / transport error).
    /// Any [`GcsBridge::send_rpc_json`] that observes this returns
    /// [`GcsError::Closed`] instead of installing a waiter that would never be
    /// woken.
    closed: bool,
    /// In-flight message-id → oneshot sender awaiting the matching response
    /// frame. Entries are inserted by [`GcsBridge::send_rpc_json`] just before
    /// the request frame is written, and removed by the background reader when
    /// the response arrives (or by the sender path on write failure).
    waiters: HashMap<u64, oneshot::Sender<(u32, Vec<u8>)>>,
}

/// Shared, cloneable handle to the [`PendingState`].
type PendingMap = Arc<Mutex<PendingState>>;

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

    /// Negotiate the GCS protocol version and return the guest's declared
    /// capabilities so the caller can drive the cold-start follow-up
    /// (`Create`/`Start` against the null container id) per hcsshim's
    /// `internal/gcs/guestconnection.go::connect`.
    ///
    /// Fails with [`GcsError::Negotiation`] if the guest returns a non-zero
    /// HRESULT or chooses a version outside the host's supported range.
    pub async fn negotiate_protocol(&self) -> GcsResult<ProtocolSupport> {
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
        Ok(resp.capabilities)
    }

    /// Drive the cold-start RPC sequence the in-guest GCS expects
    /// IMMEDIATELY after a successful `NegotiateProtocol`: an `RPCCreate`
    /// against the null container id carrying `UvmConfig{SystemType:"Container"}`,
    /// then (conditionally) `RPCStart`. Mirrors hcsshim's
    /// `internal/gcs/guestconnection.go::connect` (lines 144-167) — if the
    /// host skips these or sends an unrelated RPC first, the GCS closes the
    /// bridge (we observed this verbatim:
    /// `gcs-bridge-reader: header read failed after 1 frame(s): bridge closed`).
    async fn cold_start_create_start(&self, caps: &ProtocolSupport) -> GcsResult<()> {
        if !caps.send_host_create_message {
            return Ok(());
        }
        // `ContainerConfig` is hcsshim's `AnyInString` — a JSON value
        // serialised into a string field. The inner UvmConfig carries
        // `SystemType` plus a `TimeZoneInformation` block: hcsshim's
        // `internal/uvm/start.go::Start` ALWAYS sets a Timezone for Windows
        // UVMs (the host's tz, or UTC when `noInheritHostTimezone` is set),
        // so the in-guest gcs.exe is reached only via that code path in
        // production. Omitting the field altogether (our prior `{SystemType}`-
        // only body) causes the guest GCS to critical-process-die ~0.87ms
        // after receiving Create — the bridge sees `header read failed
        // after 1 frame(s): bridge closed` and Hyper-V-Worker eventid 18590
        // fires with bugcheck 0xEF (`CRITICAL_PROCESS_DIED`). UTC is the
        // safe default: matches hcsshim's `noInheritHostTimezone` branch
        // and avoids plumbing the host's tz through (which would need a
        // `windows`-only `GetDynamicTimeZoneInformation` call). All fields
        // beyond `StandardName`/`DaylightName` are `omitempty` Go-side, so
        // we send the minimum non-empty object the deserialiser will accept.
        let uvm_config_str = serde_json::to_string(&serde_json::json!({
            "SystemType": "Container",
            "TimeZoneInformation": {
                "StandardName": "UTC",
                "DaylightName": "UTC",
            },
        }))?;
        let create_body = serde_json::json!({
            "ActivityId": uuid::Uuid::new_v4().to_string(),
            "ContainerId": NULL_CONTAINER_ID,
            "ContainerConfig": uvm_config_str,
        });
        let create_resp: ResponseBase = self
            .send_rpc_json(RpcMessageType::Create, &create_body)
            .await?;
        if create_resp.result != 0 {
            let hresult = u32::from_ne_bytes(create_resp.result.to_ne_bytes());
            return Err(GcsError::Negotiation(format!(
                "cold-start Create returned HRESULT {hresult:#x}: {}",
                create_resp.error_message
            )));
        }

        if !caps.send_host_start_message {
            return Ok(());
        }
        let start_body = serde_json::json!({
            "ActivityId": uuid::Uuid::new_v4().to_string(),
            "ContainerId": NULL_CONTAINER_ID,
        });
        let start_resp: ResponseBase = self
            .send_rpc_json(RpcMessageType::Start, &start_body)
            .await?;
        if start_resp.result != 0 {
            let hresult = u32::from_ne_bytes(start_resp.result.to_ne_bytes());
            return Err(GcsError::Negotiation(format!(
                "cold-start Start returned HRESULT {hresult:#x}: {}",
                start_resp.error_message
            )));
        }
        Ok(())
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

        eprintln!(
            "[t=+{}us] gcs-bridge-send: rpc={rpc:?} msg_id={message_id} frame_size={} payload_size={} payload={}",
            ts_us(),
            frame.len(),
            payload.len(),
            std::str::from_utf8(&payload).unwrap_or("<non-utf8>"),
        );

        let (tx, rx) = oneshot::channel();
        {
            // Scoped lock — do not hold across the stream.write_all().await
            // below. This keeps the pending map available to the background
            // reader task while the write is in flight. If the reader has
            // already exited (`closed`), bail now: writing the frame would
            // either fail or buffer into a half-open socket, and the oneshot
            // would never be woken.
            let mut guard = self.pending.lock().await;
            if guard.closed {
                return Err(GcsError::Closed);
            }
            guard.waiters.insert(message_id, tx);
        }

        if let Err(e) = self.stream.write_all(&frame).await {
            // Eagerly drop the waiter so the entry doesn't linger and so the
            // caller sees the underlying write error instead of Closed.
            self.pending.lock().await.waiters.remove(&message_id);
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
            // Diagnostic: emit to stderr (not tracing — the integration test
            // doesn't init a subscriber) so the cause of any "bridge closed"
            // lands in `stdout.log`. Verbose by design while the GCS protocol
            // handshake is still being debugged on nanoserver:ltsc2022.
            eprintln!("[t=+{}us] gcs-bridge-reader: started", ts_us());
            let mut frames_seen: u32 = 0;
            loop {
                let mut hdr_buf = [0u8; HEADER_LEN];
                if let Err(e) = stream.read_exact(&mut hdr_buf).await {
                    eprintln!(
                        "[t=+{}us] gcs-bridge-reader: header read failed after {frames_seen} frame(s): {e}",
                        ts_us(),
                    );
                    break;
                }
                let hdr = match frame::decode_header(&hdr_buf) {
                    Ok(h) => h,
                    Err(e) => {
                        eprintln!(
                            "[t=+{}us] gcs-bridge-reader: header decode failed (bytes={hdr_buf:02x?}): {e}",
                            ts_us(),
                        );
                        break;
                    }
                };
                eprintln!(
                    "[t=+{}us] gcs-bridge-reader: frame#{frames_seen} type=0x{:08x} size={} msg_id={}",
                    ts_us(),
                    hdr.r#type,
                    hdr.size,
                    hdr.message_id,
                );
                let body_len = (hdr.size as usize) - HEADER_LEN;
                let mut body = vec![0u8; body_len];
                if body_len > 0 {
                    if let Err(e) = stream.read_exact(&mut body).await {
                        eprintln!(
                            "[t=+{}us] gcs-bridge-reader: body read failed (need {body_len} bytes): {e}",
                            ts_us(),
                        );
                        break;
                    }
                    // Cap dumped payload at 512B so a verbose stream event
                    // doesn't flood stdout.log.
                    let cap = body.len().min(512);
                    eprintln!(
                        "[t=+{}us] gcs-bridge-reader: body[..{cap}]={:?}",
                        ts_us(),
                        String::from_utf8_lossy(&body[..cap]),
                    );
                }
                frames_seen = frames_seen.saturating_add(1);
                // Only RESPONSE frames are routed to an awaiting RPC waiter.
                // Notification / stream frames have no caller — drop them
                // here (a future task can plumb them to a notification
                // channel). The previous bit check used `RESPONSE_TYPE_OFFSET
                // = 0x1000_0000`, which is actually `MSG_TYPE_REQUEST` per
                // hcsshim — so it would have routed REQUESTS and dropped
                // RESPONSES, the exact opposite. We now use the proper top-4-
                // bit mask so the dispatch only fires for responses.
                if hdr.r#type & MSG_TYPE_MASK != MSG_TYPE_RESPONSE {
                    continue;
                }
                let waiter = {
                    let mut guard = pending.lock().await;
                    guard.waiters.remove(&hdr.message_id)
                };
                if let Some(tx) = waiter {
                    // Receiver may have been dropped (caller cancelled);
                    // ignore the send error in that case.
                    let _ = tx.send((hdr.r#type, body));
                }
            }
            // On reader exit, mark the bridge closed and drop every pending
            // sender to wake stuck RPCs with GcsError::Closed. Marking
            // `closed` under the same lock that gates waiter insertion closes
            // the race where a `send_rpc_json` that just passed the `closed`
            // check installs a waiter no reader will ever wake.
            {
                let mut g = pending.lock().await;
                eprintln!(
                    "[t=+{}us] gcs-bridge-reader: exiting; dropping {} pending waiters",
                    ts_us(),
                    g.waiters.len(),
                );
                g.closed = true;
                g.waiters.clear();
            }
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
            pending: Arc::new(Mutex::new(PendingState {
                closed: false,
                waiters: HashMap::new(),
            })),
        };
        bridge.spawn_reader();
        let caps = bridge.negotiate_protocol().await?;
        // hcsshim sends Create+Start IMMEDIATELY after a successful
        // negotiate, before any other RPC. Skipping this causes the peer to
        // close the bridge ~instantly. See `cold_start_create_start` for
        // the load-bearing detail.
        bridge.cold_start_create_start(&caps).await?;
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
