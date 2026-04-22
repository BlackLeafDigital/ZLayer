//! HCN namespace lifecycle and endpoint attach/detach.
//!
//! In the 2026 canonical flow (containerd / hcsshim / kubelet-for-windows),
//! containers are attached to networks via **namespaces**, not direct
//! endpoint-to-container attach. The flow:
//!
//!   1. Create a `HostDefault` namespace for the container.
//!   2. Create endpoint(s) on the target network.
//!   3. Add each endpoint to the namespace via `HcnModifyNamespace` with
//!      `ResourceType: 1` (Endpoint), `RequestType: Add`.
//!   4. Reference the namespace GUID in the HCS compute-system document's
//!      `Container.Networking.Namespace` field.
//!
//! Thin RAII + ergonomic wrappers around the windows-rs 0.62 entry points
//! declared in `windows::Win32::System::HostComputeNetwork`:
//!
//! - `HcnCreateNamespace` / `HcnOpenNamespace` / `HcnDeleteNamespace`
//! - `HcnModifyNamespace`
//! - `HcnQueryNamespaceProperties`
//! - `HcnEnumerateNamespaces`
//!
//! The `decode_pwstr` and `classify_error` helpers are intentionally
//! duplicated from `network.rs` / `endpoint.rs` (Phase C1 follow-up will
//! hoist them into a shared `pub(crate) mod internal;`).
//!
//! # Thread safety
//!
//! [`Namespace`] is `Send` via [`OwnedNamespace`] but not `Sync`. Wrap in
//! `Arc<Mutex<_>>` if multiple tasks need concurrent mutation through the
//! same handle.

#![allow(clippy::missing_errors_doc)]

use core::ffi::c_void;

use windows::core::{GUID, HSTRING, PWSTR};
use windows::Win32::System::HostComputeNetwork::{
    HcnCreateNamespace, HcnDeleteNamespace, HcnEnumerateNamespaces, HcnModifyNamespace,
    HcnOpenNamespace, HcnQueryNamespaceProperties,
};

use crate::error::{HnsError, HnsResult};
use crate::handle::{HcnNamespaceHandle, OwnedNamespace};
use crate::schema::{
    HostComputeNamespace, ModifyNamespaceSettingRequest, ModifyRequestType, NamespaceType,
    SchemaVersion,
};

/// Owning wrapper around an HCN namespace.
///
/// Holds both the caller-assigned GUID and the `HcnCloseNamespace`-releasing
/// [`OwnedNamespace`] handle. Dropping closes (but does not delete) the
/// namespace; call [`Namespace::delete`] explicitly to remove it from the
/// host.
#[derive(Debug)]
pub struct Namespace {
    id: GUID,
    handle: OwnedNamespace,
}

impl Namespace {
    /// Create a fresh `HostDefault` HCN namespace with a newly-generated
    /// GUID. This is the common case for per-container attach.
    pub fn create_host_default() -> HnsResult<Self> {
        let id = GUID::new().map_err(|e| HnsError::Other {
            hresult: e.code().0,
            message: format!("GUID::new failed: {e}"),
        })?;
        let spec = HostComputeNamespace {
            ty: NamespaceType::HostDefault,
            schema_version: SchemaVersion::default(),
            ..HostComputeNamespace::default()
        };
        Self::create(id, &spec)
    }

    /// Create a namespace with an explicit GUID and spec.
    ///
    /// The namespace persists in HCN until [`Namespace::delete`] is called;
    /// dropping the returned `Namespace` only releases the caller's handle.
    pub fn create(id: GUID, spec: &HostComputeNamespace) -> HnsResult<Self> {
        let settings_json = serde_json::to_string(spec)?;
        let settings_hstring = HSTRING::from(settings_json);

        // HCN's out-param is `*mut *mut c_void`; our stable handle alias is
        // `*const c_void` (see handle.rs). Keep a local `*mut c_void` for
        // the round-trip and cast when handing off to
        // `OwnedNamespace::from_raw`.
        let mut raw: *mut c_void = core::ptr::null_mut();
        let mut err_record: PWSTR = PWSTR::null();

        // SAFETY: HCN writes `raw` on success and `err_record` on failure.
        // Both out-params are pointed at local mutable storage; the HSTRING
        // lives for the duration of the call.
        unsafe {
            HcnCreateNamespace(&id, &settings_hstring, &mut raw, Some(&mut err_record))
                .map_err(|e| classify_error(e.code(), err_record, "HcnCreateNamespace"))?;
        }

        if raw.is_null() {
            return Err(HnsError::Other {
                hresult: 0,
                message: "HcnCreateNamespace returned null handle".to_string(),
            });
        }
        // SAFETY: HCN just handed us ownership of `raw`; we transfer it to
        // OwnedNamespace which is responsible for closing it on drop.
        let handle = unsafe { OwnedNamespace::from_raw(raw as HcnNamespaceHandle) };
        Ok(Self { id, handle })
    }

