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
//! The wrappers own the raw handle and implement `Drop` to release it.
//! They are both `Send` and `Sync`. HCN handles are stable pointer-valued
//! identifiers into `computenetwork.dll`, and the HCN C API is documented
//! as thread-safe for API invocation: functions such as `HcnEnumerateNetworks`,
//! `HcnQueryNetworkProperties`, `HcnModifyNetwork`, `HcnQueryEndpointProperties`,
//! `HcnModifyEndpoint`, and the namespace equivalents are all callable from
//! arbitrary threads. Microsoft's hcsshim Go client (the reference HCN
//! consumer used by moby / containerd) invokes them concurrently from
//! multiple goroutines without external serialization, and the HCN
//! contract here matches HCS point-for-point.
//!
//! Caller-level invariants that genuinely need serialization (e.g. "only
//! one modify in flight per endpoint") are enforced at the wrapper-owner
//! layer via `&mut self`, `Mutex`, or `RwLock` as appropriate — not by
//! stripping `Sync` from the handle type. Removing `Sync` would only force
//! every caller of a thread-safe C API to take an unnecessary lock just to
//! share the handle across Tokio tasks (for example, when an
//! `EndpointAttachment` is stored inside an `Arc<RwLock<HashMap<_, _>>>`).
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

// Safety: HCN handles are stable pointer-valued identifiers into
// `computenetwork.dll` bookkeeping. The handle value itself can legitimately
// move between threads.
unsafe impl Send for OwnedNetwork {}

// Safety: `HcnQueryNetworkProperties`, `HcnModifyNetwork`,
// `HcnEnumerateNetworks`, and related HCN_NETWORK-taking entry points in
// `computenetwork.dll` are documented as thread-safe for API invocation.
// Microsoft's hcsshim Go client invokes them concurrently from multiple
// goroutines that share the same handle. Caller invariants that need
// serialization are enforced at the wrapper-owner layer via `&mut self` or
// `Mutex`, not by making the handle `!Sync`.
unsafe impl Sync for OwnedNetwork {}

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

// Safety: `HcnQueryEndpointProperties`, `HcnModifyEndpoint`, and the rest of
// the HCN endpoint entry points are documented as thread-safe for API
// invocation, matching the HCN network-handle contract. In ZLayer's runtime
// an `EndpointAttachment` is routinely stored inside an
// `Arc<RwLock<HashMap<_, ContainerEntry>>>` and accessed from multiple Tokio
// worker threads; Sync is required for that pattern, and the underlying C
// API supports it.
unsafe impl Sync for OwnedEndpoint {}

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

// Safety: `HcnQueryNamespaceProperties`, `HcnModifyNamespace`,
// `HcnEnumerateNamespaces`, and related HCN_NAMESPACE entry points in
// `computenetwork.dll` are documented as thread-safe for API invocation.
// Caller invariants that need serialization are enforced at the
// wrapper-owner layer via `&mut self` or `Mutex`, not by making the handle
// `!Sync`.
unsafe impl Sync for OwnedNamespace {}

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
