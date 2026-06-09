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
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::HostComputeSystem::{
    HcsCreateOperation, HcsCreateProcess, HcsGetProcessProperties, HcsModifyProcess,
    HcsOpenProcess, HcsSignalProcess, HcsTerminateProcess, HcsWaitForOperationResultAndProcessInfo,
    HCS_PROCESS, HCS_PROCESS_INFORMATION, HCS_SYSTEM,
};

use crate::error::{HcsError, HcsResult};
use crate::handle::{OwnedProcess, SendHandle};
use crate::operation::run_operation;
use crate::schema::ProcessParameters;

/// The captured stdout/stderr of a process created via
/// [`ComputeProcess::create_capturing`].
///
/// The HANDLEs are real Win32 pipe read-ends in *this* process, handed back
/// by HCS in the `HCS_PROCESS_INFORMATION` of the create operation. They are
/// drained synchronously by [`drain_pipe`] and closed afterwards.
#[derive(Debug)]
pub struct CapturedProcess {
    /// The created process handle.
    pub process: ComputeProcess,
    /// Read-end of the process's stdout pipe, or `None` if not requested.
    stdout: Option<HANDLE>,
    /// Read-end of the process's stderr pipe, or `None` if not requested.
    stderr: Option<HANDLE>,
}

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

    /// Create a process AND capture the stdout/stderr pipe read-ends HCS
    /// returns in the create operation's `HCS_PROCESS_INFORMATION`.
    ///
    /// Unlike [`Self::create`] (which discards the operation result and never
    /// requests process info), this drives the create operation to completion
    /// with [`HcsWaitForOperationResultAndProcessInfo`], which fills an
    /// `HCS_PROCESS_INFORMATION` whose `StdOutput`/`StdError` fields are real
    /// Win32 pipe HANDLEs in *this* process. The returned [`CapturedProcess`]
    /// owns those handles; call [`CapturedProcess::drain`] to read them to EOF
    /// (after the process exits) and close them.
    ///
    /// `params` should set `create_std_out_pipe` / `create_std_err_pipe` so
    /// HCS actually creates the pipes; otherwise the corresponding handle
    /// comes back invalid and [`CapturedProcess::drain`] yields an empty
    /// string for that stream.
    ///
    /// The whole call runs on a blocking thread (it uses the synchronous
    /// `HcsWaitForOperationResultAndProcessInfo`, which blocks until the
    /// create completes); callers should invoke it from
    /// `tokio::task::spawn_blocking`.
    pub fn create_capturing_blocking(
        system: SendHandle<HCS_SYSTEM>,
        params: &ProcessParameters,
    ) -> HcsResult<CapturedProcess> {
        let json = serde_json::to_string(params)?;
        let params_w = HSTRING::from(json.as_str());

        // SAFETY: a fresh operation with no completion callback — we drive it
        // synchronously below via `HcsWaitForOperationResultAndProcessInfo`.
        let op = unsafe { HcsCreateOperation(None, None) };
        if op.is_invalid() {
            return Err(HcsError::Other {
                hresult: 0,
                message: "HcsCreateOperation returned an invalid handle".to_string(),
            });
        }
        // Ensure the operation is closed on every exit path.
        // SAFETY: `op` was just produced by `HcsCreateOperation`.
        let owned_op = unsafe { crate::handle::OwnedOperation::from_raw(op) };

        // Kick off the create against this operation.
        // SAFETY: `*system` is a live compute-system handle; `params_w` lives
        // for the call; `op` is fresh; `None` security descriptor → caller's
        // default token.
        let created =
            match unsafe { HcsCreateProcess(*system, &params_w, *owned_op.as_raw(), None) } {
                Ok(handle) => handle,
                Err(e) => return Err(HcsError::from_hresult(e.code(), "HcsCreateProcess")),
            };
        if created.is_invalid() {
            return Err(HcsError::Other {
                hresult: 0,
                message: "HcsCreateProcess returned invalid handle".to_string(),
            });
        }

        // Wait for the create to complete and pull back the process info
        // (which carries the StdOutput/StdError pipe handles). A generous
        // timeout — the create itself is fast; the process then runs.
        let mut info = HCS_PROCESS_INFORMATION::default();
        let mut result_doc = windows::core::PWSTR::null();
        // SAFETY: `op` is live; `info`/`result_doc` are valid out-params; HCS
        // fills them. We pass `INFINITE`-equivalent large timeout; the create
        // completion is prompt.
        let wait = unsafe {
            HcsWaitForOperationResultAndProcessInfo(
                *owned_op.as_raw(),
                30_000,
                Some(&raw mut info),
                Some(&raw mut result_doc),
            )
        };
        if let Err(e) = wait {
            // The process handle may still be valid; wrap it so it is closed.
            // SAFETY: `created` came from `HcsCreateProcess` above.
            drop(unsafe { OwnedProcess::from_raw(created) });
            return Err(HcsError::from_hresult(
                e.code(),
                "HcsWaitForOperationResultAndProcessInfo",
            ));
        }

        let stdout = (!info.StdOutput.is_invalid()).then_some(info.StdOutput);
        let stderr = (!info.StdError.is_invalid()).then_some(info.StdError);
        // We do not use StdInput here; close it immediately if present so the
        // child does not block waiting on an input pipe we never write.
        if !info.StdInput.is_invalid() {
            // SAFETY: `StdInput` is a Win32 handle owned by us per the HCS
            // process-info contract; closing the write-end signals EOF.
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(info.StdInput);
            }
        }

        Ok(CapturedProcess {
            process: Self {
                // SAFETY: `created` came from `HcsCreateProcess` and ownership
                // transfers exclusively to the new `OwnedProcess`.
                inner: unsafe { OwnedProcess::from_raw(created) },
            },
            stdout,
            stderr,
        })
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

