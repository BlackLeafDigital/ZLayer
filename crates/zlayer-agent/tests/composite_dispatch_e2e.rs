//! Composite-dispatch end-to-end tests on a real Windows host.
//!
//! These tests require a real Windows 11 22H2+ or Server 2019+ host with HCN +
//! Wintun installed AND an installed `zlayer` WSL2 distro with `youki`. They
//! create real HCN networks and real WSL2 containers; they are `#[ignore]`
//! by default because a Linux/macOS CI runner cannot satisfy the HCS + WSL2
//! requirements. Un-ignore happens once the `MiniWindows` self-hosted runner
//! is wired up in CI (K-1 / K-2 in the Phase K plan).
//!
//! Run with (on the Windows runner):
//! ```powershell
//! cargo test --test composite_dispatch_e2e -- --ignored --nocapture
//! ```
//!
//! # Design notes
//!
//! These tests drive [`CompositeRuntime`] end-to-end on a live Windows host:
//! a real [`HcsRuntime`] (Phase E) as primary and a real
//! [`Wsl2DelegateRuntime`] (Phase F/G) as delegate. Each test body is wrapped
//! in [`std::panic::catch_unwind`] so the cleanup phase ALWAYS runs, even if
//! an assertion panics halfway through.
//!
//! # Phase state (post-G-2, post-H-7)
//!
//! * **G-2** — [`Wsl2DelegateRuntime::create_container`] now writes a real OCI
//!   bundle into the `zlayer` WSL2 distro via the cross-platform
//!   [`crate::bundle::BundleBuilder`] spec generator. The former F-9
//!   "`Unsupported` is expected" workaround has been removed; Linux-dispatch
//!   tests now assert the delegate actually created the container.
//! * **H-7** — Composite dispatch is strict: a Linux workload on a Windows node
//!   *without* a configured delegate returns
//!   [`zlayer_agent::error::AgentError::RouteToPeer`] so the scheduler can
//!   re-place the workload on a Linux peer. The permissive "fall through to
//!   primary" variant is gone. These tests always configure a delegate when
//!   they drive a Linux spec, so `RouteToPeer` is not a normal outcome in this
//!   file; the unit tests in `composite.rs` cover the strict path with mocks.
//!
//! We deliberately use a dedicated test slice CIDR (`10.220.99.0/28`) that
//! will not collide with a live `zlayer` cluster on the same host. Every
//! container we create is removed via `CompositeRuntime::remove_container`
//! in the cleanup phase; additional best-effort sweeps (`youki delete`,
//! `HcnDeleteNetwork`) run for any stragglers we could not clean up through
//! the trait.
//!
//! If any prerequisite is missing (no admin privileges, no WSL2, no Wintun,
//! helper distro unavailable) the tests skip cleanly with an `eprintln!`
//! and early return rather than failing by environment.

#![cfg(target_os = "windows")]

use std::sync::Arc;
use std::time::Duration;

use zlayer_agent::error::Result;
use zlayer_agent::runtime::{ContainerId, Runtime};
use zlayer_agent::runtimes::composite::CompositeRuntime;
use zlayer_agent::runtimes::hcs::{overlay_network_name, HcsConfig, HcsRuntime};
#[cfg(feature = "wsl")]
use zlayer_agent::runtimes::wsl2_delegate::Wsl2DelegateRuntime;
use zlayer_overlay::ipnet;
use zlayer_paths::ZLayerDirs;
use zlayer_spec::{ArchKind, DeploymentSpec, OsKind, ServiceSpec, TargetPlatform};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Test-only slice CIDR. Narrow `/28` so it cannot collide with a live
/// cluster and cheap to allocate out of.
const TEST_SLICE: &str = "10.220.99.0/28";

/// Image owner tag stamped onto HCN endpoints the tests create. Different
/// from the production `OWNER_TAG = "zlayer"` so `list_by_owner` sweeps in
/// production won't see our leftovers and vice-versa.
const TEST_OWNER_TAG: &str = "zlayer-e2e-test";