    /// Open an existing namespace by GUID.
    pub fn open(id: GUID) -> HnsResult<Self> {
        let mut raw: *mut c_void = core::ptr::null_mut();
        let mut err_record: PWSTR = PWSTR::null();
        // SAFETY: Same rationale as `create`.
        unsafe {
            HcnOpenNamespace(&id, &mut raw, Some(&mut err_record)).map_err(|e| {
                classify_error(e.code(), err_record, format!("HcnOpenNamespace({id:?})"))
            })?;
        }
        if raw.is_null() {
            return Err(HnsError::NotFound {
                id: format!("{id:?}"),
            });
        }
        // SAFETY: HCN just handed us ownership of `raw`.
        let handle = unsafe { OwnedNamespace::from_raw(raw as HcnNamespaceHandle) };
        Ok(Self { id, handle })
    }

    /// Delete a namespace by GUID.
    ///
    /// Stateless — does not require a live handle. If the caller still holds
    /// an `OwnedNamespace` for this id, HCN may allow it to close cleanly
    /// after delete; best practice is to drop the wrapper first.
    pub fn delete(id: GUID) -> HnsResult<()> {
        let mut err_record: PWSTR = PWSTR::null();
        // SAFETY: `id` is read-only; `err_record` is written only on failure.
        unsafe {
            HcnDeleteNamespace(&id, Some(&mut err_record)).map_err(|e| {
                classify_error(e.code(), err_record, format!("HcnDeleteNamespace({id:?})"))
            })?;
        }
        Ok(())
    }

    /// Attach an endpoint to this namespace.
    ///
    /// HCN will wire the endpoint into any compute system whose document
    /// references this namespace's GUID in its `Container.Networking.Namespace`
    /// field. `ResourceType: 1` is the well-known `Endpoint` selector.
    pub fn add_endpoint(&self, endpoint_id: GUID) -> HnsResult<()> {
        let req = ModifyNamespaceSettingRequest {
            resource_type: 1, // Endpoint
            request_type: ModifyRequestType::Add,
            settings: serde_json::json!({ "EndpointId": format!("{endpoint_id:?}") }),
        };
        self.modify_json(&serde_json::to_string(&req)?)
    }

    /// Detach an endpoint from this namespace.
    pub fn remove_endpoint(&self, endpoint_id: GUID) -> HnsResult<()> {
        let req = ModifyNamespaceSettingRequest {
            resource_type: 1, // Endpoint
            request_type: ModifyRequestType::Remove,
            settings: serde_json::json!({ "EndpointId": format!("{endpoint_id:?}") }),
        };
        self.modify_json(&serde_json::to_string(&req)?)
    }

    /// Low-level modify — pass a fully-formed `ModifyNamespaceSettingRequest`
    /// JSON string directly.
    pub fn modify_json(&self, modification_json: &str) -> HnsResult<()> {
        let mod_hstring = HSTRING::from(modification_json);
        let mut err_record: PWSTR = PWSTR::null();
        // SAFETY: handle is live (owned by `self`); strings outlive the call.
        unsafe {
            HcnModifyNamespace(self.handle.as_raw(), &mod_hstring, Some(&mut err_record))
                .map_err(|e| classify_error(e.code(), err_record, "HcnModifyNamespace"))?;
        }
        Ok(())
    }

    /// Query namespace properties, including the current resource list
    /// (attached endpoints, containers).
    ///
    /// `query_json` is the HCN query envelope; pass `"{}"` for a default
    /// query that returns all properties.
    pub fn query(&self, query_json: &str) -> HnsResult<HostComputeNamespace> {
        let query_hstring = HSTRING::from(query_json);
        let mut out_properties: PWSTR = PWSTR::null();
        let mut err_record: PWSTR = PWSTR::null();
        // SAFETY: handle is live; both out-params point at local storage.
        unsafe {
            HcnQueryNamespaceProperties(
                self.handle.as_raw(),
                &query_hstring,
                &mut out_properties,
                Some(&mut err_record),
            )
            .map_err(|e| classify_error(e.code(), err_record, "HcnQueryNamespaceProperties"))?;
        }
        let json = decode_pwstr(out_properties)?;
        let parsed: HostComputeNamespace = serde_json::from_str(&json)?;
        Ok(parsed)
    }

    /// List endpoints currently attached to this namespace. Returns the
    /// endpoint IDs as they appear in HCN's wire format (typically brace-
    /// wrapped GUID strings).
    pub fn list_endpoints(&self) -> HnsResult<Vec<String>> {
        let props = self.query("{}")?;
        Ok(props
            .resources
            .iter()
            .filter(|r| r.ty == "Endpoint")
            .map(|r| r.id.clone())
            .collect())
    }

    /// GUID this namespace was created/opened under.
    #[must_use]
    pub fn id(&self) -> GUID {
        self.id
    }

    /// Borrow the owned handle — useful when interop with other HCN calls
    /// outside this crate is needed.
    #[must_use]
    pub fn handle(&self) -> &OwnedNamespace {
        &self.handle
    }
}