impl CapturedProcess {
    /// Borrow the underlying [`ComputeProcess`] (e.g. to poll its exit code).
    #[must_use]
    pub fn process(&self) -> &ComputeProcess {
        &self.process
    }

    /// Read both captured pipes to EOF and return `(stdout, stderr)` as
    /// lossily-decoded UTF-8. Consumes the capture and closes both pipe
    /// handles. Blocks on the synchronous `ReadFile`; call from
    /// `tokio::task::spawn_blocking`.
    ///
    /// EOF arrives when the child closes its write-end (i.e. exits). Callers
    /// that want the buffered output should ensure the process has exited (or
    /// will exit) before draining, otherwise the read blocks until it does.
    #[must_use]
    pub fn drain(self) -> (String, String) {
        let out = self.stdout.map(drain_pipe).unwrap_or_default();
        let err = self.stderr.map(drain_pipe).unwrap_or_default();
        (
            String::from_utf8_lossy(&out).into_owned(),
            String::from_utf8_lossy(&err).into_owned(),
        )
    }

    /// Drain both pipes (blocking until the child closes them / exits) and
    /// return the drained `(stdout, stderr)` together with the still-live
    /// [`ComputeProcess`] handle, which the caller can hand back to an async
    /// context to read the authoritative exit code via
    /// [`ComputeProcess::properties`].
    ///
    /// Splitting it this way lets the heavy blocking pipe reads run inside
    /// `tokio::task::spawn_blocking` (the pipe `HANDLE`s are `!Send`, so they
    /// must never cross an `.await`), while the `Send` `ComputeProcess` is
    /// returned for the exit-code poll. Blocks; call from `spawn_blocking`.
    #[must_use]
    pub fn drain_with_process(self) -> (ComputeProcess, String, String) {
        let out = self.stdout.map(drain_pipe).unwrap_or_default();
        let err = self.stderr.map(drain_pipe).unwrap_or_default();
        (
            self.process,
            String::from_utf8_lossy(&out).into_owned(),
            String::from_utf8_lossy(&err).into_owned(),
        )
    }
}

/// Synchronously read a Win32 pipe handle to EOF, then close it.
///
/// EOF (`ReadFile` returning `ERROR_BROKEN_PIPE` / 0 bytes) terminates the
/// loop. The handle is always closed before returning, even on error.
fn drain_pipe(handle: HANDLE) -> Vec<u8> {
    use windows::Win32::Foundation::{CloseHandle, ERROR_BROKEN_PIPE};
    use windows::Win32::Storage::FileSystem::ReadFile;

    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let mut read: u32 = 0;
        // SAFETY: `handle` is a live pipe read-end owned by us; `buf` is a
        // valid writable slice; `read` is a valid out-param.
        let res = unsafe { ReadFile(handle, Some(&mut buf), Some(&raw mut read), None) };
        match res {
            Ok(()) => {
                if read == 0 {
                    break; // clean EOF
                }
                out.extend_from_slice(&buf[..read as usize]);
            }
            Err(e) => {
                // A broken pipe is the normal EOF signal for an anonymous pipe
                // once the writer (the child) has exited and all buffered
                // bytes are consumed. Anything else is unexpected; log it but
                // still return what we captured.
                if e.code() != ERROR_BROKEN_PIPE.to_hresult() {
                    tracing::debug!(error = %e, "drain_pipe: ReadFile ended on a non-EOF error");
                }
                break;
            }
        }
    }
    // SAFETY: `handle` is ours; closing the read-end after draining.
    unsafe {
        let _ = CloseHandle(handle);
    }
    out
}