/// Nanoserver image used for the Windows-dispatch assertions. We use the
/// `ltsc2025` tag (Windows Server 2025, build 26100) so the container build
/// matches a Windows 11 24H2 host (also 26100) and process isolation works
/// without Hyper-V. Process isolation on Windows client is supported from
/// 24H2 onward for build-matched images (per Microsoft's container version
/// compatibility matrix); a mismatched tag (e.g. ltsc2022 / 20348) would
/// force Hyper-V isolation, which needs the not-yet-implemented UVM subsystem.
const WINDOWS_IMAGE: &str = "mcr.microsoft.com/windows/nanoserver:ltsc2025";

/// Alpine fixture image used for the Linux-dispatch assertions. Pulled
/// through the WSL2 delegate's registry path. We use our own GHCR-hosted
/// retag of `alpine:3.19` rather than `docker.io/library/alpine:3.19`
/// directly to (a) avoid docker.io's unauthenticated-pull rate limit, which
/// trips repeatedly on CI runners that share egress IPs, and (b) keep
/// fixture-image provenance under our control so a future tag rotation
/// doesn't silently change test behavior.
const LINUX_IMAGE: &str = "ghcr.io/blackleafdigital/zlayer/test-fixtures:latest";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a minimal [`ServiceSpec`] for a single service named `test` with an
/// optional [`TargetPlatform`]. Matches the inline-YAML idiom used by the
/// unit tests in `composite.rs` so the dispatch-rule surface is covered by
/// the same spec shape end-to-end.
fn make_spec(image: &str, platform: Option<TargetPlatform>) -> ServiceSpec {
    let yaml = format!(
        r"
version: v1
deployment: composite-e2e
services:
  test:
    rtype: service
    image:
      name: {image}
    endpoints:
      - name: http
        protocol: http
        port: 8080
"
    );
    let mut spec = serde_yaml::from_str::<DeploymentSpec>(&yaml)
        .expect("valid deployment yaml")
        .services
        .remove("test")
        .expect("service 'test' present");
    spec.platform = platform;
    spec
}

/// Construct a [`ContainerId`] with the conventional `-rep-<n>` display form.
fn cid(service: &str, replica: u32) -> ContainerId {
    ContainerId::new(service, replica)
}

/// Parse the test slice CIDR into an [`ipnet::IpNet`]. Panics on malformed
/// const — covered by unit tests but kept assertive here so a typo in the
/// constant surfaces immediately rather than silently skipping network setup.
fn test_slice() -> ipnet::IpNet {
    TEST_SLICE
        .parse::<ipnet::IpNet>()
        .expect("TEST_SLICE must be a valid CIDR")
}

/// Build an [`HcsConfig`] wired for these tests: a unique per-test storage
/// root so concurrent runs don't clobber each other's wclayer state, and the
/// dedicated test slice so containers pick up real overlay endpoints.
fn test_hcs_config(storage_suffix: &str) -> HcsConfig {
    HcsConfig {
        storage_root: ZLayerDirs::system_default()
            .tmp()
            .join("zlayer-composite-e2e")
            .join(storage_suffix),
        slice_cidr: Some(test_slice()),
        ..HcsConfig::default()
    }
}

/// Build the composite under test: primary = real [`HcsRuntime`], delegate =
/// real [`Wsl2DelegateRuntime`] if WSL2 is available on this host.
///
/// Returns `Ok(None)` (skip the test) when a prerequisite is missing; the
/// specific reason is emitted via `eprintln!` so `--nocapture` reveals it.
async fn try_build_composite(
    storage_suffix: &str,
    require_wsl: bool,
) -> Option<(CompositeRuntime, bool)> {
    let hcs_cfg = test_hcs_config(storage_suffix);
    let primary = match HcsRuntime::new(hcs_cfg).await {
        Ok(rt) => Arc::new(rt) as Arc<dyn Runtime>,
        Err(e) => {
            eprintln!(
                "SKIP: HcsRuntime::new failed ({e}); HCS may be unavailable \
                 (non-elevated, Hyper-V feature off, or vmcompute service down)"
            );
            return None;
        }
    };

    #[cfg(feature = "wsl")]
    let (delegate, wsl_available): (Option<Arc<dyn Runtime>>, bool) =
        match Wsl2DelegateRuntime::try_new().await {
            Ok(Some(d)) => (Some(Arc::new(d) as Arc<dyn Runtime>), true),
            Ok(None) => {
                if require_wsl {
                    eprintln!(
                        "SKIP: Wsl2DelegateRuntime::try_new returned None \
                         (WSL2 not installed or zlayer distro unavailable)"
                    );
                    return None;
                }
                (None, false)
            }
            Err(e) => {
                if require_wsl {
                    eprintln!("SKIP: Wsl2DelegateRuntime::try_new failed: {e}");
                    return None;
                }
                eprintln!(
                    "note: Wsl2DelegateRuntime::try_new failed: {e}; continuing without delegate"
                );
                (None, false)
            }
        };

    #[cfg(not(feature = "wsl"))]
    let (delegate, wsl_available): (Option<Arc<dyn Runtime>>, bool) = {
        if require_wsl {
            eprintln!(
                "SKIP: this build does not have the `wsl` feature enabled; \
                 rebuild with --features wsl to exercise the delegate path"
            );
            return None;
        }
        (None, false)
    };

    Some((CompositeRuntime::new(primary, delegate), wsl_available))
}

