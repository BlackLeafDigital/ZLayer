//! Windows-gated overlay E2E tests.
//!
//! These tests require a real Windows 10/11 or Windows Server host with
//! HCN + Wintun installed. They create real HCN networks and endpoints;
//! they are `#[ignore]` by default.
//!
//! Style lints that don't fight the architecture are allowed: test bodies
//! exercising the full HCN setup/teardown sequence naturally exceed the
//! 100-line limit; `_net` bindings are kept alive as RAII guards; the file
//! references `PowerShell` / `New-NetRoute` / etc. in plain prose.
#![allow(
    clippy::too_many_lines,
    clippy::used_underscore_binding,
    clippy::doc_markdown,
    clippy::single_match_else,
    clippy::match_wild_err_arm
)]
//!
//! Run with:
//! ```powershell
//! cargo test --test windows_overlay_e2e -- --ignored --nocapture
//! ```
//!
//! Set `ZLAYER_HCN_UPLINK_ADAPTER` to override the auto-detected primary
//! adapter if the test's default heuristic picks the wrong one.
//!
//! # Design notes
//!
//! Tests use the dedicated test CIDR `10.220.99.0/28` (not `10.200.0.0/16`)
//! so they do not collide with a real `ZLayer` cluster that happens to be
//! running on the same host. A post-test `PowerShell` sweep is recommended
//! if a test panics mid-way — see the teardown comments in each test.
//!
//! Every sync HCN call is wrapped in `tokio::task::spawn_blocking` so the
//! multi-threaded runtime does not block its reactor thread on the HCS
//! RPC round-trip.

#![cfg(target_os = "windows")]

use std::net::IpAddr;

use windows::core::GUID;

use zlayer_hns::adapter::find_primary_adapter;
use zlayer_hns::attach::EndpointAttachment;
use zlayer_hns::endpoint::Endpoint;
use zlayer_hns::network::Network;
use zlayer_hns::schema::{
    AclAction, AclDirection, AclPolicySetting, OutBoundNatPolicySetting, SdnRoutePolicySetting,
};

/// Test-only CIDR chosen to avoid colliding with a live ZLayer cluster on
/// the same host. `/28` gives us 16 addresses — enough for a single test
/// endpoint and its gateway.
const TEST_CIDR: &str = "10.220.99.0/28";
const TEST_IP: &str = "10.220.99.2";
const TEST_PREFIX_LEN: u8 = 28;

/// Allocate a fresh GUID or fail the test with a clear message. HCN
/// requires GUIDs to be unique across the host, so we rely on v4 random
/// GUIDs here.
fn new_guid() -> GUID {
    GUID::new().expect("GUID::new must succeed on a live Windows host")
}

/// Smoke test: `find_primary_adapter` must return a non-empty friendly
/// name on a real host with a default gateway. Run before any of the HCN
/// tests — if this fails, the rest are guaranteed to fail as well.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real Windows host with a default IPv4 gateway"]
async fn test_find_primary_adapter_nonempty() {
    let name = tokio::task::spawn_blocking(find_primary_adapter)
        .await
        .expect("join error")
        .expect("find_primary_adapter must succeed on a real host");
    assert!(
        !name.is_empty(),
        "adapter friendly name must not be empty; got {name:?}",
    );
    eprintln!("primary adapter: {name}");
}

