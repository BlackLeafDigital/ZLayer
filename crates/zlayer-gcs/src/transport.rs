//! Windows `AF_HYPERV` socket transport for GCS bridge connections.
//!
//! Host-side socket bound to a UVM's runtime GUID, talking to the in-guest
//! GCS via service GUID `acef5661-84a1-4e44-856b-6245e69f4620`.
//!
//! Winsock's hvsock surface is blocking-only on Windows — there is no IOCP
//! integration analogous to TCP. We hide that by running each blocking call
//! through [`tokio::task::spawn_blocking`]. Throughput is fine for GCS RPCs
//! (small JSON frames, low rate).
//!
//! On non-Windows targets this module is empty: the public types only exist
//! behind `#[cfg(target_os = "windows")]` because there's no equivalent of
//! `AF_HYPERV` to stub out. Cross-platform code that needs to compile on
//! Linux should gate its use of `transport::*` types on the same cfg.

#![cfg(target_os = "windows")]
// Unsafe FFI is intrinsic to this module — every Winsock call is `unsafe fn`.
// Each `unsafe` block carries a SAFETY comment explaining the invariants.
#![allow(unsafe_code)]

use std::mem::size_of;
use std::sync::{Arc, Once};

use windows::Win32::Networking::WinSock::{
    accept, bind, closesocket, connect, listen, recv, send, shutdown, WSAGetLastError, WSASocketW,
    WSAStartup, INVALID_SOCKET, SD_BOTH, SEND_RECV_FLAGS, SOCKADDR, SOCKET, SOCKET_ERROR,
    SOCK_STREAM, WINSOCK_SOCKET_TYPE, WSADATA, WSA_FLAG_OVERLAPPED,
};

use crate::error::{GcsError, GcsResult};

/// GCS service GUID (well-known) — see hcsshim
/// `internal/gcs/service_guid.go` for the matching constant on the Go side.
pub const GCS_SERVICE_GUID: windows::core::GUID =
    windows::core::GUID::from_u128(0xacef_5661_84a1_4e44_856b_6245_e69f_4620);

/// `HV_GUID_LOOPBACK` — partition ID used for host-local hvsock connections
/// (host process talking to a service hosted in the same partition).
pub const HV_GUID_LOOPBACK: windows::core::GUID =
    windows::core::GUID::from_u128(0xe0e1_6197_dd56_4a10_9195_5ee7_a155_a838);

/// `HV_GUID_WILDCARD` — partition ID used by listeners that want to accept
/// connections from any partition.
pub const HV_GUID_WILDCARD: windows::core::GUID =
    windows::core::GUID::from_u128(0x0000_0000_0000_0000_0000_0000_0000_0000);

/// `WindowsGcsHvHostID` — the hvsock "parent" (host) partition address the
/// in-guest GCS uses when it dials out to the host.
///
/// Needed as the
/// `ParentAddress` of the post-connect `HvSocket` `ModifySettings` that external
/// GCS connections must send. Matches hcsshim `internal/gcs/prot/protocol.go`
/// `WindowsGcsHvHostID` (894cc2d6-9d79-424f-93fe-42969ae6d8d1).
pub const WINDOWS_GCS_HV_HOST_ID: windows::core::GUID =
    windows::core::GUID::from_u128(0x894c_c2d6_9d79_424f_93fe_4296_9ae6_d8d1);

/// `AF_HYPERV` socket address family (34 / `0x22`). Held as `u16` because
/// that is the type of the `family` field in `SOCKADDR_HV`; widened to
/// `i32` at the `WSASocketW` call site.
const AF_HYPERV: u16 = 34;

/// `HV_PROTOCOL_RAW` — the only protocol value Windows accepts for hvsock.
const HV_PROTOCOL_RAW: i32 = 1;

/// Default listen backlog for [`HvSockListener`]. GCS rarely sees more than
/// one inbound connection per UVM, but Winsock allows up to `SOMAXCONN`.
const LISTEN_BACKLOG: i32 = 8;

/// Windows `SOCKADDR_HV` — the address structure for `AF_HYPERV` sockets.
///
/// `windows-rs 0.62` does not expose this struct, so it's declared inline
/// here. Layout matches the C definition in `<hvsocket.h>` (Win10 SDK):
///
/// ```c
/// typedef struct _SOCKADDR_HV {
///     ADDRESS_FAMILY Family;     // u16
///     USHORT         Reserved;   // u16
///     GUID           VmId;
///     GUID           ServiceId;
/// } SOCKADDR_HV, *PSOCKADDR_HV;
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SockAddrHv {
    /// Address family — must be `AF_HYPERV` (34).
    pub family: u16,
    /// Reserved — must be 0.
    pub reserved: u16,
    /// Hyper-V partition GUID (target VM, or `HV_GUID_WILDCARD` for listen).
    pub vm_id: windows::core::GUID,
    /// Service GUID inside the target partition.
    pub service_id: windows::core::GUID,
}