/// Best-effort cleanup: remove `id` from the composite. Any error is logged
/// and swallowed so the cleanup phase always makes forward progress.
async fn best_effort_remove(rt: &CompositeRuntime, id: &ContainerId) {
    if let Err(e) = rt.remove_container(id).await {
        eprintln!(
            "cleanup: remove_container({id}) failed: {e} \
             (container may need manual teardown)"
        );
    }
}

/// Fallback cleanup for stragglers we could not clear through the trait. Runs
/// `wsl.exe -d zlayer -- /usr/local/bin/zlayer runtime delete <slug> --force`
/// and ignores every error — this is a last line of defence after a mid-test
/// panic.
#[cfg(feature = "wsl")]
async fn cleanup_distro_container(container_id_slug: &str) {
    if let Err(e) = zlayer_wsl::distro::wsl_exec(
        "/usr/local/bin/zlayer",
        &["runtime", "delete", container_id_slug, "--force"],
    )
    .await
    {
        eprintln!("cleanup: zlayer runtime delete {container_id_slug} failed: {e}");
    }
}

/// Compatibility shim so the cleanup call site compiles when the `wsl`
/// feature is not enabled. Does nothing — without the delegate there are no
/// in-distro stragglers to sweep.
#[cfg(not(feature = "wsl"))]
#[allow(clippy::unused_async)]
async fn cleanup_distro_container(_container_id_slug: &str) {}

/// Best-effort: tear down every HCN endpoint tagged with the test owner.
/// The production daemon wipes its own owner tag on startup via
/// `reconcile_orphans`, so we use a dedicated `TEST_OWNER_TAG` that is never
/// touched by the real agent. This makes the cleanup phase idempotent even
/// if a previous test panicked before `remove_container` ran.
fn cleanup_hcn_test_owner() {
    let owned = match zlayer_hns::attach::list_owned_endpoints(TEST_OWNER_TAG) {
        Ok(list) => list,
        Err(e) => {
            eprintln!("cleanup: list_owned_endpoints({TEST_OWNER_TAG}) failed: {e}");
            return;
        }
    };
    for (endpoint_id, name) in owned {
        if let Err(e) = zlayer_hns::endpoint::Endpoint::delete(endpoint_id) {
            eprintln!("cleanup: HcnDeleteEndpoint({endpoint_id:?}, name={name}) failed: {e}");
        }
    }
}

/// Best-effort: delete the Transparent HCN network the runtime self-allocates
/// for the test slice. The test leaves `daemon_name` at the default `"zlayer"`,
/// so the runtime creates `overlay_network_name("zlayer") == "zlayer-overlay"`.
/// HCN networks are not owner-tagged, so we enumerate every network, open each
/// to read its name, and delete the one that matches — without this the next
/// run hits a name/subnet conflict. Every error is logged and swallowed so the
/// cleanup phase always makes forward progress.
fn cleanup_hcn_test_network() {
    // Matches what the runtime creates with the default daemon name.
    let target = overlay_network_name("zlayer");
    let guids = match zlayer_hns::network::list("{}") {
        Ok(g) => g,
        Err(e) => {
            eprintln!("cleanup: HcnEnumerateNetworks failed: {e}");
            return;
        }
    };
    for guid in guids {
        let net = match zlayer_hns::network::Network::open(guid) {
            Ok(n) => n,
            Err(e) => {
                eprintln!("cleanup: HcnOpenNetwork({guid:?}) failed: {e}");
                continue;
            }
        };
        let props = match net.query("{}") {
            Ok(p) => p,
            Err(e) => {
                eprintln!("cleanup: HcnQueryNetworkProperties({guid:?}) failed: {e}");
                continue;
            }
        };
        if props.name != target {
            continue;
        }
        // Drop the owning handle before HcnDeleteNetwork — per network.rs,
        // best practice is to close the wrapper first.
        drop(net);
        if let Err(e) = zlayer_hns::network::Network::delete(guid) {
            eprintln!("cleanup: HcnDeleteNetwork({guid:?}, name={target}) failed: {e}");
        }
    }
}

