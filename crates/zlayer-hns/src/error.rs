//! Typed errors for HCN operations.
//!
//! HCN returns HRESULT codes for every call. This module classifies the
//! failures `ZLayer` cares about and preserves the raw HRESULT + message for
//! unclassified errors. When HCN supplies a JSON error record (`ErrorRecord`),
//! the structured fields are decoded and surfaced in [`HnsError::Other`] as
//! a concatenated message for now — callers that need structured access can
//! match on the `hresult` field.

use windows::core::HRESULT;

/// Typed result alias for the crate.
pub type HnsResult<T> = Result<T, HnsError>;

/// An error returned by any HCN operation in this crate.
#[derive(Debug, thiserror::Error)]
pub enum HnsError {
    /// The network, endpoint, or namespace identified by the given id does
    /// not exist. Common during cleanup races.
    #[error("HCN resource not found: {id}")]
    NotFound {
        /// Best-effort identifier of the missing resource.
        id: String,
    },

    /// The caller is not elevated or is not a member of Hyper-V
    /// Administrators — required for HCN control-plane operations.
    #[error("access denied (caller must be Administrator or Hyper-V Administrator): HRESULT 0x{hresult:08x}")]
    AccessDenied {
        /// Raw HRESULT.
        hresult: i32,
    },

    /// The JSON document passed to HCN was rejected as invalid.
    /// Usually caused by a missing `SchemaVersion`, a `PascalCase` quirk
    /// (e.g. `IPConfigurations` vs `IpConfigurations`), or UTF-16 null
    /// terminator handling issues.
    #[error("HCN rejected JSON document: {message} (HRESULT 0x{hresult:08x})")]
    InvalidJson {
        /// Raw HRESULT.
        hresult: i32,
        /// The decoded error record / stderr snippet HCN returned.
        message: String,
    },

    /// A NAT network creation failed because the requested subnet overlaps
    /// with another NAT network on the host. HCN allows only one NAT
    /// network per subnet.
    #[error("HCN NAT subnet conflict: {subnet}")]
    SubnetConflict {
        /// The overlapping subnet CIDR, if known.
        subnet: String,
    },

    /// A network, endpoint, or namespace policy was rejected. Most often:
    /// unknown `PolicySetting.Type`, malformed `PolicyJsonValue`, or a
    /// type mismatch between policy and resource (e.g. endpoint-only policy
    /// supplied on a network).
    #[error("HCN policy rejected: {message} (HRESULT 0x{hresult:08x})")]
    PolicyInvalid {
        /// Raw HRESULT.
        hresult: i32,
        /// Decoded error record or reason.
        message: String,
    },

    /// HCN returned an error we did not classify.
    #[error("HCN call failed: {message} (HRESULT 0x{hresult:08x})")]
    Other {
        /// Raw HRESULT.
        hresult: i32,
        /// Decoded error record, stderr snippet, or a derived description.
        message: String,
    },

    /// JSON (de)serialisation failure.
    #[error("HCN JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

impl HnsError {
    /// Build an `HnsError` from a raw `HRESULT`, mapping known values to
    /// specific variants and falling back to [`HnsError::Other`] otherwise.
    ///
    /// `message` is used verbatim for unclassified errors and for variants
    /// that carry a payload.
    #[must_use]
    pub fn from_hresult(hr: HRESULT, message: impl Into<String>) -> Self {
        // Magic numbers come from the HCN headers. See hcsshim's
        // internal/hns/error.go + learn.microsoft.com HCN reference.
        match hr.0 {
            // HCN_E_NETWORK_NOT_FOUND (0x803B0001),
            // HCN_E_ENDPOINT_NOT_FOUND (0x803B0004),
            // HCN_E_NAMESPACE_NOT_FOUND (0x803B000C)
            -0x7FC4_FFFF | -0x7FC4_FFFC | -0x7FC4_FFF4 => Self::NotFound { id: message.into() },

            // E_ACCESSDENIED (0x80070005)
            -0x7FFF_FFFB => Self::AccessDenied { hresult: hr.0 },

            // HCN_E_INVALID_JSON (0x803B000D)
            -0x7FC4_FFF3 => Self::InvalidJson {
                hresult: hr.0,
                message: message.into(),
            },

            // HCN_E_SUBNET_CONFLICT is commonly 0x803B0014
            -0x7FC4_FFEC => Self::SubnetConflict {
                subnet: message.into(),
            },

            // HCN_E_POLICY_INVALID (0x803B000E) — well-known failure mode
            // (calico/containerd often hit this). See Calico #8465.
            -0x7FC4_FFF2 => Self::PolicyInvalid {
                hresult: hr.0,
                message: message.into(),
            },

            _ => Self::Other {
                hresult: hr.0,
                message: message.into(),
            },
        }
    }

    /// Translate an [`HRESULT`] result into a `HnsResult<()>`.
    ///
    /// # Errors
    ///
    /// Returns the classified error when the HRESULT is a failure.
    pub fn check(hr: HRESULT, message: impl FnOnce() -> String) -> HnsResult<()> {
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
        let err = HnsError::from_hresult(hr(-0x7FFF_FFFB), "raw");
        assert!(matches!(err, HnsError::AccessDenied { .. }));
    }

    #[test]
    fn network_not_found_maps_specifically() {
        let err = HnsError::from_hresult(hr(-0x7FC4_FFFF), "my-net");
        if let HnsError::NotFound { id } = err {
            assert_eq!(id, "my-net");
        } else {
            panic!("expected NotFound, got {err:?}");
        }
    }

    #[test]
    fn subnet_conflict_maps_specifically() {
        let err = HnsError::from_hresult(hr(-0x7FC4_FFEC), "10.88.0.0/16");
        if let HnsError::SubnetConflict { subnet } = err {
            assert_eq!(subnet, "10.88.0.0/16");
        } else {
            panic!("expected SubnetConflict, got {err:?}");
        }
    }

    #[test]
    fn unknown_hresult_falls_through_to_other() {
        let err = HnsError::from_hresult(hr(-0x1234_5678), "unknown code");
        if let HnsError::Other { hresult, message } = err {
            assert_eq!(hresult, -0x1234_5678);
            assert_eq!(message, "unknown code");
        } else {
            panic!("expected Other, got {err:?}");
        }
    }

    #[test]
    fn check_success_returns_ok() {
        let r = HnsError::check(HRESULT(0), || "never called".to_string());
        assert!(r.is_ok());
    }

    #[test]
    fn check_failure_returns_err() {
        let r = HnsError::check(hr(-0x7FFF_FFFB), || "access denied".to_string());
        assert!(matches!(r, Err(HnsError::AccessDenied { .. })));
    }
}
