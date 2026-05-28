//! HCN network lifecycle.
//!
//! Wraps the six `Hcn*Network*` / `HcnEnumerateNetworks` entry points exposed
//! by `computenetwork.dll` as a safe, RAII-friendly Rust surface:
//!
//! - [`Network::create`] — `HcnCreateNetwork`
//! - [`Network::open`]   — `HcnOpenNetwork`
//! - [`Network::delete`] — `HcnDeleteNetwork` (stateless, by GUID)
//! - [`Network::modify`] — `HcnModifyNetwork`
//! - [`Network::query`]  — `HcnQueryNetworkProperties`
//! - [`list`]            — `HcnEnumerateNetworks`
//!
//! All calls translate HCN's `ErrorRecord` `PWSTR` (allocated via `LocalAlloc`)
//! into a typed [`HnsError`] with the decoded payload preserved. The owned
//! handle returned by `create` / `open` drops through [`OwnedNetwork`], which
//! calls `HcnCloseNetwork` — note that closing does **not** delete the
//! network; use [`Network::delete`] for that.

#![allow(clippy::missing_errors_doc)]

use core::ffi::c_void;

use windows::core::{GUID, HSTRING, PWSTR};
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::System::HostComputeNetwork::{
    HcnCreateNetwork, HcnDeleteNetwork, HcnEnumerateNetworks, HcnModifyNetwork, HcnOpenNetwork,
    HcnQueryNetworkProperties,
};

use crate::error::{HnsError, HnsResult};
use crate::handle::{HcnNetworkHandle, OwnedNetwork};
use crate::schema::{HostComputeNetwork, Ipam, NetworkType, Route, SchemaVersion, Subnet};

/// Owning wrapper around an HCN network. Drop closes (but does not delete)
/// the underlying handle.
#[derive(Debug)]
pub struct Network {
    id: GUID,
    handle: OwnedNetwork,
}

impl Network {
    /// Create a new HCN network from a [`HostComputeNetwork`] spec.
    ///
    /// `id` becomes the HCN-addressable network GUID. HCN persists the
    /// network until [`Network::delete`] is called; dropping the returned
    /// `Network` only releases the caller's handle.
    pub fn create(id: GUID, settings: &HostComputeNetwork) -> HnsResult<Self> {
        let settings_json = serde_json::to_string(settings)?;
        let settings_hstring = HSTRING::from(settings_json);
        let mut raw: *mut c_void = core::ptr::null_mut();
        let mut err_record: PWSTR = PWSTR::null();

        // SAFETY: HCN writes `raw` on success and `err_record` on failure.
        // Both out-params are pointed at local mutable storage; the HSTRING
        // lives for the duration of the call.
        unsafe {
            HcnCreateNetwork(&id, &settings_hstring, &mut raw, Some(&mut err_record))
                .map_err(|e| classify_error(e.code(), err_record, "HcnCreateNetwork"))?;
        }

        if raw.is_null() {
            return Err(HnsError::Other {
                hresult: 0,
                message: "HcnCreateNetwork returned null handle".to_string(),
            });
        }
        // SAFETY: HCN just handed us ownership of `raw`; we transfer it to
        // OwnedNetwork which is responsible for closing it on drop.
        let handle = unsafe { OwnedNetwork::from_raw(raw as HcnNetworkHandle) };
        Ok(Self { id, handle })
    }

    /// Open an existing HCN network by GUID.
    pub fn open(id: GUID) -> HnsResult<Self> {
        let mut raw: *mut c_void = core::ptr::null_mut();
        let mut err_record: PWSTR = PWSTR::null();
        // SAFETY: Same rationale as `create`.
        unsafe {
            HcnOpenNetwork(&id, &mut raw, Some(&mut err_record)).map_err(|e| {
                classify_error(e.code(), err_record, format!("HcnOpenNetwork({id:?})"))
            })?;
        }
        if raw.is_null() {
            return Err(HnsError::NotFound {
                id: format!("{id:?}"),
            });
        }
        // SAFETY: HCN just handed us ownership of `raw`.
        let handle = unsafe { OwnedNetwork::from_raw(raw as HcnNetworkHandle) };
        Ok(Self { id, handle })
    }