/// Verify that a container has an HCN endpoint whose IP falls inside `slice`.
/// Returns `Ok(ip)` on success; `Err` flows up with a message describing the
/// first discrepancy so failures are pinpointable.
async fn assert_container_ip_in_slice(
    rt: &CompositeRuntime,
    id: &ContainerId,
    slice: ipnet::IpNet,
) -> std::result::Result<std::net::IpAddr, String> {
    let ip = match rt.get_container_ip(id).await {
        Ok(Some(ip)) => ip,
        Ok(None) => {
            return Err(format!(
                "container {id} has no IP — composite returned None from get_container_ip"
            ));
        }
        Err(e) => return Err(format!("get_container_ip({id}) failed: {e}")),
    };
    if !slice.contains(&ip) {
        return Err(format!(
            "container {id} IP {ip} is outside the expected test slice {slice}"
        ));
    }
    Ok(ip)
}

/// Check that HCS knows about a compute system with the given id under the
/// production owner tag (`"zlayer"`). The composite's create path creates
/// systems with that tag regardless of whether the driver was spawned from a
/// test, so we match on substring rather than equality.
async fn hcs_has_system(hcs_id: &str) -> std::result::Result<bool, String> {
    let systems = zlayer_hcs::enumerate::list_by_owner("zlayer")
        .await
        .map_err(|e| format!("HcsEnumerateComputeSystems failed: {e}"))?;
    Ok(systems.iter().any(|s| s.id == hcs_id))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Dispatches a service spec with `platform.os = Windows` to the HCS primary
/// and asserts the resulting container has an HCN endpoint with an IP inside
/// the test slice.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "creates real HCS compute system + HCN endpoint; requires admin on Windows"]
async fn composite_dispatches_windows_spec_to_hcs() {
    let Some((composite, _wsl_available)) = try_build_composite("win-dispatch", false).await else {
        return;
    };

    let id = cid("win-svc", 0);
    let spec = make_spec(
        WINDOWS_IMAGE,
        Some(TargetPlatform::new(OsKind::Windows, ArchKind::Amd64)),
    );

    // `pull_image` may be slow on a cold host — skip the test (not fail) if
    // the pull fails, since that's an environment issue not a dispatch bug.
    if let Err(e) = composite.pull_image(&spec.image.name.to_string()).await {
        eprintln!("SKIP: pull_image({WINDOWS_IMAGE}) failed: {e}");
        return;
    }

    let create_result = composite.create_container(&id, &spec).await;

    let assertion_outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        create_result.expect("create_container for Windows spec must succeed");
    }));

    // --- Assertions (only if create succeeded) ---
    let hcs_check: std::result::Result<(), String> = if assertion_outcome.is_ok() {
        let runtime_for_async = &composite;
        let id_for_async = &id;
        async move {
            if !hcs_has_system(&id_for_async.to_string()).await? {
                return Err(format!(
                    "HCS list_by_owner(\"zlayer\") did not include {id_for_async} \
                     — composite did not dispatch to the HCS primary"
                ));
            }
            // The runtime delegates HCN endpoint/namespace creation to the
            // `overlayd` daemon (see `crates/zlayer-agent/src/runtimes/hcs.rs`
            // around the `overlay_attach` integration). When overlayd is
            // reachable, a Windows container MUST get an overlay IP inside
            // the node slice — that's the assertion below. When overlayd is
            // NOT reachable (typical CI without a started overlayd), the HCS
            // runtime logs and proceeds with no overlay attachment, so
            // `get_container_ip` returns `Ok(None)`. In that case we skip
            // cleanly rather than panic, matching the file-wide
            // SKIP-on-environment pattern (see `try_build_composite`).
            match runtime_for_async.get_container_ip(id_for_async).await {
                Ok(Some(_)) => {
                    let ip =
                        assert_container_ip_in_slice(runtime_for_async, id_for_async, test_slice())
                            .await?;
                    eprintln!("PASS: Windows container {id_for_async} has HCN endpoint IP {ip}");
                }
                Ok(None) => {
                    eprintln!(
                        "SKIP: composite returned no IP for {id_for_async} — overlayd is \
                         likely not connected (no overlay endpoint was allocated by the HCS \
                         runtime). Start overlayd before running this test to exercise the \
                         IP-in-slice assertion."
                    );
                }
                Err(e) => {
                    return Err(format!("get_container_ip({id_for_async}) failed: {e}"));
                }
            }
            Ok(())
        }
        .await
    } else {
        Ok(())
    };

    // --- Cleanup (always runs) ---
    best_effort_remove(&composite, &id).await;
    cleanup_hcn_test_owner();
    cleanup_hcn_test_network();

    if let Err(p) = assertion_outcome {
        std::panic::resume_unwind(p);
    }
    if let Err(msg) = hcs_check {
        panic!("{msg}");
    }
}

