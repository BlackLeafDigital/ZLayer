//! RAII wrappers for raw HCS handles.
//!
//! HCS exposes three handle types — `HCS_OPERATION`, `HCS_SYSTEM`, and
//! `HCS_PROCESS` — each of which must be released via a matching `HcsClose*`
//! call. The wrappers in this module own the raw handle and implement `Drop`
//! to release it.
//!
//! These wrappers are both `Send` and `Sync`. HCS handles are stable
//! identifier values (raw pointers into `computecore.dll` bookkeeping) and
//! the HCS C API is documented as thread-safe for API invocation: functions
//! such as `HcsTerminateComputeSystem`, `HcsGetComputeSystemProperties`,
//! `HcsModifyComputeSystem`, and the operation-completion callbacks are all
//! callable from arbitrary threads, and Microsoft's own hcsshim (the Go
//! reference client used by moby / containerd) invokes them concurrently
//! from multiple goroutines without external serialization.
//!
//! Wrapper-level invariants (for example: "do not terminate twice", or
//! "only one writer may modify a compute system at a time") are enforced at
//! the layer that owns the wrapper — via `&mut self`, `Mutex`, or `RwLock`
//! as appropriate for the caller's logical data model — rather than by
//! stripping `Sync` from the handle type itself. Removing `Sync` would only
//! force every caller of a thread-safe C API to take an unnecessary lock
//! just to share the handle across tasks (for example, when a container
//! entry is stored in an `Arc<RwLock<HashMap<String, ContainerEntry>>>` and
//! accessed from Tokio worker threads).

use windows::Win32::System::HostComputeSystem::{
    HcsCloseComputeSystem, HcsCloseOperation, HcsCloseProcess, HCS_OPERATION, HCS_PROCESS,
    HCS_SYSTEM,
};

/// Thread-safe transparent wrapper around any `Copy` FFI handle.
///
/// The upstream `windows` crate defines `HCS_SYSTEM`, `HCS_OPERATION`, and
/// `HCS_PROCESS` as `#[repr(transparent)]` tuple structs over `*mut c_void`,
/// which makes them `!Send + !Sync`. That's correct for arbitrary
/// `*mut c_void` but wrong for HCS handles specifically: the HCS C API is
/// documented as thread-safe for API invocation (see
/// `HcsTerminateComputeSystem`, `HcsGetComputeSystemProperties`,
/// `HcsCreateOperation`, and the process/operation entry points in
/// `computecore.dll`), and Microsoft's hcsshim Go client — the reference
/// implementation used by moby / containerd — routinely passes these
/// handles between goroutines without external serialization.
///
/// The orphan rule prevents us from `impl`-ing `Send`/`Sync` directly on the
/// upstream raw types. The canonical workaround, used by `hcsshim-rs` and
/// other Windows-Rust FFI crates, is a crate-local `#[repr(transparent)]`
/// newtype carrying the `Send + Sync` assertions. `SendHandle<T>` is that
/// newtype. We use it at every site where a raw HCS_* handle must cross an
/// `.await` boundary inside an `async fn` — the `Owned*` RAII wrappers
/// above already carry `Send + Sync`, but async state machines also keep
/// the un-wrapped raw values live across suspend points (locals, option
/// slots, fn params) and the compiler checks those too.
///
/// # Safety
///
/// The same argument as the `Owned*` wrappers applies: HCS handles are
/// stable pointer-valued identifiers into `computecore.dll` bookkeeping,
/// the taking APIs are documented thread-safe, and caller-level
/// invariants (for example "do not terminate twice") are enforced at the
/// wrapper-owner layer via `&mut self` / `Mutex` / `RwLock`, not by
/// stripping `Sync` from the handle type itself.
#[repr(transparent)]
#[derive(Copy, Clone, Debug)]
pub struct SendHandle<T: Copy>(pub T);

// Safety: see the type-level doc comment. HCS handles are documented
// thread-safe for API invocation; Microsoft's hcsshim Go client relies on
// this same property to pass handles across goroutines.
unsafe impl<T: Copy> Send for SendHandle<T> {}
// Safety: same rationale as Send — HCS taking APIs are thread-safe and
// hcsshim treats these handles as shareable across concurrent workers.
unsafe impl<T: Copy> Sync for SendHandle<T> {}

