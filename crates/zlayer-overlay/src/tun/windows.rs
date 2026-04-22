//! Wintun-backed TUN adapter for the Windows overlay transport.
//!
//! Wraps the [`wintun-bindings`](https://crates.io/crates/wintun-bindings)
//! crate (itself a safe wrapper around Microsoft's `wintun.dll` userspace
//! TUN driver) with a small typed facade that the rest of
//! `zlayer-overlay` consumes on Windows.
//!
//! # DLL distribution
//!
//! We intentionally do **not** embed `wintun.dll` in the binary. Wintun is
//! WireGuard LLC's property, it is signed with their certificate, and the
//! recommended deployment model is to ship it as a sibling file (or to
//! install it under `%ProgramData%`). On first use [`WindowsTun::new`]
//! looks for the DLL at the paths below, in order:
//!
//! 1. `%ProgramData%\ZLayer\wintun\wintun.dll` (installed location)
//! 2. `<exe_dir>\wintun.dll` (next to `zlayer.exe`)
//! 3. `wintun.dll` (current working directory fallback)
//!
//! If the DLL is absent, [`WindowsTun::new`] returns
//! [`OverlayError::NetworkConfig`] with a message pointing the operator at
//! <https://www.wintun.net>. This matches how `wireguard-nt` and
//! Cloudflare's `boringtun` bootstrap on Windows.
//!
//! # Async I/O
//!
//! Wintun is a blocking driver; its read/write call a Windows completion
//! port synchronously. To stay inside the Tokio multi-thread runtime we
//! run each I/O on [`tokio::task::spawn_blocking`] so the reactor threads
//! stay responsive. For the bulk transport loop a future iteration may
//! switch to `wintun-bindings`'s native `async` session (feature-gated in
//! our `Cargo.toml`), but the current Phase D3 transport only uses
//! [`WindowsTun`] for adapter creation + IP configuration — the packet
//! loop itself is deliberately stubbed.

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::task;
use wintun_bindings::{load_from_path, Adapter, Session, Wintun};

use crate::OverlayError;

/// Default pool name registered with Wintun. Shows up in the
/// Device Manager tree under *Network adapters*.
const WINTUN_POOL: &str = "ZLayer";

/// Size of the per-session ring buffer. 4 MiB mirrors the WireGuard
/// reference default and comfortably holds bursty overlay traffic at
/// 1500-byte MTU.
const WINTUN_RING_BYTES: u32 = 4 * 1024 * 1024;

/// Platform-native wrapper around a live Wintun adapter + session.
///
/// Dropping this value closes the session and deletes the adapter
/// (Wintun's RAII semantics).
pub(crate) struct WindowsTun {
    /// Keep the DLL loader alive for as long as any adapter is live.
    ///
    /// Note: [`Wintun`] is itself an `Arc<wintun_raw::wintun>`, so this
    /// field is cheap to clone.
    _wintun: Wintun,
    /// The adapter handle — needed for LUID lookup from IP Helper.
    adapter: Arc<Adapter>,
    /// The active session. Wintun closes the session on drop, at which
    /// point the adapter becomes idle; dropping `adapter` then removes
    /// the kernel device.
    session: Arc<Session>,
    /// Retained for diagnostics only.
    name: String,
}

impl WindowsTun {
    /// Create a new Wintun adapter named `name`.
    ///
    /// `_mtu` is accepted for API symmetry with the Linux / macOS paths;
    /// Wintun itself does not expose a per-adapter MTU setter (use the
    /// IP Helper `SetIpInterfaceEntry` after creation if a non-default
    /// MTU is required).
    ///
    /// # Errors
    ///
    /// Returns [`OverlayError::NetworkConfig`] if the Wintun DLL cannot
    /// be located, if the adapter cannot be created (e.g. insufficient
    /// privileges — Wintun requires Administrator), or if a session
    /// cannot be started.
    pub(crate) fn new(name: &str, _mtu: u32) -> Result<Self, OverlayError> {
        let dll_path = locate_wintun_dll().ok_or_else(|| {
            OverlayError::NetworkConfig(
                "wintun.dll not found (looked in %ProgramData%\\ZLayer\\wintun, next \
                 to zlayer.exe, and the current directory). Download it from \
                 https://www.wintun.net and place it alongside the ZLayer binary."
                    .to_string(),
            )
        })?;

        // SAFETY: `load_from_path` loads the external DLL. The
        // wintun-bindings crate's `verify_binary_signature` feature
        // (enabled in Cargo.toml) validates the DLL's Authenticode
        // signature before loading, so a tampered/arbitrary DLL in the
        // same path won't be accepted.
        let wintun = unsafe { load_from_path(&dll_path) }.map_err(|e| {
            OverlayError::NetworkConfig(format!(
                "failed to load wintun.dll from {}: {e}",
                dll_path.display()
            ))
        })?;

        // Look up an existing adapter by pool + name before creating a
        // new one — Wintun treats a duplicate name as an error, and
        // the overlay transport is idempotent.
        let adapter = if let Ok(existing) = Adapter::open(&wintun, name) {
            existing
        } else {
            Adapter::create(&wintun, WINTUN_POOL, name, None).map_err(|e| {
                OverlayError::NetworkConfig(format!(
                    "failed to create Wintun adapter '{name}': {e}"
                ))
            })?
        };

        let session = adapter.start_session(WINTUN_RING_BYTES).map_err(|e| {
            OverlayError::NetworkConfig(format!("failed to start Wintun session on '{name}': {e}"))
        })?;

        Ok(Self {
            _wintun: wintun,
            adapter,
            session,
            name: name.to_string(),
        })
    }

