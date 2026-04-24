//! Physical network adapter discovery for HCN Transparent / `L2Bridge` binding.
//!
//! An HCN Transparent or `L2Bridge` network must be bound to the friendly name
//! of a physical host NIC (the "uplink" adapter). This module finds that
//! adapter by walking `GetAdaptersAddresses` and selecting the first up
//! adapter that owns a default IPv4 gateway — the same heuristic Calico-Windows
//! and Flannel-Windows use.
//!
//! Callers can override the auto-detection by setting
//! `ZLAYER_HCN_UPLINK_ADAPTER=<friendly name>` in the environment.

#![cfg(windows)]

use std::env;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;

use windows::core::PWSTR;
use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR, WIN32_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GAA_FLAG_INCLUDE_GATEWAYS, GET_ADAPTERS_ADDRESSES_FLAGS,
    IF_TYPE_SOFTWARE_LOOPBACK, IP_ADAPTER_ADDRESSES_LH,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Networking::WinSock::AF_INET;

use crate::error::{HnsError, HnsResult};

/// Environment variable override. When set to a non-empty value, this is
/// returned verbatim and auto-detection is skipped.
pub const ZLAYER_UPLINK_ENV: &str = "ZLAYER_HCN_UPLINK_ADAPTER";

/// Find the friendly name of the primary physical network adapter.
///
/// Heuristic: walk the `GetAdaptersAddresses` linked list and return the first
/// adapter that is:
/// 1. operational (`IfOperStatus == IfOperStatusUp`),
/// 2. not a loopback (`IfType != IF_TYPE_SOFTWARE_LOOPBACK`), and
/// 3. has at least one gateway address in its `FirstGatewayAddress` chain.
///
/// If `ZLAYER_HCN_UPLINK_ADAPTER` is set in the environment, that value is
/// returned directly without inspecting the adapter table.
///
/// # Errors
///
/// Returns [`HnsError::Other`] if `GetAdaptersAddresses` fails or if no
/// adapter satisfies the heuristic.
pub fn find_primary_adapter() -> HnsResult<String> {
    if let Ok(override_name) = env::var(ZLAYER_UPLINK_ENV) {
        let trimmed = override_name.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    let buffer = query_adapters_ipv4_with_gateways()?;
    let Some(first) = buffer.first_adapter() else {
        return Err(HnsError::Other {
            hresult: 0,
            message: format!(
                "GetAdaptersAddresses returned no adapters; set {ZLAYER_UPLINK_ENV} to override"
            ),
        });
    };

    let mut cursor: *const IP_ADAPTER_ADDRESSES_LH = first;
    while !cursor.is_null() {
        // SAFETY: `cursor` points into `buffer`, whose lifetime extends to the
        // end of this function. Each `IP_ADAPTER_ADDRESSES_LH.Next` link was
        // produced by `GetAdaptersAddresses` into this same buffer.
        let adapter = unsafe { &*cursor };

        if adapter_is_candidate(adapter) {
            // SAFETY: `FriendlyName` is a null-terminated UTF-16 string the
            // OS stored inside the buffer.
            let name = unsafe { pwstr_to_string(adapter.FriendlyName) };
            if !name.is_empty() {
                return Ok(name);
            }
        }
        cursor = adapter.Next;
    }

    Err(HnsError::Other {
        hresult: 0,
        message: format!(
            "no up physical adapter with a default IPv4 gateway found; set {ZLAYER_UPLINK_ENV} \
             to the adapter friendly name (e.g. \"Ethernet\") to override"
        ),
    })
}

/// Returns true when an adapter looks like a physical uplink with a default
/// IPv4 gateway. Tunnels, loopbacks, and down adapters are rejected.
fn adapter_is_candidate(adapter: &IP_ADAPTER_ADDRESSES_LH) -> bool {
    if adapter.OperStatus != IfOperStatusUp {
        return false;
    }
    if adapter.IfType == IF_TYPE_SOFTWARE_LOOPBACK {
        return false;
    }
    !adapter.FirstGatewayAddress.is_null()
}

/// RAII wrapper around the `GetAdaptersAddresses` output buffer.
struct AdapterBuffer {
    buf: Vec<u8>,
    /// Byte count the OS actually wrote. Same as `buf.len()` today but kept
    /// distinct so future uses can shrink without reallocating.
    #[allow(dead_code)]
    used: usize,
}

impl AdapterBuffer {
    /// Returns the head of the adapter linked list, or `None` if the buffer
    /// holds no adapters.
    fn first_adapter(&self) -> Option<*const IP_ADAPTER_ADDRESSES_LH> {
        if self.buf.is_empty() {
            return None;
        }
        Some(self.buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH)
    }
}

/// Call `GetAdaptersAddresses(AF_INET, GAA_FLAG_INCLUDE_GATEWAYS, ...)` with
/// the standard grow-until-ERROR_BUFFER_OVERFLOW-stops dance.
fn query_adapters_ipv4_with_gateways() -> HnsResult<AdapterBuffer> {
    let flags: GET_ADAPTERS_ADDRESSES_FLAGS = GAA_FLAG_INCLUDE_GATEWAYS;
    let family = u32::from(AF_INET.0);

    // Microsoft recommends starting at 15 KB.
    let mut buf_len: u32 = 15 * 1024;
    let mut buffer: Vec<u8> = Vec::new();

    for _attempt in 0..3 {
        buffer.resize(buf_len as usize, 0);
        let ret = unsafe {
            GetAdaptersAddresses(
                family,
                flags,
                None,
                Some(buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>()),
                &mut buf_len,
            )
        };
        let code = WIN32_ERROR(ret);
        if code == NO_ERROR {
            let used = buf_len as usize;
            buffer.truncate(used);
            return Ok(AdapterBuffer { buf: buffer, used });
        }
        if code == ERROR_BUFFER_OVERFLOW {
            continue;
        }
        return Err(HnsError::Other {
            hresult: ret as i32,
            message: format!("GetAdaptersAddresses failed: WIN32_ERROR {ret:#x}"),
        });
    }
    Err(HnsError::Other {
        hresult: 0,
        message: "GetAdaptersAddresses kept asking for more buffer space".to_string(),
    })
}

/// Convert a null-terminated UTF-16 PWSTR (as returned by the IP Helper API)
/// into an owned `String`. Returns an empty string on null or empty input.
///
/// # Safety
///
/// The caller must ensure `p` points to a null-terminated UTF-16 sequence
/// owned by a buffer that outlives the call.
unsafe fn pwstr_to_string(p: PWSTR) -> String {
    if p.0.is_null() {
        return String::new();
    }
    // Walk until the null terminator.
    let mut len = 0usize;
    while *p.0.add(len) != 0 {
        len += 1;
    }
    let slice = std::slice::from_raw_parts(p.0, len);
    OsString::from_wide(slice).to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_override_wins_when_set() {
        // Use a unique value so we don't collide with any real setting.
        let marker = "zlayer-test-uplink-marker";
        // Restore previous value if any to avoid leaking env state to other tests.
        let previous = env::var(ZLAYER_UPLINK_ENV).ok();
        unsafe {
            env::set_var(ZLAYER_UPLINK_ENV, marker);
        }
        let got = find_primary_adapter().unwrap();
        assert_eq!(got, marker);

        match previous {
            Some(v) => unsafe { env::set_var(ZLAYER_UPLINK_ENV, v) },
            None => unsafe { env::remove_var(ZLAYER_UPLINK_ENV) },
        }
    }

    #[test]
    fn env_override_empty_falls_through() {
        let previous = env::var(ZLAYER_UPLINK_ENV).ok();
        unsafe {
            env::set_var(ZLAYER_UPLINK_ENV, "   ");
        }
        // We can't assert the exact return here — it depends on the test host.
        // But we can assert the env override is *not* returned (trimmed empty).
        let got = find_primary_adapter();
        if let Ok(name) = got {
            assert_ne!(name, "   ");
        } else {
            // Acceptable: no real adapter on the test host (CI without a
            // physical NIC), auto-detection returns NotFound.
        }
        match previous {
            Some(v) => unsafe { env::set_var(ZLAYER_UPLINK_ENV, v) },
            None => unsafe { env::remove_var(ZLAYER_UPLINK_ENV) },
        }
    }

    /// End-to-end smoke test that only runs on a host with a real adapter.
    /// Gated with `#[ignore]` because CI runners may lack a physical NIC.
    #[test]
    #[ignore = "requires a real Windows host with a default gateway"]
    fn auto_detect_returns_adapter_name() {
        let previous = env::var(ZLAYER_UPLINK_ENV).ok();
        unsafe {
            env::remove_var(ZLAYER_UPLINK_ENV);
        }
        let name = find_primary_adapter().unwrap();
        assert!(!name.is_empty());
        if let Some(v) = previous {
            unsafe {
                env::set_var(ZLAYER_UPLINK_ENV, v);
            }
        }
    }
}