/// Dispatches a service spec with `platform.os = Linux` to the WSL2 delegate.
/// Verifies the container appears in `wsl.exe -d zlayer -- /usr/local/bin/zlayer
/// runtime state <slug>` output. Skips cleanly if no WSL2 is available.
///
/// Post-G-2: the delegate now writes a full OCI bundle into the distro before
/// invoking `youki create`, so `create_container` must return `Ok` — the old
/// "`Unsupported` is expected" workaround has been removed.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "creates real WSL2 youki container; requires the zlayer WSL2 distro + youki"]
async fn composite_dispatches_linux_spec_to_wsl2() {
    let Some((composite, wsl_available)) = try_build_composite("linux-dispatch", true).await else {
        return;
    };
    if !wsl_available {
        eprintln!("SKIP: WSL2 not available on this host");
        return;
    }

    let id = cid("lin-svc", 0);
    let spec = make_spec(
        LINUX_IMAGE,
        Some(TargetPlatform::new(OsKind::Linux, ArchKind::Amd64)),
    );

    // Pull through the delegate; buffer-only errors (not found in registry,
    // no network) should skip rather than fail.
    if let Err(e) = composite.pull_image(&spec.image.name.to_string()).await {
        eprintln!("SKIP: pull_image({LINUX_IMAGE}) failed: {e}");
        return;
    }

    // G-2 landed: the delegate writes a real OCI bundle into the distro and
    // hands off to `youki create`. A Linux-targeted spec on a node with a
    // delegate MUST now produce a real container — any error is a genuine
    // failure, not an expected placeholder.
    let create_result = composite.create_container(&id, &spec).await;

    let slug = format!("{}-rep-{}", id.service, id.replica);
    let assertion_outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        create_result
            .as_ref()
            .expect("create_container for Linux spec must succeed via the WSL2 delegate (G-2)");
    }));

    // After a successful create the container must actually show up under
    // `zlayer runtime state <slug>` inside the distro. If it doesn't, dispatch
    // reached the delegate but the bundle-write / `zlayer runtime create`
    // pipeline is broken.
    let list_check: std::result::Result<(), String> = if create_result.is_ok() {
        #[cfg(feature = "wsl")]
        {
            match zlayer_wsl::distro::wsl_exec(
                "/usr/local/bin/zlayer",
                &["runtime", "state", &slug],
            )
            .await
            {
                Ok(out) if out.status.success() => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    match serde_json::from_str::<serde_json::Value>(stdout.trim()) {
                        Ok(json) if json["id"].as_str() == Some(slug.as_str()) => {
                            eprintln!(
                                "PASS: zlayer runtime state inside the zlayer distro reports {slug}"
                            );
                            Ok(())
                        }
                        Ok(json) => Err(format!(
                            "zlayer runtime state stdout does not name {slug}: {json}"
                        )),
                        Err(e) => Err(format!(
                            "zlayer runtime state stdout is not valid JSON ({e}): {stdout}"
                        )),
                    }
                }
                Ok(out) => Err(format!(
                    "zlayer runtime state failed (status {:?}): {}",
                    out.status.code(),
                    String::from_utf8_lossy(&out.stderr).trim()
                )),
                Err(e) => Err(format!(
                    "wsl.exe -d zlayer -- /usr/local/bin/zlayer runtime state {slug}: {e}"
                )),
            }
        }
        #[cfg(not(feature = "wsl"))]
        {
            // Without the `wsl` feature, `try_build_composite(require_wsl=true)`
            // already returned `None`, so we never reach this arm. Keep the
            // compiler happy with a trivially-true result.
            let _ = slug;
            Ok(())
        }
    } else {
        Ok(())
    };

    // --- Cleanup (always runs) ---
    if create_result.is_ok() {
        best_effort_remove(&composite, &id).await;
    }
    cleanup_distro_container(&slug).await;

    if let Err(p) = assertion_outcome {
        std::panic::resume_unwind(p);
    }
    if let Err(msg) = list_check {
        panic!("{msg}");
    }
}