    /// Delete an HCN network by GUID.
    ///
    /// Stateless — does not require a live handle. If the caller still holds
    /// an `OwnedNetwork` for this id, HCN may allow it to close cleanly after
    /// delete; best practice is to drop the wrapper first.
    pub fn delete(id: GUID) -> HnsResult<()> {
        let mut err_record: PWSTR = PWSTR::null();
        // SAFETY: `id` is read-only; `err_record` is written only on failure.
        unsafe {
            HcnDeleteNetwork(&id, Some(&mut err_record)).map_err(|e| {
                classify_error(e.code(), err_record, format!("HcnDeleteNetwork({id:?})"))
            })?;
        }
        Ok(())
    }

    /// Apply a modification to the network.
    ///
    /// `modification_json` is the `ModifyNetworkSettingRequest` envelope
    /// (policies, subnets, DNS, etc) — see `hcsshim/hcn` for the schema.
    pub fn modify(&self, modification_json: &str) -> HnsResult<()> {
        let mod_hstring = HSTRING::from(modification_json);
        let mut err_record: PWSTR = PWSTR::null();
        // SAFETY: handle is live (owned by `self`); strings outlive the call.
        unsafe {
            HcnModifyNetwork(self.handle.as_raw(), &mod_hstring, Some(&mut err_record))
                .map_err(|e| classify_error(e.code(), err_record, "HcnModifyNetwork"))?;
        }
        Ok(())
    }

    /// Query network properties as a [`HostComputeNetwork`].
    ///
    /// `property_query_json` is typically the empty schema-version envelope
    /// `{"SchemaVersion":{"Major":2,"Minor":0}}`.
    pub fn query(&self, property_query_json: &str) -> HnsResult<HostComputeNetwork> {
        let query_hstring = HSTRING::from(property_query_json);
        let mut out_properties: PWSTR = PWSTR::null();
        let mut err_record: PWSTR = PWSTR::null();
        // SAFETY: handle is live; both out-params point at local storage.
        unsafe {
            HcnQueryNetworkProperties(
                self.handle.as_raw(),
                &query_hstring,
                &mut out_properties,
                Some(&mut err_record),
            )
            .map_err(|e| classify_error(e.code(), err_record, "HcnQueryNetworkProperties"))?;
        }
        let json = decode_pwstr(out_properties);
        let parsed: HostComputeNetwork = serde_json::from_str(&json)?;
        Ok(parsed)
    }

    /// Create an HCN **Transparent** network bound to the given uplink
    /// physical adapter, with the given `/slice_prefix` IPv4 subnet.
    ///
    /// Transparent HCN networks put each endpoint directly on the uplink's L2
    /// segment with a caller-chosen IP. Combined with the endpoint policies
    /// from [`crate::schema::EndpointPolicy`], this is how `ZLayer` replaces the
    /// older HNS NAT model so Windows containers have real overlay IPs.
    ///
    /// The `subnet` CIDR becomes the Transparent network's IPAM subnet — HCN
    /// installs a connected route for this range on the uplink vSwitch
    /// automatically. Callers typically pass the node's per-node `/28` slice.
    ///
    /// `uplink_adapter_name` must be the friendly name of a physical adapter
    /// (the value returned by [`crate::adapter::find_primary_adapter`]).
    ///
    /// # Errors
    ///
    /// Returns any error from `HcnCreateNetwork` or from JSON serialization.
    pub fn create_transparent(
        id: GUID,
        name: &str,
        subnet: &str,
        uplink_adapter_name: &str,
    ) -> HnsResult<Self> {
        // Without an explicit gateway in the IPAM subnet, HCN's Transparent
        // driver picks one itself AND silently reserves additional addresses
        // beyond the gateway, causing `HcnCreateEndpoint` to reject low-numbered
        // host addresses (e.g. `.2` on a `/28`) with `HCN_E_ADDR_INVALID_OR_RESERVED`
        // (`0x803b002f`). Declare the gateway via a default route on the subnet
        // so HCN treats every other address in the prefix as available.
        let gateway = first_host_address(subnet).ok_or_else(|| HnsError::Other {
            hresult: 0,
            message: format!("create_transparent: invalid subnet CIDR '{subnet}'"),
        })?;
        let settings = HostComputeNetwork {
            id: None,
            name: name.to_string(),
            ty: NetworkType::Transparent,
            policies: vec![net_adapter_name_policy(uplink_adapter_name)],
            mac_pool: None,
            dns: None,
            ipams: vec![Ipam {
                ty: "Static".to_string(),
                subnets: vec![Subnet {
                    ip_address_prefix: subnet.to_string(),
                    routes: vec![Route {
                        next_hop: gateway,
                        destination_prefix: "0.0.0.0/0".to_string(),
                        metric: None,
                    }],
                    policies: Vec::new(),
                }],
            }],
            flags: 0,
            schema_version: SchemaVersion::default(),
        };
        Self::create(id, &settings)
    }

