//! RAII wrappers for raw HCN handles.
//!
//! HCN exposes three handle types — network, endpoint, and namespace — each
//! released via a matching `HcnClose*` call in `computenetwork.dll`. In
//! windows-rs 0.62 these are plain `*const core::ffi::c_void` values (the
//! crate does not provide `HCN_NETWORK` / `HCN_ENDPOINT` / `HCN_NAMESPACE`
//! wrapper structs the way `HostComputeSystem` does for HCS); we alias them
//! locally so call sites can speak in terms of [`HcnNetworkHandle`] etc.
//! while the underlying representation stays a raw void pointer.
//!
//! The wrappers own the raw handle, implement `Drop` to release it, and are
//! `Send` (the handle value itself can cross threads) but not `Sync` — HCN
//! has the same thread-affinity caveats as HCS, so concurrent mutation
//! through the same handle is not supported. Wrap in `Arc<Mutex<...>>` at
//! the call site if shared access is needed.
//!
//! In windows-rs 0.62 every `HcnClose*` function is declared
//! `unsafe fn(...) -> windows_core::Result<()>` — the underlying C entry
//! point returns an `HRESULT` that windows-rs converts via `.ok()`. Drop
//! therefore matches on the result and logs a `tracing::warn!` on failure
//! rather than panicking.

use windows::Win32::System::HostComputeNetwork::{
    HcnCloseEndpoint, HcnCloseNamespace, HcnCloseNetwork,
};

/// Type alias for a raw HCN network handle.
///
/// windows-rs 0.62 does not provide a named wrapper struct for this value;
/// it is a bare `*const core::ffi::c_void` returned from
/// `HcnCreateNetwork` / `HcnOpenNetwork`.
pub type HcnNetworkHandle = *const core::ffi::c_void;

/// Type alias for a raw HCN endpoint handle.
pub type HcnEndpointHandle = *const core::ffi::c_void;

/// Type alias for a raw HCN namespace handle.
pub type HcnNamespaceHandle = *const core::ffi::c_void;

/// Owned HCN network handle. Released via `HcnCloseNetwork` on drop.
///
/// Closing does NOT delete the network — it only releases the caller's
/// reference. Use `HcnDeleteNetwork` (wrapped elsewhere) to actually remove
/// the network from the host.
#[derive(Debug)]
pub struct OwnedNetwork(HcnNetworkHandle);

// Safety: HCN handles are raw pointer values. They can legitimately move
// between threads, but concurrent mutation through the same handle is not
// supported by the HCN C API — hence `Send` but not `Sync`.
unsafe impl Send for OwnedNetwork {}

impl OwnedNetwork {
    /// Take ownership of a raw HCN network handle previously returned by
    /// `HcnCreateNetwork` or `HcnOpenNetwork`.
    ///
    /// # Safety
    ///
    /// The caller promises that `raw` is a valid, live handle and that
    /// ownership is being transferred exclusively to this wrapper.
    #[must_use]
    pub unsafe fn from_raw(raw: HcnNetworkHandle) -> Self {
        Self(raw)
    }

    /// Borrow the raw handle to pass into an HCN function.
    #[must_use]
    pub fn as_raw(&self) -> HcnNetworkHandle {
        self.0
    }

    /// Consume this wrapper and return the raw handle without closing it.
    ///
    /// The caller becomes responsible for eventually releasing the returned
    /// handle with `HcnCloseNetwork`.
    #[must_use]
    pub fn into_raw(self) -> HcnNetworkHandle {
        let raw = self.0;
        core::mem::forget(self);
        raw
    }
}

impl Drop for OwnedNetwork {
    fn drop(&mut self) {
        // Safety: we own this handle and are the only releaser. We never
        // panic from Drop; `HcnCloseNetwork` in windows-rs 0.62 returns
        // `windows_core::Result<()>`, so we log on failure and move on.
        if let Err(err) = unsafe { HcnCloseNetwork(self.0) } {
            tracing::warn!(
                handle = ?self.0,
                error = %err,
                "HcnCloseNetwork failed during Drop"
            );
        }
    }
}

/// Owned HCN endpoint handle. Released via `HcnCloseEndpoint` on drop.
///
/// Closing does NOT delete the endpoint — it only releases the caller's
/// reference. Use `HcnDeleteEndpoint` (wrapped elsewhere) to remove the
/// endpoint.
#[derive(Debug)]
pub struct OwnedEndpoint(HcnEndpointHandle);

unsafe impl Send for OwnedEndpoint {}

impl OwnedEndpoint {
    /// # Safety
    ///
    /// Caller promises exclusive ownership of a live HCN endpoint handle.
    #[must_use]
    pub unsafe fn from_raw(raw: HcnEndpointHandle) -> Self {
        Self(raw)
    }

    #[must_use]
    pub fn as_raw(&self) -> HcnEndpointHandle {
        self.0
    }

    #[must_use]
    pub fn into_raw(self) -> HcnEndpointHandle {
        let raw = self.0;
        core::mem::forget(self);
        raw
    }
}

impl Drop for OwnedEndpoint {
    fn drop(&mut self) {
        if let Err(err) = unsafe { HcnCloseEndpoint(self.0) } {
            tracing::warn!(
                handle = ?self.0,
                error = %err,
                "HcnCloseEndpoint failed during Drop"
            );
        }
    }
}

/// Owned HCN namespace handle. Released via `HcnCloseNamespace` on drop.
///
/// Closing does NOT delete the namespace — it only releases the caller's
/// reference. Use `HcnDeleteNamespace` (wrapped elsewhere) to remove the
/// namespace.
#[derive(Debug)]
pub struct OwnedNamespace(HcnNamespaceHandle);

unsafe impl Send for OwnedNamespace {}

impl OwnedNamespace {
    /// # Safety
    ///
    /// Caller promises exclusive ownership of a live HCN namespace handle.
    #[must_use]
    pub unsafe fn from_raw(raw: HcnNamespaceHandle) -> Self {
        Self(raw)
    }

    #[must_use]
    pub fn as_raw(&self) -> HcnNamespaceHandle {
        self.0
    }

    #[must_use]
    pub fn into_raw(self) -> HcnNamespaceHandle {
        let raw = self.0;
        core::mem::forget(self);
        raw
    }
}

impl Drop for OwnedNamespace {
    fn drop(&mut self) {
        if let Err(err) = unsafe { HcnCloseNamespace(self.0) } {
            tracing::warn!(
                handle = ?self.0,
                error = %err,
                "HcnCloseNamespace failed during Drop"
            );
        }
    }
}