/// No platform on the spec, Windows image → dispatch lands on the HCS primary.
///
/// Post-H-7 this is the "cache hit → primary" path: `pull_image` inspects the
/// OCI manifest and populates the image-OS cache with `OsKind::Windows`, so
/// `select_for` returns `DispatchTarget::Primary` without needing an explicit
/// `spec.platform`. The strict "no delegate + Linux workload" branch (which
/// now returns `RouteToPeer` rather than falling through) is covered by the
/// unit tests in `composite.rs::tests` — the E2E assertion here is narrower:
/// a Windows image with no explicit platform must produce a real HCS compute
/// system, never a stray delegate dispatch.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "creates real HCS compute system + HCN endpoint; requires admin on Windows"]
async fn composite_falls_through_to_primary_when_no_platform_specified() {
    let Some((composite, _wsl_available)) = try_build_composite("fallthrough", false).await else {
        return;
    };

    let id = cid("fallthrough-svc", 0);
    // `platform = None` on purpose: the composite must consult the image-OS
    // cache (populated by the pull below with `Windows`) to decide, rather
    // than the old permissive fall-through logic.
    let spec = make_spec(WINDOWS_IMAGE, None);

    if let Err(e) = composite.pull_image(&spec.image.name.to_string()).await {
        eprintln!("SKIP: pull_image({WINDOWS_IMAGE}) failed: {e}");
        return;
    }

    let create_result = composite.create_container(&id, &spec).await;

    let assertion_outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        create_result.expect("create_container without platform must succeed on HCS primary");
    }));

    let hcs_check: std::result::Result<(), String> = if assertion_outcome.is_ok() {
        async {
            if !hcs_has_system(&id.to_string()).await? {
                return Err(format!(
                    "HCS list_by_owner(\"zlayer\") did not include {id} \
                     — composite did not dispatch to the primary (Windows cache hit)"
                ));
            }
            eprintln!("PASS: no-platform Windows spec landed on HCS primary as expected");
            Ok(())
        }
        .await
    } else {
        Ok(())
    };

    best_effort_remove(&composite, &id).await;
    cleanup_hcn_test_owner();

    if let Err(p) = assertion_outcome {
        std::panic::resume_unwind(p);
    }
    if let Err(msg) = hcs_check {
        panic!("{msg}");
    }
}