    /// GUID this network was created/opened under.
    #[must_use]
    pub fn id(&self) -> GUID {
        self.id
    }

    /// Borrow the owned handle — useful when interop with other HCN calls
    /// outside this crate is needed.
    #[must_use]
    pub fn handle(&self) -> &OwnedNetwork {
        &self.handle
    }
}

/// First usable host address in a CIDR — the conventional default-gateway
/// address for the subnet (`network + 1`). Returns `None` for malformed CIDR
/// strings or subnets too small to contain a usable host (`/31`, `/32`).
fn first_host_address(cidr: &str) -> Option<String> {
    let net: ipnet::IpNet = cidr.parse().ok()?;
    let ipnet::IpNet::V4(v4) = net else {
        // HCN Transparent overlay is IPv4-only in the current schema.
        return None;
    };
    if v4.prefix_len() >= 31 {
        return None;
    }
    let network = u32::from(v4.network());
    let gateway = std::net::Ipv4Addr::from(network.checked_add(1)?);
    Some(gateway.to_string())
}

/// Build the `NetAdapterName` network policy that binds a Transparent or
/// `L2Bridge` HCN network to a specific physical uplink adapter.
fn net_adapter_name_policy(adapter_name: &str) -> serde_json::Value {
    serde_json::json!({
        "Type": "NetAdapterName",
        "Settings": { "NetworkAdapterName": adapter_name }
    })
}

/// Build the [`HostComputeNetwork`] document we hand to `HcnCreateNetwork`
/// for a Transparent network. Factored out of `Network::create_transparent`
/// so unit tests can assert on the JSON shape without needing a live HCN.
#[cfg(test)]
pub(crate) fn transparent_settings(
    name: &str,
    subnet: &str,
    uplink_adapter_name: &str,
) -> HostComputeNetwork {
    HostComputeNetwork {
        id: None,
        name: name.to_string(),
        ty: NetworkType::Transparent,
        policies: vec![net_adapter_name_policy(uplink_adapter_name)],
        mac_pool: None,
        dns: None,
        ipams: vec![Ipam {
            ty: "Static".to_string(),
            subnets: vec![Subnet {
                ip_address_prefix: subnet.to_string(),
                routes: Vec::new(),
                policies: Vec::new(),
            }],
        }],
        flags: 0,
        schema_version: SchemaVersion::default(),
    }
}

