//! RAII wrappers for raw HCS handles.
//!
//! HCS exposes three handle types — `HCS_OPERATION`, `HCS_SYSTEM`, and
//! `HCS_PROCESS` — each of which must be released via a matching `HcsClose*`
//! call. The wrappers in this module own the raw handle, implement `Drop`
//! to release it, and mark themselves `Send` (the raw handle value itself
//! can cross threads) but not `Sync` (HCS's callback semantics make shared
//! mutation unsafe; use `Arc<Mutex<...>>` at the call site if needed).

use windows::Win32::System::HostComputeSystem::{
    HcsCloseComputeSystem, HcsCloseOperation, HcsCloseProcess, HCS_OPERATION, HCS_PROCESS,
    HCS_SYSTEM,
};

/// Owned HCS operation handle. Released via `HcsCloseOperation` on drop.
#[derive(Debug)]
pub struct OwnedOperation(HCS_OPERATION);

// Safety: HCS handles are raw pointer values; they can legitimately move
// between threads. They are NOT `Sync` — concurrent mutation through the
// same handle is not supported by the HCS C API.
unsafe impl Send for OwnedOperation {}

impl OwnedOperation {
    /// Take ownership of a raw HCS operation handle previously returned
    /// by [`HcsCreateOperation`].
    ///
    /// # Safety
    ///
    /// The caller promises that `raw` is a valid, live handle and that
    /// ownership is being transferred exclusively to this wrapper.
    #[must_use]
    pub unsafe fn from_raw(raw: HCS_OPERATION) -> Self {
        Self(raw)
    }

    /// Borrow the raw handle to pass into an HCS function.
    #[must_use]
    pub fn as_raw(&self) -> HCS_OPERATION {
        self.0
    }

    /// Consume this wrapper and return the raw handle without closing it.
    /// Use this when transferring ownership to another owner (e.g. a
    /// helper that keeps the operation alive until a callback fires).
    ///
    /// The caller is responsible for eventually closing the returned
    /// handle with `HcsCloseOperation`.
    #[must_use]
    pub fn into_raw(self) -> HCS_OPERATION {
        let raw = self.0;
        core::mem::forget(self);
        raw
    }
}

impl Drop for OwnedOperation {
    fn drop(&mut self) {
        // Safety: we own this handle and are the only releaser.
        // In windows-rs 0.62 `HcsCloseOperation` is declared `-> ()` (it
        // wraps a C `void`-returning entry point in `computecore.dll`), so
        // there is no fallible return to inspect. We still wrap the call in
        // `unsafe` because it is an FFI call, and we never panic from Drop.
        unsafe {
            HcsCloseOperation(self.0);
        }
    }
}

/// Owned HCS compute-system handle. Released via `HcsCloseComputeSystem`
/// on drop. Closing does NOT terminate the system — it only releases the
/// caller's reference. Use `HcsTerminateComputeSystem` (wrapped elsewhere)
/// to actually stop a running system.
#[derive(Debug)]
pub struct OwnedSystem(HCS_SYSTEM);

unsafe impl Send for OwnedSystem {}

impl OwnedSystem {
    /// Take ownership of a raw HCS system handle.
    ///
    /// # Safety
    ///
    /// Caller promises the handle is live and that ownership is exclusive.
    #[must_use]
    pub unsafe fn from_raw(raw: HCS_SYSTEM) -> Self {
        Self(raw)
    }

    #[must_use]
    pub fn as_raw(&self) -> HCS_SYSTEM {
        self.0
    }

    #[must_use]
    pub fn into_raw(self) -> HCS_SYSTEM {
        let raw = self.0;
        core::mem::forget(self);
        raw
    }
}

impl Drop for OwnedSystem {
    fn drop(&mut self) {
        // In windows-rs 0.62 `HcsCloseComputeSystem` is declared `-> ()`
        // (the underlying C entry point returns `void`), so there is no
        // fallible return to inspect. Never panic from Drop.
        unsafe {
            HcsCloseComputeSystem(self.0);
        }
    }
}

/// Owned HCS process handle. Released via `HcsCloseProcess` on drop.
/// Closing does NOT signal or terminate the process.
#[derive(Debug)]
pub struct OwnedProcess(HCS_PROCESS);

unsafe impl Send for OwnedProcess {}

impl OwnedProcess {
    /// # Safety
    ///
    /// Caller promises exclusive ownership of a live process handle.
    #[must_use]
    pub unsafe fn from_raw(raw: HCS_PROCESS) -> Self {
        Self(raw)
    }

    #[must_use]
    pub fn as_raw(&self) -> HCS_PROCESS {
        self.0
    }

    #[must_use]
    pub fn into_raw(self) -> HCS_PROCESS {
        let raw = self.0;
        core::mem::forget(self);
        raw
    }
}

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        // In windows-rs 0.62 `HcsCloseProcess` is declared `-> ()` (the
        // underlying C entry point returns `void`), so there is no fallible
        // return to inspect. Never panic from Drop.
        unsafe {
            HcsCloseProcess(self.0);
        }
    }
}
