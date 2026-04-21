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
use crate::handle::OwnedProcess;
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
    /// The returned handle is closed on drop, but the process keeps running
    /// inside HCS. Await [`Self::properties`] (or `wait_exit`, once
    /// implemented) to observe termination.
    pub async fn create(system: HCS_SYSTEM, process_parameters_json: &str) -> HcsResult<Self> {
        let params_w = HSTRING::from(process_parameters_json);
        // `HcsCreateProcess` returns the new `HCS_PROCESS` synchronously via
        // its `Result`, alongside scheduling the async completion. We capture
        // the handle out of the kickoff closure so we can wrap it in an
        // `OwnedProcess` after `run_operation` finishes awaiting.
        let mut created: Option<HCS_PROCESS> = None;

        let _ = run_operation(|op| {
            // SAFETY: `system` is a live compute-system handle owned by the
            // caller; `params_w` lives for the duration of this call; `op`
            // is a fresh operation handle produced by `run_operation`; we
            // pass `None` for the optional SECURITY_DESCRIPTOR, so HCS uses
            // the caller's default token.
            match unsafe { HcsCreateProcess(system, &params_w, op, None) } {
                Ok(handle) => {
                    created = Some(handle);
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
        if raw.is_invalid() {
            return Err(HcsError::Other {
                hresult: 0,
                message: "HcsCreateProcess returned invalid handle".to_string(),
            });
        }

        Ok(Self {
            // SAFETY: `raw` came from `HcsCreateProcess` above and is not
            // owned by anyone else; `OwnedProcess` takes exclusive ownership.
            inner: unsafe { OwnedProcess::from_raw(raw) },
        })
    }

    /// Typed constructor — serializes [`ProcessParameters`] to JSON and
    /// delegates to [`Self::create`].
    pub async fn spawn(system: HCS_SYSTEM, params: &ProcessParameters) -> HcsResult<Self> {
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
    #[must_use]
    pub fn raw(&self) -> HCS_PROCESS {
        self.inner.as_raw()
    }

    /// Send a signal to the process.
    ///
    /// `options_json` is the schema-defined signal options document, for
    /// example `{"Signal":"SIGTERM"}` on a Linux guest. HCS decides what
    /// values are valid based on the compute system's OS.
    pub async fn signal(&self, options_json: &str) -> HcsResult<()> {
        let opts_w = HSTRING::from(options_json);
        let handle = self.inner.as_raw();
        run_operation(|op| {
            // SAFETY: `handle` is live (owned by `self.inner`), `opts_w`
            // outlives this call, and `op` is a fresh operation handle.
            match unsafe { HcsSignalProcess(handle, op, &opts_w) } {
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
        run_operation(|op| {
            // SAFETY: see `signal`.
            match unsafe { HcsTerminateProcess(handle, op, &opts_w) } {
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
        run_operation(|op| {
            // SAFETY: see `signal`.
            match unsafe { HcsModifyProcess(handle, op, &mod_w) } {
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
        run_operation(|op| {
            // SAFETY: see `signal`.
            match unsafe { HcsGetProcessProperties(handle, op, &query_w) } {
                Ok(()) => windows::core::HRESULT(0),
                Err(e) => e.code(),
            }
        })
        .await
    }
}