/// The highest-value end-to-end test. Creates a Transparent HCN network
/// bound to the primary uplink, creates an overlay endpoint with the
/// three Transparent-network policies (`OutBoundNAT`, `SDNRoute`, `ACL`),
/// then reads the endpoint properties back out of HCN and asserts the
/// wire layout matches what we wrote.
///
/// Teardown runs even if the assertions fail — endpoint + namespace are
/// deleted via `EndpointAttachment::teardown`, and the network is
/// deleted via `Network::delete`. Both are best-effort.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "creates real HCN network + endpoint; requires admin on Windows"]
async fn test_create_transparent_network_and_endpoint() {
    let adapter = tokio::task::spawn_blocking(find_primary_adapter)
        .await
        .expect("join error")
        .expect("find_primary_adapter must succeed");
    eprintln!("using uplink adapter: {adapter}");

    let net_id = new_guid();
    let adapter_clone = adapter.clone();

    // --- Create network ---------------------------------------------------
    let net_result = tokio::task::spawn_blocking(move || {
        Network::create_transparent(net_id, "zlayer-e2e-test", TEST_CIDR, &adapter_clone)
    })
    .await
    .expect("join error");

    let _net = match net_result {
        Ok(n) => n,
        Err(e) => {
            eprintln!(
                "FAILED: Network::create_transparent: {e:?}\n\
                 Common causes: not elevated, Hyper-V feature not enabled, \
                 HNS service not running, or uplink adapter {adapter:?} is not \
                 actually a physical NIC."
            );
            panic!("Network::create_transparent: {e}");
        }
    };

    // --- Create overlay endpoint ------------------------------------------
    let ip: IpAddr = TEST_IP.parse().expect("TEST_IP is a valid IPv4 literal");
    let attach_result = tokio::task::spawn_blocking(move || {
        EndpointAttachment::create_overlay(
            net_id,
            "zlayer-test",
            "test-container-1",
            ip,
            TEST_PREFIX_LEN,
            TEST_CIDR,
            None,
            None,
        )
    })
    .await
    .expect("join error");

    let attachment = match attach_result {
        Ok(a) => a,
        Err(e) => {
            eprintln!("FAILED: EndpointAttachment::create_overlay: {e:?}");
            // Best-effort cleanup of the network we just made.
            let _ = tokio::task::spawn_blocking(move || Network::delete(net_id)).await;
            panic!("EndpointAttachment::create_overlay: {e}");
        }
    };

    let endpoint_id = attachment.endpoint_id();

    // --- Read back endpoint properties and assert ------------------------
    //
    // We wrap the assertion body in a closure so we can still run teardown
    // even if any assertion panics.
    let props_result = tokio::task::spawn_blocking(move || {
        let ep = Endpoint::open(endpoint_id)?;
        ep.query_properties("{}")
    })
    .await
    .expect("join error");

    let assertion_outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let props = props_result.expect("query_properties must succeed on a live endpoint");

        // IP configuration must match what we allocated.
        assert_eq!(
            props.ip_configurations.len(),
            1,
            "expected exactly one IP configuration, got {:?}",
            props.ip_configurations
        );
        assert_eq!(
            props.ip_configurations[0].ip_address, TEST_IP,
            "IPConfigurations[0].IpAddress wire mismatch",
        );
        assert_eq!(
            props.ip_configurations[0].prefix_length, TEST_PREFIX_LEN,
            "IPConfigurations[0].PrefixLength wire mismatch",
        );

        // Policies must round-trip each of the three Transparent-network
        // policies we wrote in `EndpointAttachment::create_overlay`. HCN
        // may add extra policies on its own; we assert presence, not
        // exclusivity.
        let mut saw_outbound_nat = false;
        let mut saw_sdn_route = false;
        let mut saw_acl = false;
        for p in &props.policies {
            match p.get("Type").and_then(|v| v.as_str()) {
                Some("OutBoundNAT") => saw_outbound_nat = true,
                Some("SDNRoute") => saw_sdn_route = true,
                Some("ACL") => saw_acl = true,
                _ => {}
            }
        }
        assert!(
            saw_outbound_nat,
            "expected OutBoundNAT policy, got {:?}",
            props.policies
        );
        assert!(
            saw_sdn_route,
            "expected SDNRoute policy, got {:?}",
            props.policies
        );
        assert!(saw_acl, "expected ACL policy, got {:?}", props.policies);
    }));

    // --- Teardown (always runs) ------------------------------------------
    let teardown_ep = tokio::task::spawn_blocking(move || attachment.teardown()).await;
    if let Ok(Err(e)) = &teardown_ep {
        eprintln!("teardown: EndpointAttachment::teardown failed: {e:?}");
    } else if let Err(e) = &teardown_ep {
        eprintln!("teardown: spawn_blocking join error: {e:?}");
    }

    // Drop the owning handle before `HcnDeleteNetwork` — per network.rs,
    // best practice is to close the wrapper first.
    drop(_net);
    let teardown_net = tokio::task::spawn_blocking(move || Network::delete(net_id)).await;
    if let Ok(Err(e)) = &teardown_net {
        eprintln!("teardown: Network::delete failed: {e:?}");
    } else if let Err(e) = &teardown_net {
        eprintln!("teardown: spawn_blocking join error: {e:?}");
    }

    // Propagate any assertion failure now that resources are cleaned up.
    if let Err(p) = assertion_outcome {
        std::panic::resume_unwind(p);
    }
}