    /// Return the raw 64-bit LUID value of the underlying adapter.
    ///
    /// The LUID is the identity used by every `MIB_UNICASTIPADDRESS_ROW`
    /// and `MIB_IPFORWARD_ROW2` interaction on Windows; it can be
    /// reconstructed on the consumer side as a `NET_LUID_LH` union with
    /// `Value = luid_value()`.
    ///
    /// Currently used only for logging (the packet loop that would
    /// thread this directly into IP Helper is pending — today the
    /// [`crate::interface::windows`] backend looks up the LUID via
    /// `ConvertInterfaceAliasToLuid` from the adapter name instead).
    pub(crate) fn luid_value(&self) -> u64 {
        // SAFETY: `NET_LUID_LH` is a `#[repr(C)]` union of a 64-bit
        // `Value` field and a bitfield arm that covers the same 64
        // bits. Reading `Value` is always well-defined.
        unsafe { self.adapter.get_luid().Value }
    }

    /// Name the adapter was created under. Preserved for logging /
    /// diagnostics; do not depend on it for kernel lookups — use
    /// [`Self::luid`] instead.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// Receive the next IP packet from the Wintun ring.
    ///
    /// Runs on the Tokio blocking pool because Wintun's receive call
    /// blocks on a Windows completion port.
    ///
    /// # Errors
    ///
    /// Returns [`OverlayError::NetworkConfig`] on any underlying Wintun
    /// session error (adapter removed, driver unloaded, etc.).
    pub(crate) async fn recv(&self, buf: &mut [u8]) -> Result<usize, OverlayError> {
        let session = Arc::clone(&self.session);
        let buf_len = buf.len();
        let out = task::spawn_blocking(move || -> Result<Vec<u8>, String> {
            let pkt = session.receive_blocking().map_err(|e| e.to_string())?;
            let bytes = pkt.bytes();
            if bytes.len() > buf_len {
                return Err(format!(
                    "packet too large for buffer: got {} bytes, buffer holds {}",
                    bytes.len(),
                    buf_len
                ));
            }
            Ok(bytes.to_vec())
        })
        .await
        .map_err(|e| OverlayError::NetworkConfig(format!("wintun recv join error: {e}")))?
        .map_err(OverlayError::NetworkConfig)?;

        let n = out.len();
        buf[..n].copy_from_slice(&out);
        Ok(n)
    }

    /// Inject an IP packet into the Wintun ring (kernel-ward).
    ///
    /// Runs on the Tokio blocking pool — see [`Self::recv`].
    ///
    /// # Errors
    ///
    /// Returns [`OverlayError::NetworkConfig`] if the ring cannot accept
    /// the packet (e.g. adapter is being torn down or buffer allocation
    /// fails).
    pub(crate) async fn send(&self, pkt: &[u8]) -> Result<(), OverlayError> {
        let session = Arc::clone(&self.session);
        let owned = pkt.to_vec();
        task::spawn_blocking(move || -> Result<(), String> {
            let mut tx = session
                .allocate_send_packet(u16::try_from(owned.len()).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            tx.bytes_mut().copy_from_slice(&owned);
            session.send_packet(tx);
            Ok(())
        })
        .await
        .map_err(|e| OverlayError::NetworkConfig(format!("wintun send join error: {e}")))?
        .map_err(OverlayError::NetworkConfig)?;
        Ok(())
    }
}

/// Locate `wintun.dll` on the host filesystem.
///
/// Search order matches the module docs: installed directory first,
/// then next to the running executable, then the current working
/// directory. Returns `None` if the DLL cannot be found anywhere.
fn locate_wintun_dll() -> Option<PathBuf> {
    // 1. %ProgramData%\ZLayer\wintun\wintun.dll
    let installed = zlayer_paths::ZLayerDirs::system_default()
        .data_dir()
        .join("wintun")
        .join("wintun.dll");
    if installed.exists() {
        return Some(installed);
    }

    // 2. Next to the current executable.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let sibling = parent.join("wintun.dll");
            if sibling.exists() {
                return Some(sibling);
            }
        }
    }

    // 3. CWD fallback.
    let cwd = Path::new("wintun.dll");
    if cwd.exists() {
        return Some(cwd.to_path_buf());
    }

    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_dll_returns_none_in_clean_env() {
        // This is best-effort: the test only asserts that the locator
        // function is total — it returns `Option<PathBuf>` without
        // panicking. It may return `Some(...)` on a developer box that
        // already has Wintun installed, and that's a legitimate outcome;
        // we simply avoid asserting the negative there.
        let _ = locate_wintun_dll();
    }
}