impl<T: Copy> std::ops::Deref for SendHandle<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

/// Owned HCS operation handle. Released via `HcsCloseOperation` on drop.
#[derive(Debug)]
pub struct OwnedOperation(HCS_OPERATION);

// Safety: HCS handles are stable pointer-valued identifiers into
// `computecore.dll`. The handle value itself can legitimately move between
// threads.
unsafe impl Send for OwnedOperation {}

// Safety: the HCS C API is documented as thread-safe for API invocation
// (see `HcsCreateOperation`, `HcsWaitForOperationResult`,
// `HcsGetOperationResult`, etc. in the HostComputeSystem docs). Microsoft's
// hcsshim Go client calls these functions from multiple goroutines
// concurrently without serialization. Any caller-level invariants (e.g.
// "only one waiter per operation") are enforced at the wrapper owner's
// level via `&mut self` or a `Mutex`, not by removing `Sync`.
unsafe impl Sync for OwnedOperation {}

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
    ///
    /// Returns a [`SendHandle<HCS_OPERATION>`] rather than the bare raw
    /// handle so the wrapper's `Send + Sync` guarantees propagate to
    /// callers that hold the value across an `.await`. FFI call sites
    /// dereference with `*handle` (via [`std::ops::Deref`]) when passing
    /// the value to a C entry point.
    #[must_use]
    pub fn as_raw(&self) -> SendHandle<HCS_OPERATION> {
        SendHandle(self.0)
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

// Safety: `HcsGetComputeSystemProperties`, `HcsTerminateComputeSystem`,
// `HcsShutdownComputeSystem`, `HcsModifyComputeSystem`, and the rest of the
// HCS_SYSTEM-taking entry points in `computecore.dll` are documented as
// thread-safe. Microsoft's hcsshim Go client calls them concurrently from
// multiple goroutines that all share the same handle. Caller invariants
// that truly require serialization (e.g. do-not-terminate-twice) are
// enforced at the wrapper-owner layer via `&mut self` or `Mutex`, not by
// making the handle `!Sync`.
unsafe impl Sync for OwnedSystem {}

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

    /// Borrow the raw handle to pass into an HCS function.
    ///
    /// Returns a [`SendHandle<HCS_SYSTEM>`] rather than the bare raw
    /// handle so the wrapper's `Send + Sync` guarantees propagate to
    /// callers that hold the value across an `.await`. FFI call sites
    /// dereference with `*handle` (via [`std::ops::Deref`]) when passing
    /// the value to a C entry point.
    #[must_use]
    pub fn as_raw(&self) -> SendHandle<HCS_SYSTEM> {
        SendHandle(self.0)
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

// Safety: `HcsSignalProcess`, `HcsGetProcessInfo`, `HcsGetProcessProperties`,
// `HcsModifyProcess`, and the process I/O stream entry points are all
// documented as thread-safe. In practice I/O-reader tasks and the
// signal/wait task typically run on different Tokio worker threads and
// share the same `OwnedProcess` through an `Arc` — Sync is required for
// that pattern, and the underlying C API supports it.
unsafe impl Sync for OwnedProcess {}

impl OwnedProcess {
    /// # Safety
    ///
    /// Caller promises exclusive ownership of a live process handle.
    #[must_use]
    pub unsafe fn from_raw(raw: HCS_PROCESS) -> Self {
        Self(raw)
    }

    /// Borrow the raw handle to pass into an HCS function.
    ///
    /// Returns a [`SendHandle<HCS_PROCESS>`] rather than the bare raw
    /// handle so the wrapper's `Send + Sync` guarantees propagate to
    /// callers that hold the value across an `.await`. FFI call sites
    /// dereference with `*handle` (via [`std::ops::Deref`]) when passing
    /// the value to a C entry point.
    #[must_use]
    pub fn as_raw(&self) -> SendHandle<HCS_PROCESS> {
        SendHandle(self.0)
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