/// Verify that every policy we write comes back through HCN with the
/// same shape — i.e. the typed structs in [`zlayer_hns::schema`]
/// deserialize what HCN echoed back without data loss.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "creates real HCN network + endpoint; requires admin on Windows"]
async fn test_endpoint_policy_json_matches_wire() {
    let adapter = tokio::task::spawn_blocking(find_primary_adapter)
        .await
        .expect("join error")
        .expect("find_primary_adapter must succeed");

    let net_id = new_guid();
    let adapter_clone = adapter.clone();

    let _net = tokio::task::spawn_blocking(move || {
        Network::create_transparent(net_id, "zlayer-e2e-policy", TEST_CIDR, &adapter_clone)
    })
    .await
    .expect("join error")
    .expect("Network::create_transparent");

    let ip: IpAddr = TEST_IP.parse().expect("valid IPv4");
    let attachment = tokio::task::spawn_blocking(move || {
        EndpointAttachment::create_overlay(
            net_id,
            "zlayer-test",
            "test-container-1",
            ip,
            TEST_PREFIX_LEN,
            TEST_CIDR,
            None,
            None,
        )
    })
    .await
    .expect("join error");

    let attachment = match attachment {
        Ok(a) => a,
        Err(e) => {
            let _ = tokio::task::spawn_blocking(move || Network::delete(net_id)).await;
            panic!("EndpointAttachment::create_overlay: {e}");
        }
    };
    let endpoint_id = attachment.endpoint_id();

    let props_result = tokio::task::spawn_blocking(move || {
        let ep = Endpoint::open(endpoint_id)?;
        ep.query_properties("{}")
    })
    .await
    .expect("join error");

    let assertion_outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let props = props_result.expect("query_properties");

        for p in &props.policies {
            let ty = p
                .get("Type")
                .and_then(|v| v.as_str())
                .expect("every policy must have a Type field");
            let settings = p
                .get("Settings")
                .cloned()
                .expect("every policy must have a Settings object");

            match ty {
                "OutBoundNAT" => {
                    let decoded: OutBoundNatPolicySetting = serde_json::from_value(settings)
                        .expect(
                            "OutBoundNAT Settings must deserialize into OutBoundNatPolicySetting",
                        );
                    assert_eq!(
                        decoded.exceptions,
                        vec![TEST_CIDR.to_string()],
                        "OutBoundNAT.Exceptions wire mismatch",
                    );
                }
                "SDNRoute" => {
                    let decoded: SdnRoutePolicySetting = serde_json::from_value(settings)
                        .expect("SDNRoute Settings must deserialize into SdnRoutePolicySetting");
                    assert_eq!(
                        decoded.destination_prefix, TEST_CIDR,
                        "SDNRoute.DestinationPrefix wire mismatch",
                    );
                    assert!(
                        !decoded.need_encap,
                        "SDNRoute.NeedEncap must be false for overlay-on-WireGuard",
                    );
                }
                "ACL" => {
                    let decoded: AclPolicySetting = serde_json::from_value(settings)
                        .expect("ACL Settings must deserialize into AclPolicySetting");
                    assert_eq!(decoded.action, AclAction::Allow, "ACL.Action wire mismatch",);
                    assert_eq!(
                        decoded.direction,
                        AclDirection::In,
                        "ACL.Direction wire mismatch",
                    );
                    assert_eq!(
                        decoded.remote_addresses, TEST_CIDR,
                        "ACL.RemoteAddresses wire mismatch",
                    );
                }
                // HCN sometimes injects extra policies (e.g. auto-managed
                // NetAdapterName on Transparent networks). Unknown types
                // are fine — we only assert on the three we installed.
                _ => {
                    eprintln!("ignoring unknown policy type {ty:?}");
                }
            }
        }
    }));

    let teardown_ep = tokio::task::spawn_blocking(move || attachment.teardown()).await;
    if let Ok(Err(e)) = &teardown_ep {
        eprintln!("teardown: EndpointAttachment::teardown failed: {e:?}");
    }
    drop(_net);
    let teardown_net = tokio::task::spawn_blocking(move || Network::delete(net_id)).await;
    if let Ok(Err(e)) = &teardown_net {
        eprintln!("teardown: Network::delete failed: {e:?}");
    }

    if let Err(p) = assertion_outcome {
        std::panic::resume_unwind(p);
    }
}

