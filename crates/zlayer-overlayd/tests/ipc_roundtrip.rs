//! Cross-platform IPC integration test for `zlayer-overlayd`.
//!
//! Starts a real [`OverlaydServer`] behind the [`transport::serve`] accept loop
//! over a Unix domain socket in a temp dir, then drives it through a real
//! [`OverlaydClient`] connection and asserts request/response round-trips work
//! end-to-end. This exercises the full stack: length-prefixed JSON framing, the
//! client's request-id matching, and the server's dispatch — over an actual
//! socket rather than an in-memory duplex.
//!
//! Only side-effect-free requests are issued (`Status`, `SetLocalNodeId`,
//! `SetLocalWgPubkey`) so no real overlay/adapter setup is triggered.
//!
//! Unix-only: the test uses a `UnixStream`-backed transport. The Windows
//! named-pipe end-to-end path is harder to stand up in a unit test, and the
//! framing itself is already covered by `transport.rs`'s in-memory-duplex unit
//! tests, which run on every platform.

#![cfg(unix)]

use std::sync::Arc;

use tokio::sync::Mutex;
use zlayer_overlayd::transport;
use zlayer_overlayd::{OverlaydClient, OverlaydServer};
use zlayer_types::overlayd::{OverlaydRequest, OverlaydResponse};

/// Build a unique socket + data-dir path under the system temp dir so parallel
/// test runs (and reruns after a crash) never collide.
fn unique_paths() -> (std::path::PathBuf, std::path::PathBuf) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let base = std::env::temp_dir().join(format!(
        "zlayer-overlayd-ipc-{}-{}-{}",
        std::process::id(),
        stamp,
        n
    ));
    let socket = base.with_extension("sock");
    let data_dir = base.with_extension("data");
    (socket, data_dir)
}

#[tokio::test(flavor = "multi_thread")]
async fn ipc_round_trip_over_unix_socket() {
    let (socket, data_dir) = unique_paths();
    // Clean any leftovers from a prior crashed run.
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_dir_all(&data_dir);

    // Wrap a real server engine in Arc<Mutex<_>> so the accept-loop handler can
    // serve every connection through the one shared engine.
    let server = Arc::new(Mutex::new(OverlaydServer::new(data_dir.clone())));

    // Spawn the real accept loop. Each connection loops recv -> handle -> send
    // until the peer hangs up (FramedConn::recv returns OverlaydError::Closed).
    let serve_socket = socket.clone();
    let serve_server = Arc::clone(&server);
    let serve_task = tokio::spawn(async move {
        let _ = transport::serve(&serve_socket, move |mut conn| {
            let server = Arc::clone(&serve_server);
            async move {
                loop {
                    // Peer closed or transport error -> end this connection task.
                    let Ok(frame) = conn.recv().await else {
                        return;
                    };
                    // A client only ever sends Request frames; ignore others.
                    if let zlayer_types::overlayd::OverlaydFrame::Request { id, request } = frame {
                        let response = server.lock().await.handle(request).await;
                        if conn
                            .send(&zlayer_types::overlayd::OverlaydFrame::Response { id, response })
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            }
        })
        .await;
    });

    // Connect with backoff so we don't race the listener's bind().
    let mut client = OverlaydClient::connect_with_backoff(&socket)
        .await
        .expect("client must connect to the overlayd socket");

    // 1. Status on a fresh server: must NOT error, and the snapshot must
    //    deserialize as an empty/default view (no overlay set up yet).
    let resp = client
        .request(OverlaydRequest::Status)
        .await
        .expect("Status request must round-trip");
    let snap = match resp {
        OverlaydResponse::Status(snap) => snap,
        other => panic!("expected Status response, got {other:?}"),
    };
    assert!(snap.interface.is_none(), "fresh server has no interface");
    assert!(snap.node_ip.is_none(), "fresh server has no node_ip");
    assert!(snap.public_key.is_none(), "fresh server has no public_key");
    assert_eq!(snap.peer_count, 0);
    assert_eq!(snap.service_count, 0);
    assert!(snap.peers.is_empty());

    // 2. SetLocalNodeId then SetLocalWgPubkey: both side-effect-free, expect Ok.
    let resp = client
        .request(OverlaydRequest::SetLocalNodeId { node_id: 7 })
        .await
        .expect("SetLocalNodeId must round-trip");
    assert!(
        matches!(resp, OverlaydResponse::Ok),
        "SetLocalNodeId should return Ok, got {resp:?}"
    );

    let resp = client
        .request(OverlaydRequest::SetLocalWgPubkey { pubkey: "k".into() })
        .await
        .expect("SetLocalWgPubkey must round-trip");
    assert!(
        matches!(resp, OverlaydResponse::Ok),
        "SetLocalWgPubkey should return Ok, got {resp:?}"
    );

    // `call` should fold a non-error response identically (sanity-check the
    // client's id-matching path once more over the live socket).
    let resp = client
        .call(OverlaydRequest::SetLocalNodeId { node_id: 9 })
        .await
        .expect("call() must succeed for a non-error response");
    assert!(matches!(resp, OverlaydResponse::Ok));

    // Drop the client to close its half of the socket, then stop the listener.
    drop(client);
    serve_task.abort();

    // Tidy up the on-disk temp artifacts.
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_dir_all(&data_dir);
}
