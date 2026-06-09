//! HCN endpoint lifecycle + stats.
//!
//! Thin RAII + ergonomic wrappers around the windows-rs 0.62 entry points
//! declared in `windows::Win32::System::HostComputeNetwork`:
//!
//! - `HcnCreateEndpoint` / `HcnOpenEndpoint` / `HcnDeleteEndpoint`
//! - `HcnModifyEndpoint`
//! - `HcnQueryEndpointProperties`
//! - `HcnQueryEndpointStats`
//! - `HcnEnumerateEndpoints`
//!
//! All JSON round-trips use [`crate::schema`] types. Error paths classify the
//! raw `HRESULT` via [`HnsError::from_hresult`] and decode the `ErrorRecord`
//! PWSTR into the error message.
//!
//! The `decode_pwstr` and `classify_error` helpers are intentionally
//! duplicated from `network.rs` as private `fn` items here. Hoisting both
//! into a shared `pub(crate) mod internal;` is flagged as a follow-up in
//! Phase C1 (we keep the duplication for now to avoid racing the in-flight
//! `network.rs` work).
//!
//! # Thread safety
//!
//! [`Endpoint`] is `Send` via [`OwnedEndpoint`] but not `Sync`. Wrap in
//! `Arc<Mutex<_>>` if multiple tasks need concurrent mutation through the
//! same handle.

#![allow(clippy::missing_errors_doc)]

use core::ffi::c_void;

use windows::core::{GUID, HSTRING, PWSTR};
use windows::Win32::System::HostComputeNetwork::{
    HcnCreateEndpoint, HcnDeleteEndpoint, HcnEnumerateEndpoints, HcnModifyEndpoint,
    HcnOpenEndpoint, HcnQueryEndpointProperties, HcnQueryEndpointStats,
};

use crate::error::{HnsError, HnsResult};
use crate::handle::{HcnEndpointHandle, OwnedEndpoint};
use crate::network::Network;
use crate::schema::{EndpointStats, HostComputeEndpoint};

/// Owning wrapper around an HCN endpoint.
///
/// Holds both the caller-assigned GUID and the `HcnCloseEndpoint`-releasing
/// [`OwnedEndpoint`] handle. Dropping closes (but does not delete) the
/// endpoint; call [`Endpoint::delete`] explicitly to remove it from the host.
#[derive(Debug)]
pub struct Endpoint {
    id: GUID,
    handle: OwnedEndpoint,
}

impl Endpoint {
    /// Create an endpoint on the given network.
    ///
    /// `settings.host_compute_network` is overwritten with the string form of
    /// `network_id` so the JSON references the correct network regardless of
    /// what the caller pre-populated.
    ///
    /// `HcnCreateEndpoint` requires a **valid open network handle** as its
    /// first argument — passing null is rejected with `E_INVALIDARG`
    /// (`0x80070057`). We therefore open the target network here (via
    /// [`Network::open`]) and pass its raw `HCN_NETWORK` handle. This mirrors
    /// hcsshim's `createEndpoint` path, which calls `hcnOpenNetwork` then
    /// `hcnCreateEndpoint(networkHandle, ...)` (hcn/hcnendpoint.go). The
    /// opened [`Network`] is kept alive across the create call and dropped
    /// afterwards (its `Drop` calls `HcnCloseNetwork`).
    pub fn create(
        network_id: GUID,
        endpoint_id: GUID,
        settings: &HostComputeEndpoint,
    ) -> HnsResult<Self> {
        let mut ep_settings = settings.clone();
        ep_settings.host_compute_network = format!("{network_id:?}");

        let settings_json = serde_json::to_string(&ep_settings)?;
        let settings_hstring = HSTRING::from(settings_json);

        // Open the target network so we can hand its handle to
        // HcnCreateEndpoint. Propagate any open error (mapped via the same
        // classify_error/HnsError path used elsewhere in this crate).
        let network = Network::open(network_id)?;

        // HCN's out-param is `*mut *mut c_void`; our stable handle alias is
        // `*const c_void` (see handle.rs). Keep a local `*mut c_void` for the
        // round-trip and cast when handing off to `OwnedEndpoint::from_raw`.
        let mut raw: *mut c_void = core::ptr::null_mut();
        let mut err_record: PWSTR = PWSTR::null();

        unsafe {
            HcnCreateEndpoint(
                network.handle().as_raw(),
                &endpoint_id,
                &settings_hstring,
                &mut raw,
                Some(&mut err_record),
            )
            .map_err(|e| classify_error(e.code(), err_record, "HcnCreateEndpoint"))?;
        }

        // Keep `network` alive until after HcnCreateEndpoint returns; drop it
        // now (HcnCloseNetwork) since the endpoint no longer needs it.
        drop(network);

        if raw.is_null() {
            return Err(HnsError::Other {
                hresult: 0,
                message: "HcnCreateEndpoint returned null handle".to_string(),
            });
        }
        let handle = unsafe { OwnedEndpoint::from_raw(raw as HcnEndpointHandle) };
        Ok(Self {
            id: endpoint_id,
            handle,
        })
    }

