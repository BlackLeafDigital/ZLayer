//! `ComputeSystem` lifecycle — create, open, start, shutdown, terminate,
//! pause/resume/save, query, modify.
//!
//! Every async method drives the HCS native async-operation model through
//! [`crate::operation::run_operation`], which handles the creation of the
//! backing `HCS_OPERATION`, registration of the completion trampoline, and
//! translation of the returned JSON / classified error back into an
//! ordinary `HcsResult`.
//!
//! # Handle ownership
//!
//! A `ComputeSystem` owns an [`OwnedSystem`] handle. Dropping the
//! `ComputeSystem` closes the caller's reference to the system via
//! `HcsCloseComputeSystem` — it does **not** terminate the system itself.
//! Use [`ComputeSystem::terminate`] to forcibly stop the system, or
//! [`ComputeSystem::shutdown`] for a graceful stop.

#![allow(clippy::missing_errors_doc)] // every method returns HcsResult; clippy pedantic complains otherwise.

use windows::core::{HRESULT, HSTRING};
use windows::Win32::System::HostComputeSystem::{
    HcsCreateComputeSystem, HcsGetComputeSystemProperties, HcsModifyComputeSystem,
    HcsOpenComputeSystem, HcsPauseComputeSystem, HcsResumeComputeSystem, HcsSaveComputeSystem,
    HcsShutDownComputeSystem, HcsStartComputeSystem, HcsTerminateComputeSystem, HCS_SYSTEM,
};

use crate::error::{HcsError, HcsResult};
use crate::handle::{OwnedSystem, SendHandle};
use crate::operation::run_operation;

/// A single HRESULT success value used to turn `windows_core::Result<()>`
/// into the `HRESULT` that `run_operation`'s closure must return.
const HR_OK: HRESULT = HRESULT(0);

/// Convert the `windows_core::Result<()>` returned by the windows-rs 0.62
/// HCS bindings into the `HRESULT` shape that [`run_operation`] expects in
/// its closure.
#[inline]
fn to_hresult(result: windows::core::Result<()>) -> HRESULT {
    match result {
        Ok(()) => HR_OK,
        Err(e) => e.code(),
    }
}

/// Owned compute system. All lifecycle calls take `&self`; the enclosed
/// [`OwnedSystem`] handle is closed on `Drop`.
#[derive(Debug)]
pub struct ComputeSystem {
    inner: OwnedSystem,
}

impl ComputeSystem {
    /// Create a new compute system.
    ///
    /// `id` is the caller-chosen container/VM identifier (often a GUID);
    /// `configuration_json` is the schema v2 compute-system document
    /// serialised with [`crate::schema::ComputeSystem`].
    ///
    /// The returned future resolves once HCS reports the system has
    /// transitioned to the `Created` state. The underlying handle is
    /// closed when the returned [`ComputeSystem`] is dropped, but the
    /// system itself is not terminated — call [`Self::shutdown`] or
    /// [`Self::terminate`] to stop it.
    pub async fn create(id: &str, configuration_json: &str) -> HcsResult<Self> {
        let id_w = HSTRING::from(id);
        let cfg_w = HSTRING::from(configuration_json);

        // HCS returns the compute-system handle synchronously on a
        // successful kickoff; the operation completion signals that the
        // system has finished transitioning to `Created`. We therefore
        // capture the handle in the closure via a mutable slot and then
        // await the operation for the state transition.
        //
        // The slot is wrapped in `SendHandle` so this local may cross the
        // `run_operation` `.await` below while keeping the enclosing
        // future `Send` — raw `HCS_SYSTEM` is `!Send + !Sync` because it
        // is transparent over `*mut c_void`.
        let mut handle_slot: Option<SendHandle<HCS_SYSTEM>> = None;

        let result = {
            let handle_slot = &mut handle_slot;
            run_operation(move |op| {
                // SAFETY: `id_w` and `cfg_w` live for the whole closure
                // invocation; `op` is a live HCS operation handle owned
                // by `run_operation`; security descriptor is None
                // (default — caller's token ACL).
                let res = unsafe { HcsCreateComputeSystem(&id_w, &cfg_w, op, None) };
                match res {
                    Ok(sys) => {
                        *handle_slot = Some(SendHandle(sys));
                        HR_OK
                    }
                    Err(e) => e.code(),
                }
            })
            .await
        };

        // Propagate a kickoff / completion failure before attempting to
        // wrap the handle (there is nothing to wrap in that case).
        let _json = result?;

        let Some(raw) = handle_slot else {
            return Err(HcsError::Other {
                hresult: 0,
                message: "HcsCreateComputeSystem returned success without a handle".to_string(),
            });
        };
        if raw.0.is_invalid() {
            return Err(HcsError::Other {
                hresult: 0,
                message: "HcsCreateComputeSystem returned an invalid handle".to_string(),
            });
        }

        // SAFETY: `raw.0` was just produced by a successful
        // `HcsCreateComputeSystem` call and has not been handed to any
        // other owner.
        let inner = unsafe { OwnedSystem::from_raw(raw.0) };
        Ok(Self { inner })
    }