impl SockAddrHv {
    /// Construct a new `SOCKADDR_HV` filled in for the given (vm, service)
    /// pair. `family` is fixed to `AF_HYPERV` and `reserved` is zeroed.
    #[must_use]
    pub const fn new(vm_id: windows::core::GUID, service_id: windows::core::GUID) -> Self {
        Self {
            family: AF_HYPERV,
            reserved: 0,
            vm_id,
            service_id,
        }
    }
}

/// One-time `WSAStartup` invocation. Winsock requires this be called before
/// any socket APIs are used; subsequent calls are no-ops once the first
/// succeeds for the lifetime of the process. We never pair it with
/// `WSACleanup` because the matching call would have to happen at process
/// exit and Rust's drop order across threads makes that fragile — Winsock
/// tolerates leaking the startup ref and the OS reclaims it on exit.
fn ensure_winsock_started() -> GcsResult<()> {
    static INIT: Once = Once::new();
    static mut STARTUP_ERR: i32 = 0;

    INIT.call_once(|| {
        let mut data = WSADATA::default();
        // SAFETY: `WSAStartup` is the documented entry point; we pass a
        // valid `&mut WSADATA` and the requested version `2.2` (low byte
        // major, high byte minor) per MSDN.
        let rc = unsafe { WSAStartup(0x0202, &raw mut data) };
        if rc != 0 {
            // SAFETY: `STARTUP_ERR` is only written here inside `call_once`,
            // and only read after `call_once` has returned (a happens-before
            // relationship enforced by `Once`).
            unsafe {
                STARTUP_ERR = rc;
            }
        }
    });

    // SAFETY: see above — read is sequenced after the init writer.
    let rc = unsafe { STARTUP_ERR };
    if rc == 0 {
        Ok(())
    } else {
        Err(GcsError::Hvsock(format!("WSAStartup failed: rc={rc}")))
    }
}

/// Pull the last Winsock error and stringify it with the supplied context.
fn wsa_err(ctx: &str) -> GcsError {
    // SAFETY: `WSAGetLastError` reads thread-local state set by the most
    // recent Winsock call on this thread — always safe to call.
    let code = unsafe { WSAGetLastError() };
    GcsError::Hvsock(format!("WSA error {code:?}: {ctx}", code = code.0))
}

/// Owned wrapper around a raw `SOCKET` value. Stores the socket as `usize`
/// (matching `SOCKET`'s underlying repr) so the inner type is trivially
/// `Send` + `Sync`, and `Drop` calls `closesocket` exactly once.
#[derive(Debug)]
struct HvSocketInner {
    raw: usize,
}

impl HvSocketInner {
    const fn from_socket(s: SOCKET) -> Self {
        Self { raw: s.0 }
    }

    const fn socket(&self) -> SOCKET {
        SOCKET(self.raw)
    }
}

impl Drop for HvSocketInner {
    fn drop(&mut self) {
        if self.raw != INVALID_SOCKET.0 {
            // Best-effort `shutdown(SD_BOTH)` BEFORE `closesocket`. Winsock
            // documents that `closesocket` on a socket with pending blocking
            // calls from other threads forces those calls to fail with
            // `WSAEINTR` — but only after a brief grace period, and only for
            // some blocking ops. A prior `shutdown` makes the wake-up of an
            // in-flight blocking `accept`/`recv` on another thread immediate
            // and deterministic, which is the difference between a clean
            // teardown and a wedged tokio blocking-pool thread that prevents
            // the runtime (and the test process) from ever exiting.
            //
            // SAFETY: `raw` came from a successful `WSASocketW`/`accept`
            // call; we own the handle and have not closed it elsewhere
            // (Inner is wrapped in `Arc` so Drop only runs when the last
            // clone is released).
            let _ = unsafe { shutdown(self.socket(), SD_BOTH) };
            // SAFETY: same as above — exactly one `closesocket` per handle.
            let _ = unsafe { closesocket(self.socket()) };
        }
    }
}