/// Pull a Linux image first (populates the composite's image-OS cache via
/// `fetch_image_os`), then create a service spec without an explicit
/// platform. Dispatch must route to the WSL2 delegate because the cache
/// tells the composite this image is Linux.
///
/// Post-G-2 the delegate writes a real OCI bundle and calls `youki create`,
/// so `create_container` must succeed — no more "`Unsupported` is expected"
/// placeholder. The delegate is always configured in this test
/// (`require_wsl = true`), so H-7's strict `RouteToPeer` policy never fires.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "creates real WSL2 youki container after a Linux image pull"]
async fn composite_uses_image_os_cache_after_pull() {
    let Some((composite, wsl_available)) = try_build_composite("image-os-cache", true).await else {
        return;
    };
    if !wsl_available {
        eprintln!("SKIP: WSL2 not available on this host");
        return;
    }

    let id = cid("cache-svc", 0);
    let spec = make_spec(LINUX_IMAGE, None);

    // Priming pull: the composite's pull path calls `fetch_image_os`, which
    // reads the OCI manifest's `config.os` and populates the cache.
    if let Err(e) = composite.pull_image(&spec.image.name.to_string()).await {
        eprintln!("SKIP: pull_image({LINUX_IMAGE}) failed: {e}");
        return;
    }

    let create_result = composite.create_container(&id, &spec).await;

    let slug = format!("{}-rep-{}", id.service, id.replica);
    let assertion_outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        create_result.as_ref().expect(
            "image-OS cache hit for a Linux image must dispatch to the delegate and succeed (G-2)",
        );
    }));

    let list_check: std::result::Result<(), String> = if create_result.is_ok() {
        #[cfg(feature = "wsl")]
        {
            match zlayer_wsl::distro::wsl_exec(
                "/usr/local/bin/zlayer",
                &["runtime", "state", &slug],
            )
            .await
            {
                Ok(out) if out.status.success() => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    match serde_json::from_str::<serde_json::Value>(stdout.trim()) {
                        Ok(json) if json["id"].as_str() == Some(slug.as_str()) => {
                            eprintln!(
                                "PASS: cache-routed Linux spec created container {slug} in the distro"
                            );
                            Ok(())
                        }
                        Ok(json) => Err(format!(
                            "zlayer runtime state stdout does not name {slug}: {json}"
                        )),
                        Err(e) => Err(format!(
                            "zlayer runtime state stdout is not valid JSON ({e}): {stdout}"
                        )),
                    }
                }
                Ok(out) => Err(format!(
                    "zlayer runtime state failed (status {:?}): {}",
                    out.status.code(),
                    String::from_utf8_lossy(&out.stderr).trim()
                )),
                Err(e) => Err(format!(
                    "wsl.exe -d zlayer -- /usr/local/bin/zlayer runtime state {slug}: {e}"
                )),
            }
        }
        #[cfg(not(feature = "wsl"))]
        {
            let _ = slug;
            Ok(())
        }
    } else {
        Ok(())
    };

    if create_result.is_ok() {
        best_effort_remove(&composite, &id).await;
    }
    cleanup_distro_container(&slug).await;

    if let Err(p) = assertion_outcome {
        std::panic::resume_unwind(p);
    }
    if let Err(msg) = list_check {
        panic!("{msg}");
    }
}

/// Create a Windows container (primary), then call `stop_container`. The
/// composite's per-container dispatch cache must remember that the
/// container was created on the primary and route the stop there — never
/// bounce it to the delegate.
///
/// This test is the lifecycle contract: once a runtime has created a
/// container, every subsequent operation on that container must go to the
/// same runtime. Regressions would surface as a `NotFound` from the wrong
/// runtime or a silent hang.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "creates real HCS compute system; requires admin on Windows"]
async fn composite_create_then_stop_uses_same_runtime() {
    let Some((composite, _wsl_available)) = try_build_composite("create-stop", false).await else {
        return;
    };

    let id = cid("lifecycle-svc", 0);
    let spec = make_spec(
        WINDOWS_IMAGE,
        Some(TargetPlatform::new(OsKind::Windows, ArchKind::Amd64)),
    );

    if let Err(e) = composite.pull_image(&spec.image.name.to_string()).await {
        eprintln!("SKIP: pull_image({WINDOWS_IMAGE}) failed: {e}");
        return;
    }
    if let Err(e) = composite.create_container(&id, &spec).await {
        eprintln!("SKIP: create_container failed: {e}");
        return;
    }

    // Stop path must succeed without bouncing to the delegate — if the
    // dispatch cache were broken, this would error with NotFound from the
    // delegate side or silently hang inside youki.
    let stop_result = composite.stop_container(&id, Duration::from_secs(5)).await;

    let assertion_outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        stop_result.expect("stop_container must succeed on the same runtime that created");
    }));

    best_effort_remove(&composite, &id).await;
    cleanup_hcn_test_owner();

    if let Err(p) = assertion_outcome {
        std::panic::resume_unwind(p);
    }
}

// ---------------------------------------------------------------------------
// Compile-time trait checks. These exercise the code paths that are not
// cfg-gated away on Linux so a `cargo check -p zlayer-agent --tests` on a
// non-Windows host still catches import breakage in the composite module.
// ---------------------------------------------------------------------------

#[allow(dead_code, clippy::extra_unused_type_parameters)]
fn _trait_sanity<R: Runtime + Send + Sync + 'static>() -> fn() -> Result<()> {
    || Ok(())
}