    /// Open an existing compute system by id. This call is synchronous —
    /// there is no backing HCS operation.
    ///
    /// `requested_access` is a standard Windows access mask; `0` asks for
    /// the default access granted to the caller's token.
    pub fn open(id: &str, requested_access: u32) -> HcsResult<Self> {
        let id_w = HSTRING::from(id);
        // SAFETY: FFI call; `id_w` is a live HSTRING for the duration of
        // the call and `requested_access` is a plain integer.
        let raw = unsafe { HcsOpenComputeSystem(&id_w, requested_access) }
            .map_err(|e| HcsError::from_hresult(e.code(), format!("HcsOpenComputeSystem({id})")))?;

        if raw.is_invalid() {
            return Err(HcsError::Other {
                hresult: 0,
                message: "HcsOpenComputeSystem returned an invalid handle".to_string(),
            });
        }
        // SAFETY: `raw` came from a successful HCS open call.
        let inner = unsafe { OwnedSystem::from_raw(raw) };
        Ok(Self { inner })
    }

    /// Borrow the underlying raw HCS handle. Useful for low-level callers
    /// (e.g. event-callback registration) that need the bare pointer; the
    /// handle remains owned by this [`ComputeSystem`] and must not be
    /// closed by the caller.
    ///
    /// Returns a [`SendHandle<HCS_SYSTEM>`] rather than the bare raw
    /// handle so the wrapper's `Send + Sync` guarantees propagate to
    /// callers that hold the value across an `.await` — raw `HCS_SYSTEM`
    /// is transparent over `*mut c_void` and is therefore `!Send + !Sync`.
    /// FFI call sites dereference with `*handle` (via [`std::ops::Deref`])
    /// when passing the value to a C entry point.
    #[must_use]
    pub fn raw(&self) -> SendHandle<HCS_SYSTEM> {
        self.inner.as_raw()
    }

    /// Start the compute system. `options_json` is an optional JSON
    /// options document; pass `""` to use defaults.
    pub async fn start(&self, options_json: &str) -> HcsResult<()> {
        let opts_w = HSTRING::from(options_json);
        // Wrap the raw handle so the enclosing future stays `Send` across
        // the `.await` below. See `handle::SendHandle` for the rationale.
        //
        // NOTE: Inside the closure we MUST dereference through `*handle`
        // (not `handle.0`). Rust 2021's disjoint-field capture rules cause
        // `handle.0` in a `move` closure to capture ONLY the inner
        // `HCS_SYSTEM` field (which is `!Send + !Sync`), defeating the
        // `SendHandle` wrapper. `*handle` goes through `Deref`, which
        // requires `&handle` and therefore captures the whole wrapper.
        let handle = self.inner.as_raw();
        run_operation(move |op| {
            // SAFETY: `*handle` is live for the duration of `self`, which
            // outlives this await (borrow check); `opts_w` lives for the
            // closure invocation; `op` is owned by `run_operation`.
            to_hresult(unsafe { HcsStartComputeSystem(*handle, op, &opts_w) })
        })
        .await?;
        Ok(())
    }