/// Create a fresh `AF_HYPERV` `SOCK_STREAM` socket via `WSASocketW`.
fn new_hvsock() -> GcsResult<SOCKET> {
    ensure_winsock_started()?;
    // SAFETY: arguments are constants of the documented kind; no protocol
    // info pointer is required for hvsock, hence `None`.
    let socket = unsafe {
        WSASocketW(
            i32::from(AF_HYPERV),
            SOCK_STREAM.0,
            HV_PROTOCOL_RAW,
            None,
            0,
            WSA_FLAG_OVERLAPPED,
        )
    };
    socket.map_err(|e| GcsError::Hvsock(format!("WSASocketW: {e}")))
}

/// Connected hvsock stream wrapping a blocking Winsock socket. All I/O runs
/// on a tokio blocking thread via [`tokio::task::spawn_blocking`].
#[derive(Debug, Clone)]
pub struct HvSockStream {
    inner: Arc<HvSocketInner>,
}

impl HvSockStream {
    /// Connect to `(vm_id, service_id)` — the GCS endpoint inside the
    /// running UVM. The blocking `connect` call is executed on a tokio
    /// blocking thread so callers may `.await` from any async context.
    pub async fn connect(
        vm_id: windows::core::GUID,
        service_id: windows::core::GUID,
    ) -> GcsResult<Self> {
        let socket = new_hvsock()?;
        let inner = Arc::new(HvSocketInner::from_socket(socket));

        let inner_for_blocking = inner.clone();
        let join = tokio::task::spawn_blocking(move || -> GcsResult<()> {
            let addr = SockAddrHv::new(vm_id, service_id);
            let addr_ptr: *const SOCKADDR = std::ptr::from_ref(&addr).cast();
            let addr_len = i32::try_from(size_of::<SockAddrHv>())
                .map_err(|e| GcsError::Hvsock(format!("addr size overflow: {e}")))?;
            // SAFETY: `addr_ptr` points to a live `SockAddrHv` on the stack
            // that outlives the call; `addr_len` is the matching size. The
            // socket handle is owned by `inner_for_blocking` and stays
            // valid for the duration of this closure.
            let rc = unsafe { connect(inner_for_blocking.socket(), addr_ptr, addr_len) };
            if rc == SOCKET_ERROR {
                Err(wsa_err("connect"))
            } else {
                Ok(())
            }
        })
        .await
        .map_err(|e| GcsError::Hvsock(format!("connect join: {e}")))?;
        join?;

        Ok(Self { inner })
    }

    /// Connect to the loopback partition (for host-local testing).
    pub async fn connect_loopback(service_id: windows::core::GUID) -> GcsResult<Self> {
        Self::connect(HV_GUID_LOOPBACK, service_id).await
    }

    /// Read exactly `buf.len()` bytes (analogous to `AsyncReadExt::read_exact`).
    ///
    /// Returns [`GcsError::Closed`] if the peer closes the connection
    /// before the full buffer is filled.
    pub async fn read_exact(&self, buf: &mut [u8]) -> GcsResult<()> {
        if buf.is_empty() {
            return Ok(());
        }
        let inner = self.inner.clone();
        let len = buf.len();
        let filled = tokio::task::spawn_blocking(move || -> GcsResult<Vec<u8>> {
            let mut out = vec![0u8; len];
            let mut filled = 0usize;
            while filled < len {
                // SAFETY: `out[filled..]` is a valid &mut [u8] slice;
                // `recv` writes at most `slice.len()` bytes; the socket
                // is owned by `inner`.
                let n = unsafe { recv(inner.socket(), &mut out[filled..], SEND_RECV_FLAGS(0)) };
                if n == SOCKET_ERROR {
                    return Err(wsa_err("recv"));
                }
                if n == 0 {
                    return Err(GcsError::Closed);
                }
                let advanced =
                    usize::try_from(n).map_err(|e| GcsError::Hvsock(format!("recv count: {e}")))?;
                filled += advanced;
            }
            Ok(out)
        })
        .await
        .map_err(|e| GcsError::Hvsock(format!("read_exact join: {e}")))??;

        buf.copy_from_slice(&filled);
        Ok(())
    }

