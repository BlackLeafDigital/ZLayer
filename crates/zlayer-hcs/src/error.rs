//! Typed errors for HCS operations.
//!
//! HCS returns HRESULT codes for every call. This module classifies the
//! common ones we care about and preserves the raw HRESULT + message for
//! anything we don't recognise, so callers can match on specific failure
//! modes (retryable, access denied, not found, etc).

use windows::core::HRESULT;

/// Typed result alias for the crate.
pub type HcsResult<T> = Result<T, HcsError>;

/// An error returned by any HCS operation in this crate.
#[derive(Debug, thiserror::Error)]
pub enum HcsError {
    /// The compute system identified by the given id does not exist. Usually
    /// benign during stop/terminate races — the caller may have already had
    /// the system removed out from under it.
    #[error("compute system not found: {id}")]
    NotFound {
        /// Best-effort identifier of the missing system.
        id: String,
    },

    /// The caller does not have the privileges required to invoke this HCS
    /// function. On Windows this almost always means the process is not a
    /// member of the local Administrators or Hyper-V Administrators group.
    #[error("access denied (caller must be Administrator or Hyper-V Administrator): HRESULT 0x{hresult:08x}")]
    AccessDenied {
        /// The raw HRESULT returned by HCS.
        hresult: i32,
    },

    /// Operation returned `HCS_E_OPERATION_PENDING` when we expected a
    /// synchronous completion. Most operations in this crate are async and
    /// internally await completion — seeing this variant at the surface
    /// means a caller used a synchronous escape hatch incorrectly.
    #[error("operation is still pending")]
    OperationPending,

    /// A system-wide event callback is already registered on this compute
    /// system, so per-operation completion callbacks can no longer be used.
    /// Choose one callback model per system and stick with it.
    #[error("system event callback already registered; operation callbacks are unavailable on this system")]
    SystemCallbackAlreadySet,

    /// The HCS schema version in the JSON document is unsupported by the
    /// host (too old or too new). HRESULT plus the message HCS emitted.
    #[error("invalid schema: {message} (HRESULT 0x{hresult:08x})")]
    InvalidSchema {
        /// Raw HRESULT.
        hresult: i32,
        /// Error message returned by HCS.
        message: String,
    },

    /// HCS returned a non-zero HRESULT that we did not classify.
    #[error("HCS call failed: {message} (HRESULT 0x{hresult:08x})")]
    Other {
        /// The raw HRESULT code.
        hresult: i32,
        /// The error message HCS returned, or a derived description if HCS
        /// did not supply one.
        message: String,
    },

    /// JSON (de)serialization failure, usually on a schema document or a
    /// returned properties payload.
    #[error("HCS JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// The HCS callback thread panicked. We catch the unwind at the FFI
    /// boundary to avoid UB across the C ABI; this surface-level error is
    /// what the caller sees. The original panic has been logged via
    /// `tracing::error!` before this is produced.
    #[error("HCS callback thread panicked: {0}")]
    CallbackPanic(String),
}

impl HcsError {
    /// Build an `HcsError` from a raw `HRESULT`, mapping known values onto
    /// specific variants and falling back to [`HcsError::Other`] otherwise.
    ///
    /// `message` is used only for unclassified errors or for the
    /// [`HcsError::InvalidSchema`] variant; it is ignored for all other
    /// known variants that don't carry a payload.
    #[must_use]
    pub fn from_hresult(hr: HRESULT, message: impl Into<String>) -> Self {
        // Magic numbers come from winerror.h / vmcompute headers. Treat any
        // match conservatively — HCS occasionally reuses generic codes
        // (E_ACCESSDENIED, E_INVALIDARG) for multiple semantic failures.
        match hr.0 {
            // HCS_E_SYSTEM_NOT_FOUND (0x80370100) — used on Get/Open/Stop
            // after a system is already gone.
            -0x7FC8_FF00 => Self::NotFound { id: message.into() },

            // E_ACCESSDENIED (0x80070005) — almost always the admin-group issue.
            -0x7FFF_FFFB => Self::AccessDenied { hresult: hr.0 },

            // HCS_E_OPERATION_PENDING (0x8037_0103)
            -0x7FC8_FEFD => Self::OperationPending,

            // HCS_E_SYSTEM_CALLBACK_ALREADY_SET (0x8037_0109)
            -0x7FC8_FEF7 => Self::SystemCallbackAlreadySet,

            // HCS_E_INVALID_JSON (0x8037_0002) / HCS_E_INVALID_DOCUMENT etc.
            -0x7FC8_FFFE | -0x7FC8_FFFD => Self::InvalidSchema {
                hresult: hr.0,
                message: message.into(),
            },

            _ => Self::Other {
                hresult: hr.0,
                message: message.into(),
            },
        }
    }

    /// Translate an [`HRESULT`] result into a Rust `HcsResult<()>`. Success
    /// (`hr.is_ok()`) maps to `Ok(())`; any failure uses
    /// [`HcsError::from_hresult`] with the given message supplier.
    ///
    /// # Errors
    ///
    /// Returns the classified error whenever the HRESULT is a failure.
    pub fn check(hr: HRESULT, message: impl FnOnce() -> String) -> HcsResult<()> {
        if hr.is_ok() {
            Ok(())
        } else {
            Err(Self::from_hresult(hr, message()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::HRESULT;

    fn hr(code: i32) -> HRESULT {
        HRESULT(code)
    }

    #[test]
    fn access_denied_maps_specifically() {
        let err = HcsError::from_hresult(hr(-0x7FFF_FFFB), "raw");
        assert!(matches!(err, HcsError::AccessDenied { .. }));
    }

    #[test]
    fn not_found_maps_specifically() {
        let err = HcsError::from_hresult(hr(-0x7FC8_FF00), "my-container");
        if let HcsError::NotFound { id } = err {
            assert_eq!(id, "my-container");
        } else {
            panic!("expected NotFound, got {err:?}");
        }
    }

    #[test]
    fn unknown_hresult_falls_through_to_other() {
        let err = HcsError::from_hresult(hr(-0x1234_5678), "unknown code");
        if let HcsError::Other { hresult, message } = err {
            assert_eq!(hresult, -0x1234_5678);
            assert_eq!(message, "unknown code");
        } else {
            panic!("expected Other, got {err:?}");
        }
    }

    #[test]
    fn check_success_returns_ok() {
        let r = HcsError::check(HRESULT(0), || "should not be called".to_string());
        assert!(r.is_ok());
    }

    #[test]
    fn check_failure_returns_err() {
        let r = HcsError::check(hr(-0x7FFF_FFFB), || "access denied".to_string());
        assert!(matches!(r, Err(HcsError::AccessDenied { .. })));
    }
}