    /// Shut down the compute system gracefully. `options_json` may carry
    /// a shutdown timeout per the HCS schema.
    pub async fn shutdown(&self, options_json: &str) -> HcsResult<()> {
        let opts_w = HSTRING::from(options_json);
        let handle = self.inner.as_raw();
        run_operation(move |op| {
            // SAFETY: see `start` — identical lifetime argument.
            to_hresult(unsafe { HcsShutDownComputeSystem(*handle, op, &opts_w) })
        })
        .await?;
        Ok(())
    }

    /// Forcefully terminate the compute system without a grace period.
    pub async fn terminate(&self, options_json: &str) -> HcsResult<()> {
        let opts_w = HSTRING::from(options_json);
        let handle = self.inner.as_raw();
        run_operation(move |op| {
            // SAFETY: see `start`.
            to_hresult(unsafe { HcsTerminateComputeSystem(*handle, op, &opts_w) })
        })
        .await?;
        Ok(())
    }

    /// Pause the compute system. Supported on Hyper-V-isolated systems;
    /// process-isolated containers will typically return a failure
    /// HRESULT here.
    pub async fn pause(&self, options_json: &str) -> HcsResult<()> {
        let opts_w = HSTRING::from(options_json);
        let handle = self.inner.as_raw();
        run_operation(move |op| {
            // SAFETY: see `start`.
            to_hresult(unsafe { HcsPauseComputeSystem(*handle, op, &opts_w) })
        })
        .await?;
        Ok(())
    }

    /// Resume a previously paused compute system.
    pub async fn resume(&self, options_json: &str) -> HcsResult<()> {
        let opts_w = HSTRING::from(options_json);
        let handle = self.inner.as_raw();
        run_operation(move |op| {
            // SAFETY: see `start`.
            to_hresult(unsafe { HcsResumeComputeSystem(*handle, op, &opts_w) })
        })
        .await?;
        Ok(())
    }

    /// Save the compute system's runtime state to disk.
    pub async fn save(&self, options_json: &str) -> HcsResult<()> {
        let opts_w = HSTRING::from(options_json);
        let handle = self.inner.as_raw();
        run_operation(move |op| {
            // SAFETY: see `start`.
            to_hresult(unsafe { HcsSaveComputeSystem(*handle, op, &opts_w) })
        })
        .await?;
        Ok(())
    }

    /// Read one or more properties. `property_query_json` is a JSON
    /// `PropertyQuery` document such as
    /// `{"PropertyTypes":["Statistics"]}`. Returns the raw JSON response
    /// from HCS; callers deserialize via [`crate::schema::Statistics`] (or
    /// a sibling property type) as appropriate.
    pub async fn properties(&self, property_query_json: &str) -> HcsResult<String> {
        let query_w = HSTRING::from(property_query_json);
        let handle = self.inner.as_raw();
        run_operation(move |op| {
            // SAFETY: see `start`.
            to_hresult(unsafe { HcsGetComputeSystemProperties(*handle, op, &query_w) })
        })
        .await
    }

    /// Apply a modification to a running compute system.
    /// `modification_json` is a JSON `ModifySettingRequest` document per
    /// the hcsshim schema.
    pub async fn modify(&self, modification_json: &str) -> HcsResult<()> {
        let mod_w = HSTRING::from(modification_json);
        let handle = self.inner.as_raw();
        run_operation(move |op| {
            // SAFETY: see `start`; the fourth argument (optional identity
            // handle) is `None` — we do not attach a token to the request.
            to_hresult(unsafe { HcsModifyComputeSystem(*handle, op, &mod_w, None) })
        })
        .await?;
        Ok(())
    }
}
