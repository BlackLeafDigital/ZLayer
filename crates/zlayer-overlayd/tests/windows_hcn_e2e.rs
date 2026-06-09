//! Windows-gated HCN E2E tests for `zlayer-overlayd`.
//!
//! These were carried forward from the agent's `windows_overlay_e2e.rs` when
//! the HCN mechanics moved into `zlayer-overlayd`. They require a real Windows
//! 10/11 or Windows Server host with HCN installed and admin privileges; they
//! create real HCN networks and endpoints and are `#[ignore]` by default.
//!
//! Style lints that don't fight the architecture are allowed: test bodies
//! exercising the full HCN setup/teardown sequence naturally exceed the
//! 100-line limit and reference `PowerShell` / `Get-NetAdapter` in prose.
#![allow(
    clippy::too_many_lines,
    clippy::doc_markdown,
    clippy::single_match_else,
    clippy::match_wild_err_arm
)]
//!
//! Run with:
//! ```powershell
//! cargo test -p zlayer-overlayd --test windows_hcn_e2e -- --ignored --nocapture
//! ```
//!
//! # Design notes
//!
//! Tests use dedicated test CIDRs (`10.221.50.0/28`, `10.221.51.0/28`) — not
//! the default cluster `10.200.0.0/16` — so they never collide with a real
//! `ZLayer` cluster running on the same host. A post-test `PowerShell` sweep is
//! recommended if a test panics mid-way; teardown always runs on success and on
//! a caught assertion failure.
//!
//! Every sync HCN call is wrapped in `tokio::task::spawn_blocking` so the
//! multi-threaded runtime does not block its reactor thread on the HCS RPC
//! round-trip.

#![cfg(target_os = "windows")]

use std::net::IpAddr;

use windows::core::GUID;

use zlayer_hns::attach::EndpointAttachment;
use zlayer_hns::network::Network;
use zlayer_hns::schema::NetworkType;

const TEST_PREFIX_LEN: u8 = 28;

/// Allocate a fresh GUID or fail the test with a clear message. HCN requires
/// GUIDs to be unique across the host, so we rely on v4 random GUIDs here.
fn new_guid() -> GUID {
    GUID::new().expect("GUID::new must succeed on a live Windows host")
}