    /// Write all `buf` bytes.
    pub async fn write_all(&self, buf: &[u8]) -> GcsResult<()> {
        if buf.is_empty() {
            return Ok(());
        }
        let inner = self.inner.clone();
        let owned = buf.to_vec();
        tokio::task::spawn_blocking(move || -> GcsResult<()> {
            let mut sent = 0usize;
            while sent < owned.len() {
                // SAFETY: `owned[sent..]` is a valid &[u8] slice; `send`
                // reads at most `slice.len()` bytes; the socket is owned
                // by `inner`.
                let n = unsafe { send(inner.socket(), &owned[sent..], SEND_RECV_FLAGS(0)) };
                if n == SOCKET_ERROR {
                    return Err(wsa_err("send"));
                }
                if n == 0 {
                    return Err(GcsError::Closed);
                }
                let advanced =
                    usize::try_from(n).map_err(|e| GcsError::Hvsock(format!("send count: {e}")))?;
                sent += advanced;
            }
            Ok(())
        })
        .await
        .map_err(|e| GcsError::Hvsock(format!("write_all join: {e}")))??;
        Ok(())
    }

    /// Gracefully shut down both directions of the stream. Consumes `self`
    /// because the socket is no longer usable for I/O after shutdown.
    pub async fn shutdown(self) -> GcsResult<()> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || -> GcsResult<()> {
            // SAFETY: `inner.socket()` is a valid open socket owned by the
            // Arc; `SD_BOTH` is a documented constant.
            let rc = unsafe { shutdown(inner.socket(), SD_BOTH) };
            if rc == SOCKET_ERROR {
                Err(wsa_err("shutdown"))
            } else {
                Ok(())
            }
        })
        .await
        .map_err(|e| GcsError::Hvsock(format!("shutdown join: {e}")))??;
        Ok(())
    }
}

/// Bound listener that accepts inbound hvsock connections (host receives
/// the in-guest GCS connecting back). Used when the host-side wants to be
/// the server — hcsshim runs in this mode for some GCS flows.
#[derive(Debug, Clone)]
pub struct HvSockListener {
    inner: Arc<HvSocketInner>,
}

impl HvSockListener {
    /// Bind to `(vm_id, service_id)` and start listening. `vm_id` is
    /// normally [`HV_GUID_WILDCARD`] so any inbound partition can connect.
    pub async fn bind(
        vm_id: windows::core::GUID,
        service_id: windows::core::GUID,
    ) -> GcsResult<Self> {
        let socket = new_hvsock()?;
        let inner = Arc::new(HvSocketInner::from_socket(socket));

        let inner_for_blocking = inner.clone();
        tokio::task::spawn_blocking(move || -> GcsResult<()> {
            let addr = SockAddrHv::new(vm_id, service_id);
            let addr_ptr: *const SOCKADDR = std::ptr::from_ref(&addr).cast();
            let addr_len = i32::try_from(size_of::<SockAddrHv>())
                .map_err(|e| GcsError::Hvsock(format!("addr size overflow: {e}")))?;
            // SAFETY: same invariants as `connect` above.
            let bind_rc = unsafe { bind(inner_for_blocking.socket(), addr_ptr, addr_len) };
            if bind_rc == SOCKET_ERROR {
                return Err(wsa_err("bind"));
            }
            // SAFETY: socket handle is valid and owned by `inner_for_blocking`.
            let listen_rc = unsafe { listen(inner_for_blocking.socket(), LISTEN_BACKLOG) };
            if listen_rc == SOCKET_ERROR {
                return Err(wsa_err("listen"));
            }
            Ok(())
        })
        .await
        .map_err(|e| GcsError::Hvsock(format!("bind join: {e}")))??;

        Ok(Self { inner })
    }

    /// Accept the next inbound connection. Blocks (on the tokio blocking
    /// thread pool) until a peer connects.
    ///
    /// Cancellation safety: the blocking task captures only the raw `SOCKET`
    /// handle (a `usize` newtype) — NOT an `Arc<HvSocketInner>` clone. If the
    /// caller's future is dropped (e.g. `tokio::time::timeout` fires), the
    /// listener's only remaining `Arc` reference drops, [`HvSocketInner::Drop`]
    /// runs, and the `shutdown` + `closesocket` there force the blocking
    /// `accept` to return `WSAEINTR`/`WSAENOTSOCK` instead of wedging the
    /// blocking-pool thread forever (which would prevent the tokio runtime
    /// — and the test process — from ever exiting).
    pub async fn accept(&self) -> GcsResult<HvSockStream> {
        let raw_socket = self.inner.socket();
        let accepted = tokio::task::spawn_blocking(move || -> GcsResult<SOCKET> {
            // SAFETY: `raw_socket` is a valid, bound, listening socket owned
            // by the caller's `HvSockListener` for the lifetime of this
            // blocking call. If the listener is dropped concurrently, the
            // socket will be `closesocket`-ed which makes `accept` return
            // an error — but it will never be a use-after-free because
            // Winsock handle values are not reused while the call is in
            // flight on this thread. `accept` allocates a new socket handle
            // which we take ownership of (wrapped in `HvSocketInner` below).
            let s = unsafe { accept(raw_socket, None, None) }
                .map_err(|e| GcsError::Hvsock(format!("accept: {e}")))?;
            Ok(s)
        })
        .await
        .map_err(|e| GcsError::Hvsock(format!("accept join: {e}")))??;

        Ok(HvSockStream {
            inner: Arc::new(HvSocketInner::from_socket(accepted)),
        })
    }
}