    /// Open an existing endpoint by GUID.
    pub fn open(id: GUID) -> HnsResult<Self> {
        let mut raw: *mut c_void = core::ptr::null_mut();
        let mut err_record: PWSTR = PWSTR::null();
        unsafe {
            HcnOpenEndpoint(&id, &mut raw, Some(&mut err_record)).map_err(|e| {
                classify_error(e.code(), err_record, format!("HcnOpenEndpoint({id:?})"))
            })?;
        }
        if raw.is_null() {
            return Err(HnsError::NotFound {
                id: format!("{id:?}"),
            });
        }
        Ok(Self {
            id,
            handle: unsafe { OwnedEndpoint::from_raw(raw as HcnEndpointHandle) },
        })
    }

    /// Delete an endpoint by GUID. This permanently removes it from the host.
    pub fn delete(id: GUID) -> HnsResult<()> {
        let mut err_record: PWSTR = PWSTR::null();
        unsafe {
            HcnDeleteEndpoint(&id, Some(&mut err_record)).map_err(|e| {
                classify_error(e.code(), err_record, format!("HcnDeleteEndpoint({id:?})"))
            })?;
        }
        Ok(())
    }

    /// Apply a modification to the endpoint (policy updates, port mappings,
    /// etc.).
    pub fn modify(&self, modification_json: &str) -> HnsResult<()> {
        let mod_hstring = HSTRING::from(modification_json);
        let mut err_record: PWSTR = PWSTR::null();
        unsafe {
            HcnModifyEndpoint(self.handle.as_raw(), &mod_hstring, Some(&mut err_record))
                .map_err(|e| classify_error(e.code(), err_record, "HcnModifyEndpoint"))?;
        }
        Ok(())
    }

    /// Query full endpoint properties including IP configurations.
    ///
    /// `query_json` is the HCN query envelope; pass `"{}"` for a default
    /// query that returns all properties.
    pub fn query_properties(&self, query_json: &str) -> HnsResult<HostComputeEndpoint> {
        let query_hstring = HSTRING::from(query_json);
        let mut out_properties: PWSTR = PWSTR::null();
        let mut err_record: PWSTR = PWSTR::null();
        unsafe {
            HcnQueryEndpointProperties(
                self.handle.as_raw(),
                &query_hstring,
                &mut out_properties,
                Some(&mut err_record),
            )
            .map_err(|e| classify_error(e.code(), err_record, "HcnQueryEndpointProperties"))?;
        }
        let json = decode_pwstr(out_properties);
        let parsed: HostComputeEndpoint = serde_json::from_str(&json)?;
        Ok(parsed)
    }

    /// Query per-endpoint counters (bytes/packets sent/received).
    pub fn query_stats(&self, query_json: &str) -> HnsResult<EndpointStats> {
        let query_hstring = HSTRING::from(query_json);
        let mut out_stats: PWSTR = PWSTR::null();
        let mut err_record: PWSTR = PWSTR::null();
        unsafe {
            HcnQueryEndpointStats(
                self.handle.as_raw(),
                &query_hstring,
                &mut out_stats,
                Some(&mut err_record),
            )
            .map_err(|e| classify_error(e.code(), err_record, "HcnQueryEndpointStats"))?;
        }
        let json = decode_pwstr(out_stats);
        let parsed: EndpointStats = serde_json::from_str(&json)?;
        Ok(parsed)
    }