/// Enumerate HCN networks matching `query_json`.
///
/// Pass `"{}"` (or an HCN schema-version-only envelope) to return every
/// network on the host. The returned vector contains the network GUIDs; use
/// [`Network::open`] to obtain a handle for each.
pub fn list(query_json: &str) -> HnsResult<Vec<GUID>> {
    let query_hstring = HSTRING::from(query_json);
    let mut out_networks: PWSTR = PWSTR::null();
    let mut err_record: PWSTR = PWSTR::null();
    // SAFETY: Both out-params point at local storage; query outlives the call.
    unsafe {
        HcnEnumerateNetworks(&query_hstring, &mut out_networks, Some(&mut err_record))
            .map_err(|e| classify_error(e.code(), err_record, "HcnEnumerateNetworks"))?;
    }
    let json = decode_pwstr(out_networks);
    if json.is_empty() {
        return Ok(Vec::new());
    }
    // HCN returns a JSON array of GUID strings, some variants brace-wrapped.
    let arr: Vec<String> = serde_json::from_str(&json).unwrap_or_default();
    let mut guids = Vec::with_capacity(arr.len());
    for s in arr {
        let bare = s.trim_matches(|c: char| c == '{' || c == '}');
        let guid = GUID::try_from(bare).map_err(|e| HnsError::Other {
            hresult: 0,
            message: format!("bad GUID from HcnEnumerateNetworks: {s}: {e}"),
        })?;
        guids.push(guid);
    }
    Ok(guids)
}

/// Decode a `PWSTR` returned by HCN and release its backing buffer.
///
/// HCN allocates JSON buffers via `LocalAlloc`; the caller must free them
/// with `LocalFree`. We do both here so every call site stays leak-free.
fn decode_pwstr(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: HCN handed us a null-terminated UTF-16 buffer allocated via
    // LocalAlloc. We read it, then free it via LocalFree. The PWSTR is
    // consumed by value so no caller can reuse the freed pointer.
    let s = unsafe { p.to_string().unwrap_or_default() };
    // SAFETY: `p.0` came from LocalAlloc and is alive until this LocalFree.
    unsafe {
        let _ = LocalFree(Some(HLOCAL(p.0.cast())));
    }
    s
}

/// Convert a failed HCN HRESULT + `ErrorRecord` `PWSTR` into a typed
/// [`HnsError`].
///
/// Consumes the `PWSTR` (freeing the underlying buffer) so callers never
/// leak the `ErrorRecord`.
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

#[cfg(test)]
mod tests {
    use super::{net_adapter_name_policy, transparent_settings};
    use serde_json::json;

    #[test]
    fn net_adapter_name_policy_shape_matches_hcsshim() {
        let v = net_adapter_name_policy("Ethernet 2");
        assert_eq!(
            v,
            json!({
                "Type": "NetAdapterName",
                "Settings": { "NetworkAdapterName": "Ethernet 2" }
            })
        );
    }

    #[test]
    fn transparent_settings_wire_format() {
        let settings = transparent_settings("zlayer-overlay", "10.200.42.0/28", "Ethernet");
        let v = serde_json::to_value(&settings).unwrap();

        assert_eq!(v["Name"], json!("zlayer-overlay"));
        assert_eq!(v["Type"], json!("Transparent"));
        assert_eq!(v["Ipams"][0]["Type"], json!("Static"));
        assert_eq!(
            v["Ipams"][0]["Subnets"][0]["IpAddressPrefix"],
            json!("10.200.42.0/28")
        );
        assert_eq!(v["Policies"][0]["Type"], json!("NetAdapterName"));
        assert_eq!(
            v["Policies"][0]["Settings"]["NetworkAdapterName"],
            json!("Ethernet")
        );
        assert_eq!(v["SchemaVersion"]["Major"], json!(2));
        assert_eq!(v["SchemaVersion"]["Minor"], json!(0));
    }

    #[test]
    fn transparent_settings_round_trip_preserves_shape() {
        let settings = transparent_settings("zlayer-overlay", "10.200.0.0/28", "Ethernet 2");
        let json_str = serde_json::to_string(&settings).unwrap();
        let parsed: crate::schema::HostComputeNetwork = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed.name, "zlayer-overlay");
        assert!(matches!(parsed.ty, crate::schema::NetworkType::Transparent));
        assert_eq!(parsed.ipams.len(), 1);
        assert_eq!(
            parsed.ipams[0].subnets[0].ip_address_prefix,
            "10.200.0.0/28"
        );
        assert_eq!(parsed.policies.len(), 1);
        assert_eq!(parsed.policies[0]["Type"], "NetAdapterName");
    }
}
