//! Process lifecycle inside an HCS compute system.
//!
//! Wraps the `Hcs*Process*` entry points from `computecore.dll` (create,
//! open, signal, terminate, modify, and property query) behind RAII /
//! async-aware types. All mutating calls are routed through
//! [`crate::operation::run_operation`] so the caller awaits HCS's native
//! completion callback without blocking a Tokio worker.
//!
//! Strings crossing the FFI boundary go in via `&HSTRING` — windows-rs 0.62
//! implements `Param<PCWSTR>` for `&HSTRING`, so HCS sees a valid,
//! null-terminated UTF-16 buffer for the duration of the call.

#![allow(clippy::missing_errors_doc)]

use windows::core::HSTRING;
use windows::Win32::System::HostComputeSystem::{
    HcsCreateProcess, HcsGetProcessProperties, HcsModifyProcess, HcsOpenProcess, HcsSignalProcess,
    HcsTerminateProcess, HCS_PROCESS, HCS_SYSTEM,
};

use crate::error::{HcsError, HcsResult};
use crate::handle::{OwnedProcess, SendHandle};
use crate::operation::run_operation;
use crate::schema::ProcessParameters;

/// Handle to a process running inside an HCS compute system.
///
/// Dropping the handle releases our reference via `HcsCloseProcess`; it does
/// not signal or terminate the underlying process. Use [`Self::terminate`]
/// or [`Self::signal`] to actually stop the process.
#[derive(Debug)]
pub struct ComputeProcess {
    inner: OwnedProcess,
}

impl ComputeProcess {
    /// Create a new process in the given compute system.
    ///
    /// `process_parameters_json` must be a valid [`ProcessParameters`] JSON
    /// document; HCS will reject malformed or schema-mismatched payloads via
    /// [`HcsError::InvalidSchema`]. For the typed variant see
    /// [`Self::spawn`].
    ///
    /// `system` is accepted as a [`SendHandle<HCS_SYSTEM>`] so the
    /// returned future stays `Send` even when the caller holds it across
    /// task boundaries — raw `HCS_SYSTEM` is `!Send + !Sync` because it
    /// is `#[repr(transparent)]` over `*mut c_void`. Callers obtain one
    /// directly from [`crate::system::ComputeSystem::raw`], which returns
    /// `SendHandle<HCS_SYSTEM>` at the source.
    ///
    /// The returned handle is closed on drop, but the process keeps running
    /// inside HCS. Await [`Self::properties`] (or `wait_exit`, once
    /// implemented) to observe termination.
    pub async fn create(
        system: SendHandle<HCS_SYSTEM>,
        process_parameters_json: &str,
    ) -> HcsResult<Self> {
        let params_w = HSTRING::from(process_parameters_json);
        // `HcsCreateProcess` returns the new `HCS_PROCESS` synchronously via
        // its `Result`, alongside scheduling the async completion. We capture
        // the handle out of the kickoff closure so we can wrap it in an
        // `OwnedProcess` after `run_operation` finishes awaiting.
        //
        // Slot wrapped in `SendHandle` so it may cross the `run_operation`
        // await below while the generated future stays `Send`.
        let mut created: Option<SendHandle<HCS_PROCESS>> = None;

        // NOTE: use `*system` (Deref) rather than `system.0`. Rust 2021's
        // disjoint-field capture would otherwise capture only the inner
        // `HCS_SYSTEM` field (which is `!Send + !Sync`), defeating the
        // `SendHandle` wrapper; `*system` forces capture of the whole
        // wrapper via `&system.deref()`.
        //
        // `created` is captured by explicit `&mut` (bound in an outer scope)
        // so we can still observe the assignment from the outer fn after
        // the closure runs. The closure is non-`move`; the inner `let`
        // below establishes the `&mut Option<...>` binding to capture.
        let created_slot = &mut created;
        let _ = run_operation(|op| {
            // SAFETY: `*system` is a live compute-system handle owned by
            // the caller; `params_w` lives for the duration of this call;
            // `op` is a fresh operation handle produced by `run_operation`;
            // we pass `None` for the optional SECURITY_DESCRIPTOR, so HCS
            // uses the caller's default token.
            match unsafe { HcsCreateProcess(*system, &params_w, op, None) } {
                Ok(handle) => {
                    *created_slot = Some(SendHandle(handle));
                    windows::core::HRESULT(0)
                }
                Err(e) => e.code(),
            }
        })
        .await?;

        // If the kickoff succeeded we must have captured a handle; if the
        // device returned `Ok(())` but handed us an invalid value, surface
        // it as `Other` rather than constructing a bogus `OwnedProcess`.
        let raw = created.ok_or_else(|| HcsError::Other {
            hresult: 0,
            message: "HcsCreateProcess succeeded but produced no handle".to_string(),
        })?;
        if raw.0.is_invalid() {
            return Err(HcsError::Other {
                hresult: 0,
                message: "HcsCreateProcess returned invalid handle".to_string(),
            });
        }

        Ok(Self {
            // SAFETY: `raw.0` came from `HcsCreateProcess` above and is
            // not owned by anyone else; `OwnedProcess` takes exclusive
            // ownership.
            inner: unsafe { OwnedProcess::from_raw(raw.0) },
        })
    }

    /// Typed constructor — serializes [`ProcessParameters`] to JSON and
    /// delegates to [`Self::create`].
    pub async fn spawn(
        system: SendHandle<HCS_SYSTEM>,
        params: &ProcessParameters,
    ) -> HcsResult<Self> {
        let json = serde_json::to_string(params)?;
        Self::create(system, &json).await
    }