/// **Adapter-safety proof for the Transparent→Internal change.**
///
/// Creates the default HCN **Internal** overlay network (what overlayd's
/// `ensure_overlay_network` now builds by default) plus an endpoint on it, then
/// reads the network back and asserts:
///   * `Type == Internal` (an internal vSwitch — not Transparent/L2Bridge), and
///   * it carries **no `NetAdapterName` policy** — i.e. nothing was bound to a
///     physical host NIC, so no external vSwitch was created over the
///     operator's gateway adapter.
///
/// The endpoint attach also confirms a container would get a real overlay IP on
/// the Internal segment. Pair this with a host-side `Get-NetAdapter` snapshot
/// before/after to confirm Ethernet/Wi-Fi stay untouched. Teardown always runs.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "creates a real HCN Internal network + endpoint; requires admin on Windows"]
async fn test_create_internal_network_no_physical_binding() {
    const INTERNAL_CIDR: &str = "10.221.50.0/28";
    const INTERNAL_IP: &str = "10.221.50.2";

    let net_id = new_guid();

    // --- Create the Internal network, query it straight back -------------
    let net_props = tokio::task::spawn_blocking(move || {
        let net = Network::create_internal(net_id, "zlayer-e2e-internal", INTERNAL_CIDR)?;
        // The owning handle drops at the end of this closure but the network
        // persists in HCN until `Network::delete`.
        net.query("{}")
    })
    .await
    .expect("join error")
    .expect("Network::create_internal / query must succeed (admin + Hyper-V required)");

    // HCN Static IPAM needs a moment to settle before the first endpoint.
    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;

    // --- Attach an endpoint: proves a container gets a real overlay IP ----
    let ip: IpAddr = INTERNAL_IP
        .parse()
        .expect("INTERNAL_IP is a valid IPv4 literal");
    let attach_result = tokio::task::spawn_blocking(move || {
        EndpointAttachment::create_overlay(
            net_id,
            "zlayer-test",
            "test-internal-1",
            ip,
            TEST_PREFIX_LEN,
            INTERNAL_CIDR,
            None,
            None,
        )
    })
    .await
    .expect("join error");

    let attachment = match attach_result {
        Ok(a) => a,
        Err(e) => {
            // Clean up the network before failing so a rerun is not blocked.
            let _ = tokio::task::spawn_blocking(move || Network::delete(net_id)).await;
            panic!("EndpointAttachment::create_overlay on Internal network: {e}");
        }
    };

    // The endpoint attached successfully above (`create_overlay` returned Ok) —
    // that is the proof a container gets a real overlay IP on the Internal
    // segment: HCN validates and assigns the address at `HcnCreateEndpoint`
    // time and rejects an unroutable one. HCN's query-back for a
    // namespace-attached endpoint returns a minimal object (no
    // `IPConfigurations`), so re-reading the IP here is not reliable.
    eprintln!(
        "Internal-network endpoint attached: {:?}",
        attachment.endpoint_id()
    );

    // --- Assertions (wrapped so teardown still runs on failure) ----------
    let assertion_outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Safety-critical: Internal type, no physical-NIC binding policy.
        assert!(
            matches!(net_props.ty, NetworkType::Internal),
            "network must be Internal (got {:?}) — Transparent/L2Bridge would bind a physical NIC",
            net_props.ty
        );
        for p in &net_props.policies {
            assert_ne!(
                p.get("Type").and_then(|v| v.as_str()),
                Some("NetAdapterName"),
                "Internal network must NOT carry a NetAdapterName policy (that binds a physical \
                 host NIC / creates an external vSwitch); got policies {:?}",
                net_props.policies
            );
        }
    }));

    // --- Teardown (always runs) ------------------------------------------
    let _ = tokio::task::spawn_blocking(move || attachment.teardown()).await;
    let _ = tokio::task::spawn_blocking(move || Network::delete(net_id)).await;

    if let Err(p) = assertion_outcome {
        std::panic::resume_unwind(p);
    }
}

/// **Dedicated-overlay coexistence proof.** A `OverlayMode::Dedicated` service
/// gets its OWN per-service HCN network (named `<base>-svc-<service>`) created
/// via the same `Network::create_internal` the base overlay uses. This test
/// stands up the base Internal network AND a per-service-named Internal network
/// *simultaneously* and asserts BOTH are `Internal` with NO `NetAdapterName`
/// policy — i.e. a dedicated service network coexists with the base network and
/// neither binds a physical host NIC / creates an external vSwitch. Pair with a
/// host-side `Get-NetAdapter` / `Get-VMSwitch` snapshot to confirm zero external
/// vSwitches on the gateway NICs while both networks are live.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "creates two real HCN Internal networks; requires admin on Windows"]
async fn test_dedicated_service_network_coexists_internal_only() {
    const BASE_CIDR: &str = "10.221.52.0/28";
    const SVC_CIDR: &str = "10.221.53.0/28";

    let base_id = new_guid();
    let svc_id = new_guid();

    // Bring both networks up at once: the base overlay network and a
    // per-service dedicated network using overlayd's `<base>-svc-<service>`
    // naming convention. Both go through `Network::create_internal`.
    let both = tokio::task::spawn_blocking(move || -> Result<_, String> {
        let base = Network::create_internal(base_id, "zlayer-e2e-base", BASE_CIDR)
            .map_err(|e| format!("base create_internal: {e}"))?;
        let svc = Network::create_internal(svc_id, "zlayer-e2e-base-svc-web", SVC_CIDR)
            .map_err(|e| format!("per-service create_internal: {e}"))?;
        let base_props = base.query("{}").map_err(|e| format!("base query: {e}"))?;
        let svc_props = svc.query("{}").map_err(|e| format!("svc query: {e}"))?;
        Ok((base_props, svc_props))
    })
    .await
    .expect("join error");

    let (base_props, svc_props) = match both {
        Ok(p) => p,
        Err(e) => {
            // Best-effort cleanup of whatever did get created.
            let _ = tokio::task::spawn_blocking(move || Network::delete(base_id)).await;
            let _ = tokio::task::spawn_blocking(move || Network::delete(svc_id)).await;
            panic!("coexisting Internal networks: {e}");
        }
    };

    let assertion_outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for (label, props) in [("base", &base_props), ("service", &svc_props)] {
            assert!(
                matches!(props.ty, NetworkType::Internal),
                "{label} network must be Internal (got {:?}) — Transparent/L2Bridge binds a NIC",
                props.ty
            );
            for p in &props.policies {
                assert_ne!(
                    p.get("Type").and_then(|v| v.as_str()),
                    Some("NetAdapterName"),
                    "{label} network must NOT carry a NetAdapterName policy (no physical-NIC \
                     binding / external vSwitch); got policies {:?}",
                    props.policies
                );
            }
        }
    }));

    // Teardown both, always.
    let _ = tokio::task::spawn_blocking(move || Network::delete(base_id)).await;
    let _ = tokio::task::spawn_blocking(move || Network::delete(svc_id)).await;

    if let Err(p) = assertion_outcome {
        std::panic::resume_unwind(p);
    }
}