/// Enumerate HCN namespaces matching `query_json`.
///
/// Pass `"{}"` (or an HCN schema-version-only envelope) to return every
/// namespace on the host. The returned vector contains the namespace GUIDs;
/// use [`Namespace::open`] to obtain a handle for each.
pub fn list(query_json: &str) -> HnsResult<Vec<GUID>> {
    let query_hstring = HSTRING::from(query_json);
    let mut out_namespaces: PWSTR = PWSTR::null();
    let mut err_record: PWSTR = PWSTR::null();
    // SAFETY: Both out-params point at local storage; query outlives the
    // call.
    unsafe {
        HcnEnumerateNamespaces(&query_hstring, &mut out_namespaces, Some(&mut err_record))
            .map_err(|e| classify_error(e.code(), err_record, "HcnEnumerateNamespaces"))?;
    }
    let json = decode_pwstr(out_namespaces)?;
    if json.is_empty() {
        return Ok(Vec::new());
    }
    // HCN returns a JSON array of GUID strings, sometimes brace-wrapped.
    let arr: Vec<String> = serde_json::from_str(&json).unwrap_or_default();
    let mut guids = Vec::with_capacity(arr.len());
    for s in arr {
        let bare = s.trim_matches(|c: char| c == '{' || c == '}');
        let guid = GUID::try_from(bare).map_err(|e| HnsError::Other {
            hresult: 0,
            message: format!("bad GUID from HcnEnumerateNamespaces: {s}: {e}"),
        })?;
        guids.push(guid);
    }
    Ok(guids)
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------
//
// These are duplicated from `network.rs` / `endpoint.rs`. Once all three
// modules are in, we should hoist them into `pub(crate) mod internal;`.

/// Convert an HCN-returned PWSTR to an owned `String` and free its backing
/// `LocalAlloc` buffer. Safe to call with a null pointer (returns empty).
fn decode_pwstr(p: PWSTR) -> HnsResult<String> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    if p.is_null() {
        return Ok(String::new());
    }
    // SAFETY: HCN handed us a null-terminated UTF-16 buffer allocated via
    // LocalAlloc. We read it, then free it via LocalFree. The PWSTR is
    // consumed by value so no caller can reuse the freed pointer.
    let s = unsafe { p.to_string().unwrap_or_default() };
    // SAFETY: `p.0` came from LocalAlloc and is alive until this LocalFree.
    unsafe {
        let _ = LocalFree(Some(HLOCAL(p.0.cast())));
    }
    Ok(s)
}

/// Classify a windows-rs `HRESULT` into an [`HnsError`], folding in the
/// decoded `ErrorRecord` PWSTR and a caller-supplied context string.
///
/// Consumes the `PWSTR` (freeing the underlying buffer) so callers never
/// leak the ErrorRecord.
fn classify_error<S: Into<String>>(
    hr: windows::core::HRESULT,
    err_record: PWSTR,
    context: S,
) -> HnsError {
    let ctx: String = context.into();
    let decoded = decode_pwstr(err_record).unwrap_or_default();
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
    use crate::schema::ModifyRequestType;

    #[test]
    fn decode_pwstr_null_returns_empty() {
        let s = decode_pwstr(PWSTR::null()).unwrap();
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
            "HcnCreateNamespace",
        );
        if let HnsError::Other { message, .. } = err {
            assert_eq!(message, "HcnCreateNamespace");
        } else {
            panic!("expected Other, got {err:?}");
        }
    }

    #[test]
    fn add_endpoint_request_serialises_with_resource_type_1() {
        // Build the request body `add_endpoint` would emit and verify the
        // wire format matches hcsshim expectations.
        let endpoint_id = GUID::from_u128(0x1234_5678_9abc_def0_1122_3344_5566_7788);
        let req = ModifyNamespaceSettingRequest {
            resource_type: 1,
            request_type: ModifyRequestType::Add,
            settings: serde_json::json!({
                "EndpointId": format!("{endpoint_id:?}")
            }),
        };
        let v: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert_eq!(v["ResourceType"], serde_json::json!(1));
        assert_eq!(v["RequestType"], serde_json::json!("Add"));
        assert!(
            v["Settings"]["EndpointId"]
                .as_str()
                .unwrap()
                .contains("12345678"),
            "EndpointId should contain GUID data: {v}"
        );
    }

    #[test]
    fn remove_endpoint_request_uses_remove_verb() {
        let endpoint_id = GUID::zeroed();
        let req = ModifyNamespaceSettingRequest {
            resource_type: 1,
            request_type: ModifyRequestType::Remove,
            settings: serde_json::json!({
                "EndpointId": format!("{endpoint_id:?}")
            }),
        };
        let v: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert_eq!(v["RequestType"], serde_json::json!("Remove"));
        assert_eq!(v["ResourceType"], serde_json::json!(1));
    }
}