/// Install a test host route `10.220.99.0/28 → <wintun adapter>`, verify
/// it shows up in `Get-NetRoute`, then delete it.
///
/// This test exercises the same route-install behavior used by Wintun's
/// `/16` host route, but targets our test CIDR so it cannot collide with
/// a live cluster. It is optional: if no Wintun adapter is present we
/// skip with a clear `eprintln!` rather than failing.
///
/// `WindowsIpHelperOps` in `zlayer-overlay` is `pub(crate)`, so from an
/// integration test we drive the equivalent operation via the
/// `New-NetRoute` / `Remove-NetRoute` PowerShell cmdlets — these wrap
/// exactly the same `CreateIpForwardEntry2` call that the crate issues.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live Wintun adapter (zlayer-overlay or equivalent)"]
async fn test_host_route_install_via_wintun() {
    // 1. Discover a Wintun-ish adapter.
    let Some(adapter_alias) = find_wintun_adapter_alias().await else {
        eprintln!(
            "SKIP: no Wintun adapter (looked for 'zlayer-overlay' or any \
             InterfaceDescription matching 'Wintun'); start the overlay \
             transport before re-running this test."
        );
        return;
    };
    eprintln!("using wintun adapter: {adapter_alias}");

    // 2. Install the route via New-NetRoute.
    let install = run_powershell(&format!(
        "New-NetRoute -DestinationPrefix '{TEST_CIDR}' \
         -InterfaceAlias '{adapter_alias}' -PolicyStore ActiveStore \
         -ErrorAction Stop | Out-Null"
    ))
    .await;
    if let Err(e) = install {
        eprintln!("FAILED to install test route: {e}");
        panic!("New-NetRoute failed: {e}");
    }

    // 3. Verify via Get-NetRoute.
    let verify = run_powershell(&format!(
        "Get-NetRoute -DestinationPrefix '{TEST_CIDR}' \
         -ErrorAction SilentlyContinue | Format-List"
    ))
    .await;

    let assertion_outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let stdout = verify.expect("Get-NetRoute invocation");
        assert!(
            stdout.contains("InterfaceAlias"),
            "Get-NetRoute stdout missing 'InterfaceAlias' field: {stdout}",
        );
        assert!(
            stdout.contains(&adapter_alias),
            "Get-NetRoute stdout does not reference our adapter {adapter_alias:?}: {stdout}",
        );
    }));

    // 4. Teardown (always runs).
    let _ = run_powershell(&format!(
        "Remove-NetRoute -DestinationPrefix '{TEST_CIDR}' \
         -InterfaceAlias '{adapter_alias}' -Confirm:$false \
         -ErrorAction SilentlyContinue"
    ))
    .await;

    if let Err(p) = assertion_outcome {
        std::panic::resume_unwind(p);
    }
}

/// Look for a Wintun adapter alias via PowerShell. Prefers our canonical
/// `zlayer-overlay` name, falls back to any adapter whose
/// InterfaceDescription mentions Wintun.
async fn find_wintun_adapter_alias() -> Option<String> {
    // Exact-name lookup first.
    let exact = run_powershell(
        "(Get-NetAdapter -Name 'zlayer-overlay' -ErrorAction SilentlyContinue).Name",
    )
    .await
    .ok()
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty());
    if let Some(a) = exact {
        return Some(a);
    }
    // Fallback: any adapter whose description includes "Wintun".
    let wintun = run_powershell(
        "(Get-NetAdapter | Where-Object { $_.InterfaceDescription -like '*Wintun*' } | \
         Select-Object -First 1).Name",
    )
    .await
    .ok()
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty());
    wintun
}

/// Run a PowerShell one-liner and return stdout. Uses `-NoProfile` so
/// user `$PROFILE` scripts cannot perturb the test output.
async fn run_powershell(command: &str) -> Result<String, String> {
    let command = command.to_string();
    tokio::task::spawn_blocking(move || {
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &command])
            .output()
            .map_err(|e| format!("failed to spawn powershell: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "powershell exit {:?}\nstdout: {}\nstderr: {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    })
    .await
    .map_err(|e| format!("join error: {e}"))?
}
