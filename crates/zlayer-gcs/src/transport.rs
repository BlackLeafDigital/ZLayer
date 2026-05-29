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
            // SAFETY: `raw` came from a successful `WSASocketW`/`accept`
            // call; we own the handle and have not closed it elsewhere
            // (Inner is wrapped in `Arc` so Drop only runs when the last
            // clone is released).
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
    pub async fn accept(&self) -> GcsResult<HvSockStream> {
        let inner = self.inner.clone();
        let accepted = tokio::task::spawn_blocking(move || -> GcsResult<SOCKET> {
            // SAFETY: `inner.socket()` is a valid, bound, listening socket.
            // `accept` allocates a new socket handle which we take
            // ownership of (wrapped in `HvSocketInner` below).
            let s = unsafe { accept(inner.socket(), None, None) }
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
}