// `WINSOCK_SOCKET_TYPE` is a `pub struct(pub i32)` newtype. We re-export the
// inner value cleanly by accessing `.0` at the call site; this stub silences
// any "unused import" hint if the underlying crate changes shape later.
#[allow(dead_code)]
const _: WINSOCK_SOCKET_TYPE = SOCK_STREAM;

#[cfg(test)]
mod tests {
    use super::{SockAddrHv, AF_HYPERV, GCS_SERVICE_GUID, HV_GUID_LOOPBACK, HV_GUID_WILDCARD};

    #[test]
    fn sockaddr_hv_layout() {
        let addr = SockAddrHv::new(HV_GUID_LOOPBACK, GCS_SERVICE_GUID);
        assert_eq!(addr.family, AF_HYPERV);
        assert_eq!(addr.reserved, 0);
        assert_eq!(addr.vm_id, HV_GUID_LOOPBACK);
        assert_eq!(addr.service_id, GCS_SERVICE_GUID);

        // 2 (family) + 2 (reserved) + 16 (vm_id) + 16 (service_id) = 36
        assert_eq!(std::mem::size_of::<SockAddrHv>(), 36);
        // No padding/alignment surprises — must be 4-byte aligned for the
        // u16 fields, no more.
        assert_eq!(std::mem::align_of::<SockAddrHv>(), 4);
    }

    #[test]
    fn wildcard_guid_is_zero() {
        // Sanity check that the wildcard GUID literal is all zeros, as the
        // hcsshim source documents.
        assert_eq!(HV_GUID_WILDCARD.to_u128(), 0);
    }

    /// Regression test for the hour-long hang observed in the Hyper-V e2e
    /// runs (`run-g4-retry` / `run-g4-retry-2`): when the bridge's `accept`
    /// future hit its 120s timeout, the underlying blocking `accept()` on a
    /// tokio blocking-pool thread wedged forever because the blocking task
    /// captured a clone of the `Arc<HvSocketInner>` — preventing `Drop` from
    /// ever running and `closesocket` from waking the syscall.
    ///
    /// With the fix, the blocking task only captures the raw `SOCKET` value;
    /// when the listener drops, `HvSocketInner::Drop` runs `shutdown` +
    /// `closesocket`, the blocking `accept` returns an error, and the
    /// tokio runtime is free to shut down.
    ///
    /// Gated on Windows because hvsock is Windows-only and binding requires
    /// the Hyper-V hvsock provider. If `bind` fails (no Hyper-V on the host)
    /// we skip — the bug only manifests on a host that can actually bind.
    #[tokio::test]
    async fn accept_timeout_does_not_wedge_runtime() {
        use std::time::{Duration, Instant};

        // Use the wildcard listen address with a fresh per-process service
        // GUID so reruns don't collide with leftover bound sockets.
        let svc = windows::core::GUID::from_u128(
            0xdead_beef_0000_4000_8000_0000_0000_0001_u128 ^ u128::from(std::process::id()),
        );

        let Ok(listener) = super::HvSockListener::bind(HV_GUID_WILDCARD, svc).await else {
            // No Hyper-V on this host — fix is unverifiable here.
            return;
        };

        let started = Instant::now();
        let res = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
        assert!(
            res.is_err(),
            "accept should hit the timeout (no peer dials)"
        );

        // Drop the listener; this must trigger `HvSocketInner::Drop` which
        // closes the socket and unblocks the wedged blocking-pool accept.
        drop(listener);

        // Give the blocking task a generous-but-bounded chance to return.
        // Pre-fix this would hang for ~hours; post-fix it returns immediately
        // once `closesocket` runs. We assert under 5s to leave huge headroom
        // for slow CI hosts without making a regression invisible.
        let total = started.elapsed();
        assert!(
            total < Duration::from_secs(5),
            "accept_timeout test took {total:?}; regression: blocking pool wedged"
        );
    }
}