/// **Persistent-lifecycle proof.** Verifies that
/// [`zlayer_overlayd::server::purge_managed_networks`] (the full-uninstall path)
/// deletes the HCN network recorded in the agent network marker and clears the
/// marker — i.e. the overlay network is removed only on uninstall, by the
/// marker the create path writes.
///
/// Creates a real HCN Internal network under a non-overlay name (so only the
/// marker pass, not the name-sweep, can delete it), hand-writes a marker
/// pointing at it under a temp data dir, runs the purge, then asserts the
/// network is gone and the marker file is removed.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "creates a real HCN network + marker; requires admin on Windows"]
async fn test_purge_managed_networks_deletes_recorded_network() {
    use zlayer_overlayd::network_state::{ManagedNetwork, NetworkState, OWNER_BASE};

    const CIDR: &str = "10.221.51.0/28";
    let net_name = "zlayer-e2e-purge-marker";
    let net_id = new_guid();

    // Create the network and drop the handle (the network persists in HCN
    // until an explicit delete).
    tokio::task::spawn_blocking(move || {
        let n = Network::create_internal(net_id, net_name, CIDR)
            .expect("create_internal must succeed (admin + Hyper-V required)");
        drop(n);
    })
    .await
    .expect("join error");

    // Hand-write a marker pointing at the network under a temp data dir.
    let data_dir = std::env::temp_dir().join(format!("zlayer-purge-e2e-{}", std::process::id()));
    let marker_path = data_dir.join("agent_network.json");
    let bare_id = format!("{net_id:?}")
        .trim_matches(|c: char| c == '{' || c == '}')
        .to_ascii_lowercase();
    let mut st = NetworkState::default();
    st.upsert(ManagedNetwork {
        owner: OWNER_BASE.to_string(),
        kind: "hcn-internal".to_string(),
        name: net_name.to_string(),
        id: bare_id,
        subnet: CIDR.to_string(),
        wg_port: None,
        wg_private_key: None,
        wg_public_key: None,
        interface: None,
    });
    st.save(&marker_path).expect("save marker");

    // Run the full-uninstall network purge.
    let dd = data_dir.clone();
    tokio::task::spawn_blocking(move || {
        zlayer_overlayd::server::purge_managed_networks(&dd, "zlayer");
    })
    .await
    .expect("join error");

    // The network must be gone and the marker cleared.
    let still_exists = tokio::task::spawn_blocking(move || Network::open(net_id).is_ok())
        .await
        .expect("join error");
    let marker_exists = marker_path.exists();
    let _ = std::fs::remove_dir_all(&data_dir);

    // Best-effort cleanup if the purge somehow didn't delete it.
    if still_exists {
        let _ = tokio::task::spawn_blocking(move || Network::delete(net_id)).await;
    }

    assert!(
        !still_exists,
        "purge_managed_networks must delete the marker-recorded HCN network"
    );
    assert!(
        !marker_exists,
        "purge_managed_networks must remove the marker file"
    );
}