    /// Convenience: return the primary IP address of the endpoint, if any.
    pub fn primary_ip(&self) -> HnsResult<Option<String>> {
        let props = self.query_properties("{}")?;
        Ok(props
            .ip_configurations
            .first()
            .map(|ic| ic.ip_address.clone()))
    }

    /// The GUID this endpoint was opened/created with.
    #[must_use]
    pub fn id(&self) -> GUID {
        self.id
    }

    /// Borrow the owned HCN handle (for callers that need to pass it to
    /// functions not yet wrapped here, e.g. namespace modify).
    #[must_use]
    pub fn handle(&self) -> &OwnedEndpoint {
        &self.handle
    }
}

/// Enumerate endpoints matching an HCN query string.
///
/// Pass `"{}"` (or an empty JSON object) to list all endpoints.
pub fn list(query_json: &str) -> HnsResult<Vec<GUID>> {
    let query_hstring = HSTRING::from(query_json);
    let mut out_endpoints: PWSTR = PWSTR::null();
    let mut err_record: PWSTR = PWSTR::null();
    unsafe {
        HcnEnumerateEndpoints(&query_hstring, &mut out_endpoints, Some(&mut err_record))
            .map_err(|e| classify_error(e.code(), err_record, "HcnEnumerateEndpoints"))?;
    }
    let json = decode_pwstr(out_endpoints);
    // HCN returns a JSON array of strings; each string is the endpoint's
    // GUID, sometimes wrapped in `{...}` braces (mirroring the hcsshim wire
    // format). Strip braces and parse.
    let arr: Vec<String> = if json.is_empty() {
        Vec::new()
    } else {
        serde_json::from_str(&json).unwrap_or_default()
    };
    let mut guids = Vec::with_capacity(arr.len());
    for s in arr {
        let bare = s.trim_matches(|c: char| c == '{' || c == '}');
        let guid = GUID::try_from(bare).map_err(|e| HnsError::Other {
            hresult: 0,
            message: format!("bad GUID from HcnEnumerateEndpoints: {s}: {e}"),
        })?;
        guids.push(guid);
    }
    Ok(guids)
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------
//
// These are duplicated from the (parallel) network.rs. Once that lands we
// should hoist both to `pub(crate) mod internal;`.

/// Convert an HCN-returned PWSTR to an owned `String` and free its backing
/// `LocalAlloc` buffer. Safe to call with a null pointer (returns empty).
fn decode_pwstr(p: PWSTR) -> String {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    if p.is_null() {
        return String::new();
    }
    // SAFETY: HCN handed us a null-terminated UTF-16 buffer allocated via
    // LocalAlloc. We read it, then free it. The PWSTR is consumed by value
    // so no caller can reuse the freed pointer.
    let s = unsafe { p.to_string().unwrap_or_default() };
    // SAFETY: `p.0` came from LocalAlloc and is alive until this LocalFree.
    unsafe {
        let _ = LocalFree(Some(HLOCAL(p.0.cast())));
    }
    s
}

/// Classify a windows-rs `HRESULT` into an [`HnsError`], folding in the
/// decoded `ErrorRecord` PWSTR and a caller-supplied context string.
fn classify_error<S: Into<String>>(
    hr: windows::core::HRESULT,
    err_record: PWSTR,
    context: S,
) -> HnsError {
    let ctx: String = context.into();
    let decoded = decode_pwstr(err_record);
    let msg = if decoded.is_empty() {
        ctx
    } else {
        format!("{ctx}: {decoded}")
    };
    HnsError::from_hresult(hr, msg)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_pwstr_null_returns_empty() {
        let s = decode_pwstr(PWSTR::null());
        assert!(s.is_empty());
    }

    #[test]
    fn classify_error_access_denied_hresult() {
        let err = classify_error(windows::core::HRESULT(-0x7FFF_FFFB), PWSTR::null(), "ctx");
        assert!(matches!(err, HnsError::AccessDenied { .. }));
    }

    #[test]
    fn classify_error_preserves_context_when_errrecord_empty() {
        let err = classify_error(
            windows::core::HRESULT(-0x1234_5678),
            PWSTR::null(),
            "HcnCreateEndpoint",
        );
        if let HnsError::Other { message, .. } = err {
            assert_eq!(message, "HcnCreateEndpoint");
        } else {
            panic!("expected Other, got {err:?}");
        }
    }
}