    /// Open an existing process by its `ProcessId` (for example the value
    /// read back from [`crate::schema::ProcessStatus::process_id`]).
    ///
    /// This is a synchronous call — unlike the other process entry points
    /// `HcsOpenProcess` has no `HCS_OPERATION` parameter and returns the
    /// handle directly.
    pub fn open(system: HCS_SYSTEM, process_id: u32, requested_access: u32) -> HcsResult<Self> {
        // SAFETY: `system` is a live compute-system handle owned by the
        // caller; the call returns the handle synchronously via its Result
        // and has no async completion side-effects.
        let raw = unsafe { HcsOpenProcess(system, process_id, requested_access) }.map_err(|e| {
            HcsError::from_hresult(e.code(), format!("HcsOpenProcess(pid={process_id})"))
        })?;
        if raw.is_invalid() {
            return Err(HcsError::Other {
                hresult: 0,
                message: "HcsOpenProcess returned invalid handle".to_string(),
            });
        }
        Ok(Self {
            // SAFETY: `raw` came from `HcsOpenProcess` above and ownership
            // is transferred exclusively to the new `OwnedProcess`.
            inner: unsafe { OwnedProcess::from_raw(raw) },
        })
    }

    /// Raw handle for low-level callers that need to invoke HCS entry
    /// points this wrapper does not expose.
    ///
    /// Returns a [`SendHandle<HCS_PROCESS>`] rather than the bare raw
    /// handle so the wrapper's `Send + Sync` guarantees propagate to
    /// callers that hold the value across an `.await` — raw `HCS_PROCESS`
    /// is transparent over `*mut c_void` and is therefore `!Send + !Sync`.
    /// FFI call sites dereference with `*handle` (via [`std::ops::Deref`])
    /// when passing the value to a C entry point.
    #[must_use]
    pub fn raw(&self) -> SendHandle<HCS_PROCESS> {
        self.inner.as_raw()
    }

    /// Send a signal to the process.
    ///
    /// `options_json` is the schema-defined signal options document, for
    /// example `{"Signal":"SIGTERM"}` on a Linux guest. HCS decides what
    /// values are valid based on the compute system's OS.
    pub async fn signal(&self, options_json: &str) -> HcsResult<()> {
        let opts_w = HSTRING::from(options_json);
        // Wrap the raw process handle so the enclosing future stays `Send`
        // across the `.await` below. See `handle::SendHandle`.
        //
        // NOTE: Inside the closure we use `*handle` (not `handle.0`) so
        // Rust 2021's disjoint-field capture rules do not strip the
        // `SendHandle` wrapper down to the `!Send + !Sync` inner field.
        // `*handle` goes through `Deref`, capturing the whole wrapper.
        let handle = self.inner.as_raw();
        run_operation(move |op| {
            // SAFETY: `*handle` is live (owned by `self.inner`), `opts_w`
            // outlives this call, and `op` is a fresh operation handle.
            match unsafe { HcsSignalProcess(*handle, op, &opts_w) } {
                Ok(()) => windows::core::HRESULT(0),
                Err(e) => e.code(),
            }
        })
        .await?;
        Ok(())
    }

    /// Force-terminate the process. This is the HCS equivalent of
    /// `TerminateProcess` and is not graceful; prefer [`Self::signal`] when
    /// the guest supports the requested signal.
    pub async fn terminate(&self, options_json: &str) -> HcsResult<()> {
        let opts_w = HSTRING::from(options_json);
        let handle = self.inner.as_raw();
        run_operation(move |op| {
            // SAFETY: see `signal`.
            match unsafe { HcsTerminateProcess(*handle, op, &opts_w) } {
                Ok(()) => windows::core::HRESULT(0),
                Err(e) => e.code(),
            }
        })
        .await?;
        Ok(())
    }

    /// Modify live process settings. Typical payloads are console-resize
    /// requests (see [`Self::resize_console`] for a convenience wrapper)
    /// or I/O-pipe reconfiguration documents.
    pub async fn modify(&self, modification_json: &str) -> HcsResult<()> {
        let mod_w = HSTRING::from(modification_json);
        let handle = self.inner.as_raw();
        run_operation(move |op| {
            // SAFETY: see `signal`.
            match unsafe { HcsModifyProcess(*handle, op, &mod_w) } {
                Ok(()) => windows::core::HRESULT(0),
                Err(e) => e.code(),
            }
        })
        .await?;
        Ok(())
    }

    /// Convenience: resize the attached console to `cols` columns by `rows`
    /// rows. Equivalent to calling [`Self::modify`] with a
    /// `{"ConsoleSize":{"Width":cols,"Height":rows}}` document.
    pub async fn resize_console(&self, cols: u16, rows: u16) -> HcsResult<()> {
        let json = serde_json::to_string(&serde_json::json!({
            "ConsoleSize": {
                "Width": cols,
                "Height": rows,
            }
        }))?;
        self.modify(&json).await
    }

    /// Read process properties. Returns the raw JSON document HCS emits;
    /// callers parse it via [`crate::schema::ProcessStatus`] (or a
    /// property-specific type) depending on `property_query_json`.
    pub async fn properties(&self, property_query_json: &str) -> HcsResult<String> {
        let query_w = HSTRING::from(property_query_json);
        let handle = self.inner.as_raw();
        run_operation(move |op| {
            // SAFETY: see `signal`.
            match unsafe { HcsGetProcessProperties(*handle, op, &query_w) } {
                Ok(()) => windows::core::HRESULT(0),
                Err(e) => e.code(),
            }
        })
        .await
    }
}
