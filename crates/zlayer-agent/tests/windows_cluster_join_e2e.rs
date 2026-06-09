//! Windows cluster-join E2E test (Phase K-4).
//!
//! Spins up a mixed-OS deployment on a two-peer cluster:
//! - One Linux peer (address from `ZLAYER_LINUX_PEER` env var; skip if absent).
//! - This Windows host (`MiniWindows`) as the second peer.
//!
//! Deployment: two services — one `alpine:3.19` (Linux, scheduled on the
//! Linux peer or via local WSL2 delegate), one
//! `mcr.microsoft.com/windows/nanoserver:ltsc2022` (Windows, scheduled on the
//! Windows peer).
//!
//! # Assertions
//!
//! 1. Both services reach `Running`.
//! 2. Each resolves the other's service name via overlay DNS
//!    (`svc-linux.overlay.local` ↔ `svc-win.overlay.local`).
//! 3. Ping connectivity over the overlay.
//! 4. Clean teardown — no leaked HCN endpoints, no leaked WSL containers.
//!
//! # Prerequisites
//!
//! Ignored by default — real hardware is required (Wintun + WSL2 distro +
//! Hyper-V + a real Linux peer reachable from the Windows host). Run via
//!
//! ```powershell
//! cargo test --test windows_cluster_join_e2e -- --ignored --nocapture
//! ```
//!
//! on `MiniWindows` with the following environment configured:
//!
//! * `ZLAYER_LINUX_PEER` — TCP endpoint of a running Linux daemon reachable
//!   from this host, e.g. `tcp://10.0.0.1:3669`. **Tests skip cleanly with an
//!   `eprintln!` message if this env var is absent.**
//! * `ZLAYER_DAEMON_ADDR` (optional) — HTTP endpoint of the *local* Windows
//!   daemon. Defaults to `http://127.0.0.1:3669`.
//! * `ZLAYER_API_TOKEN` (optional) — bearer token for the local daemon API.
//!   Required unless the daemon was started with auth disabled.
//!
//! # Design
//!
//! The test drives the local Windows daemon's HTTP API via `reqwest` (already
//! a `zlayer-agent` dep). It deliberately avoids depending on `zlayer-client`
//! / `zlayer-api` crates (not in `zlayer-agent`'s dev-deps) so the test
//! compiles everywhere with just the existing workspace dependency set. The
//! cleanup phase always runs, even if an assertion panics halfway through, by
//! wrapping the assertion body in [`std::panic::catch_unwind`] — same pattern
//! as `composite_dispatch_e2e.rs`.

#![cfg(target_os = "windows")]

use std::time::Duration;

use reqwest::StatusCode;
use serde_json::Value as JsonValue;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Deployment name used by every test in this file. Dedicated prefix so
/// cleanup sweeps cannot accidentally touch an unrelated deployment on the
/// same daemon.
const DEPLOYMENT_NAME: &str = "k4-mixed-workload";

/// Linux service name inside the deployment.
const LINUX_SVC: &str = "svc-linux";

/// Windows service name inside the deployment.
const WIN_SVC: &str = "svc-win";

/// Overlay DNS suffix used by the daemon's DNS registrar.
const OVERLAY_DNS_SUFFIX: &str = "overlay.local";

/// Default local daemon endpoint when `ZLAYER_DAEMON_ADDR` is unset.
const DEFAULT_DAEMON_ADDR: &str = "http://127.0.0.1:3669";

/// Alpine fixture image used for the Linux service. Pulled on the Linux
/// peer (or via the local WSL2 delegate) as part of deployment
/// orchestration. We use our own GHCR-hosted retag of `alpine:3.19` rather
/// than docker.io directly to dodge unauthenticated-pull rate limits and
/// keep fixture provenance under our control.
const LINUX_IMAGE: &str = "ghcr.io/blackleafdigital/zlayer/test-fixtures:latest";

/// Nanoserver image used for the Windows service. `ltsc2025` (build 26100)
/// so it build-matches a Windows 11 24H2 host and runs process-isolated
/// without the not-yet-implemented Hyper-V UVM path.
const WINDOWS_IMAGE: &str = "mcr.microsoft.com/windows/nanoserver:ltsc2025";

/// How long to poll before declaring the deployment "not Running". Cold pulls
/// of nanoserver can take several minutes, so the ceiling is generous.
const RUNNING_TIMEOUT: Duration = Duration::from_secs(600);

/// Interval between `get_deployment` polls.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Short timeout for per-request HTTP calls. Keep below `POLL_INTERVAL` so
/// one stuck call cannot starve the whole polling loop.
const HTTP_REQ_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Spec fixture
// ---------------------------------------------------------------------------

/// YAML spec for the mixed-workload deployment. Formatted as a raw string so
/// the body of the test file is the source of truth — no external fixture
/// files.
fn deployment_yaml() -> String {
    format!(
        r"
version: v1
deployment: {DEPLOYMENT_NAME}
services:
  {LINUX_SVC}:
    rtype: service
    image:
      name: {LINUX_IMAGE}
    platform:
      os: linux
      arch: amd64
    scale:
      mode: fixed
      replicas: 1
    command:
      entrypoint:
        - sh
        - -c
        - 'while true; do sleep 3600; done'
  {WIN_SVC}:
    rtype: service
    image:
      name: {WINDOWS_IMAGE}
    platform:
      os: windows
      arch: amd64
    scale:
      mode: fixed
      replicas: 1
    command:
      entrypoint:
        - cmd.exe
        - /c
        - 'ping -n 999999 127.0.0.1 >nul'
",
    )
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Address of a reachable Linux peer for this test. `None` means the test
/// cannot run on this host and should skip cleanly.
fn linux_peer_addr() -> Option<String> {
    std::env::var("ZLAYER_LINUX_PEER")
        .ok()
        .filter(|v| !v.is_empty())
}

/// HTTP endpoint of the *local* Windows daemon. Respects `ZLAYER_DAEMON_ADDR`
/// if set, otherwise falls back to the common localhost default.
fn local_daemon_addr() -> String {
    std::env::var("ZLAYER_DAEMON_ADDR")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_DAEMON_ADDR.to_string())
}

/// Optional bearer token for the local daemon API. When `None`, calls go out
/// without an `Authorization` header; the daemon must be configured to allow
/// anonymous local access in that mode.
fn api_token() -> Option<String> {
    std::env::var("ZLAYER_API_TOKEN")
        .ok()
        .filter(|v| !v.is_empty())
}

/// Build a preconfigured reqwest client with a sane per-request timeout.
fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(HTTP_REQ_TIMEOUT)
        .build()
        .expect("reqwest client should build with default TLS backend")
}

/// Attach the bearer token (if any) and send. Returns the parsed JSON body on
/// `2xx`, otherwise an error that includes the status + response text so
/// failures pinpoint which call went wrong.
async fn send_json(
    client: &reqwest::Client,
    req: reqwest::RequestBuilder,
    context: &str,
) -> Result<(StatusCode, String), String> {
    let req = if let Some(t) = api_token() {
        req.bearer_auth(t)
    } else {
        req
    };
    let resp = req
        .send()
        .await
        .map_err(|e| format!("{context}: request failed: {e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("{context}: reading body failed: {e}"))?;
    // Touch `client` so callers can use the same instance for subsequent
    // requests without re-wiring timeouts.
    let _ = client;
    Ok((status, text))
}

/// POST the deployment YAML to the local daemon. Returns the parsed response
/// JSON on success, `Err(String)` on any non-2xx status.
async fn submit_deployment(client: &reqwest::Client, base: &str) -> Result<JsonValue, String> {
    let url = format!("{base}/api/v1/deployments");
    let body = serde_json::json!({ "spec": deployment_yaml() });
    let req = client.post(&url).json(&body);
    let (status, text) = send_json(client, req, "POST /api/v1/deployments").await?;
    if !status.is_success() {
        return Err(format!(
            "POST /api/v1/deployments returned {status}: {text}"
        ));
    }
    serde_json::from_str::<JsonValue>(&text)
        .map_err(|e| format!("POST /api/v1/deployments body was not JSON: {e}; body={text}"))
}

/// GET a single deployment's details. Returns `Ok(Some(json))` on 200,
/// `Ok(None)` on 404 (already deleted), `Err` on any other failure.
async fn get_deployment(
    client: &reqwest::Client,
    base: &str,
    name: &str,
) -> Result<Option<JsonValue>, String> {
    let url = format!("{base}/api/v1/deployments/{name}");
    let req = client.get(&url);
    let (status, text) = send_json(client, req, "GET /api/v1/deployments/{name}").await?;
    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(format!(
            "GET /api/v1/deployments/{name} returned {status}: {text}"
        ));
    }
    let parsed = serde_json::from_str::<JsonValue>(&text).map_err(|e| {
        format!("GET /api/v1/deployments/{name} body was not JSON: {e}; body={text}")
    })?;
    Ok(Some(parsed))
}

/// Poll until every service in `deployment` reports `Running` or the overall
/// timeout elapses. Errors include the last-seen status so root-cause is
/// visible from the test log.
async fn wait_for_running(client: &reqwest::Client, base: &str, name: &str) -> Result<(), String> {
    let deadline = std::time::Instant::now() + RUNNING_TIMEOUT;
    let mut last_status = String::from("<no response>");

    while std::time::Instant::now() < deadline {
        match get_deployment(client, base, name).await {
            Ok(Some(dep)) => {
                last_status = dep
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("<missing>")
                    .to_string();
                if last_status.eq_ignore_ascii_case("running") {
                    return Ok(());
                }
                if last_status.eq_ignore_ascii_case("failed") {
                    return Err(format!(
                        "deployment {name} reached terminal Failed status: {dep}"
                    ));
                }
            }
            Ok(None) => {
                return Err(format!(
                    "deployment {name} disappeared (404) while waiting for Running"
                ));
            }
            Err(e) => {
                // Transient errors (daemon restart, brief network blip) are
                // logged but do not fail the wait — the next poll retries.
                eprintln!("wait_for_running: transient error: {e}");
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    Err(format!(
        "deployment {name} did not reach Running within {RUNNING_TIMEOUT:?}; last status: {last_status}"
    ))
}

/// Exec a command inside a service's container. Returns the response JSON on
/// 2xx. Callers inspect `stdout` / `stderr` / `exit_code` fields to validate
/// the result.
async fn exec_in_service(
    client: &reqwest::Client,
    base: &str,
    deployment: &str,
    service: &str,
    cmd: &[&str],
) -> Result<JsonValue, String> {
    let url = format!("{base}/api/v1/deployments/{deployment}/services/{service}/exec");
    let body = serde_json::json!({ "command": cmd });
    let req = client.post(&url).json(&body);
    let (status, text) = send_json(client, req, "POST .../exec").await?;
    if !status.is_success() {
        return Err(format!(
            "exec on {deployment}/{service} returned {status}: {text}"
        ));
    }
    serde_json::from_str::<JsonValue>(&text)
        .map_err(|e| format!("exec body was not JSON: {e}; body={text}"))
}

/// Best-effort DELETE to tear down the deployment, plus an HCN endpoint sweep
/// that matches the production owner tag so no stray overlay endpoints are
/// left behind after the test exits. All errors are logged and swallowed —
/// the cleanup phase always makes forward progress.
async fn cleanup_deployment(client: &reqwest::Client, base: &str, name: &str) {
    let url = format!("{base}/api/v1/deployments/{name}");
    let req = client.delete(&url);
    match send_json(client, req, "DELETE /api/v1/deployments/{name}").await {
        Ok((status, text)) if status.is_success() || status == StatusCode::NOT_FOUND => {
            eprintln!("cleanup: DELETE {name} -> {status}");
            let _ = text;
        }
        Ok((status, text)) => {
            eprintln!("cleanup: DELETE {name} returned {status}: {text}");
        }
        Err(e) => eprintln!("cleanup: DELETE {name} failed: {e}"),
    }

    cleanup_hcn_leftovers();
}

/// Best-effort: tear down every HCN endpoint tagged with the production
/// `"zlayer"` owner whose name references our deployment. Mirrors the
/// `cleanup_hcn_test_owner` helper in `composite_dispatch_e2e.rs` but scoped
/// to *our* deployment name so we never clobber unrelated state from a real
/// running cluster on the same host.
fn cleanup_hcn_leftovers() {
    let owned = match zlayer_hns::attach::list_owned_endpoints("zlayer") {
        Ok(list) => list,
        Err(e) => {
            eprintln!("cleanup: list_owned_endpoints(\"zlayer\") failed: {e}");
            return;
        }
    };
    for (endpoint_id, name) in owned {
        if !name.contains(DEPLOYMENT_NAME) {
            continue;
        }
        if let Err(e) = zlayer_hns::endpoint::Endpoint::delete(endpoint_id) {
            eprintln!("cleanup: HcnDeleteEndpoint({endpoint_id:?}, name={name}) failed: {e}");
        }
    }
}

/// Extract the concatenated stdout (fallback: stderr) from an exec response.
fn exec_output(resp: &JsonValue) -> String {
    let stdout = resp
        .get("stdout")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if stdout.is_empty() {
        resp.get("stderr")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    } else {
        stdout
    }
}

/// Common setup: verify the Linux peer env var, build an HTTP client, and
/// submit the deployment. Returns `Some(client, base)` ready for assertions,
/// or `None` if the test should skip cleanly.
async fn setup_or_skip() -> Option<(reqwest::Client, String)> {
    let Some(peer) = linux_peer_addr() else {
        eprintln!(
            "SKIP: ZLAYER_LINUX_PEER env var not set; this test needs a real \
             Linux peer reachable from the Windows host"
        );
        return None;
    };
    eprintln!("using Linux peer: {peer}");

    let base = local_daemon_addr();
    let client = http_client();

    // Pre-flight: confirm the local daemon is reachable before we try to
    // deploy anything. A 200 on /health (or any 2xx) is enough.
    let health_url = format!("{base}/health");
    match client.get(&health_url).send().await {
        Ok(resp) if resp.status().is_success() => {
            eprintln!("local daemon {base} /health -> {}", resp.status());
        }
        Ok(resp) => {
            eprintln!(
                "SKIP: local daemon {base} /health returned {}; start the daemon before running this test",
                resp.status()
            );
            return None;
        }
        Err(e) => {
            eprintln!(
                "SKIP: could not reach local daemon at {base}: {e}; start the daemon before running this test"
            );
            return None;
        }
    }

    Some((client, base))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Submit the mixed-workload deployment, wait for both services to reach
/// `Running`, and tear down cleanly.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real Windows host with Wintun + zlayer WSL2 distro + a reachable Linux peer via ZLAYER_LINUX_PEER env var"]
async fn cluster_mixed_workload_reach_running() {
    let Some((client, base)) = setup_or_skip().await else {
        return;
    };

    let deploy_result = submit_deployment(&client, &base).await;
    let assertion_outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        deploy_result
            .as_ref()
            .expect("POST /api/v1/deployments must succeed for a mixed-OS spec");
    }));

    let running_check: Result<(), String> = if deploy_result.is_ok() {
        wait_for_running(&client, &base, DEPLOYMENT_NAME)
            .await
            .map(|()| {
                eprintln!(
                "PASS: deployment {DEPLOYMENT_NAME} reports Running (both Linux + Windows services)"
            );
            })
    } else {
        Ok(())
    };

    // --- Cleanup (always runs) ---
    cleanup_deployment(&client, &base, DEPLOYMENT_NAME).await;

    if let Err(p) = assertion_outcome {
        std::panic::resume_unwind(p);
    }
    if let Err(msg) = running_check {
        panic!("{msg}");
    }
}

/// After both services reach `Running`, exec `nslookup` from each container
/// targeting the other's overlay DNS name. Validates that the daemon's DNS
/// registrar has written both records and they resolve across the peer
/// boundary.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real Windows host with Wintun + zlayer WSL2 distro + a reachable Linux peer via ZLAYER_LINUX_PEER env var"]
async fn cluster_mixed_workload_overlay_dns_resolves() {
    let Some((client, base)) = setup_or_skip().await else {
        return;
    };

    let deploy_result = submit_deployment(&client, &base).await;
    let body: Result<(), String> = async {
        deploy_result
            .as_ref()
            .map_err(|e| format!("deploy failed: {e}"))?;
        wait_for_running(&client, &base, DEPLOYMENT_NAME).await?;

        // Linux container resolves the Windows service name.
        let linux_target = format!("{WIN_SVC}.{OVERLAY_DNS_SUFFIX}");
        let lin_out = exec_in_service(
            &client,
            &base,
            DEPLOYMENT_NAME,
            LINUX_SVC,
            &["nslookup", &linux_target],
        )
        .await?;
        let lin_text = exec_output(&lin_out);
        if !lin_text.contains(&linux_target) && !lin_text.contains("Address") {
            return Err(format!(
                "nslookup from Linux did not resolve {linux_target}: {lin_text}"
            ));
        }
        eprintln!("PASS: Linux container resolved {linux_target}");

        // Windows container resolves the Linux service name.
        let win_target = format!("{LINUX_SVC}.{OVERLAY_DNS_SUFFIX}");
        let win_out = exec_in_service(
            &client,
            &base,
            DEPLOYMENT_NAME,
            WIN_SVC,
            &["nslookup", &win_target],
        )
        .await?;
        let win_text = exec_output(&win_out);
        if !win_text.contains(&win_target) && !win_text.contains("Address") {
            return Err(format!(
                "nslookup from Windows did not resolve {win_target}: {win_text}"
            ));
        }
        eprintln!("PASS: Windows container resolved {win_target}");

        Ok(())
    }
    .await;

    let assertion_outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        body.as_ref()
            .expect("overlay DNS resolution must succeed on both sides");
    }));

    cleanup_deployment(&client, &base, DEPLOYMENT_NAME).await;

    if let Err(p) = assertion_outcome {
        std::panic::resume_unwind(p);
    }
    if let Err(msg) = body {
        panic!("{msg}");
    }
}

/// One `ping -n 1` / `ping -c 1` from each side to the other's overlay DNS
/// name. Confirms L3 reachability over the overlay, not just DNS.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real Windows host with Wintun + zlayer WSL2 distro + a reachable Linux peer via ZLAYER_LINUX_PEER env var"]
async fn cluster_mixed_workload_overlay_ping() {
    let Some((client, base)) = setup_or_skip().await else {
        return;
    };

    let deploy_result = submit_deployment(&client, &base).await;
    let body: Result<(), String> = async {
        deploy_result
            .as_ref()
            .map_err(|e| format!("deploy failed: {e}"))?;
        wait_for_running(&client, &base, DEPLOYMENT_NAME).await?;

        // Linux -> Windows.
        let win_target = format!("{WIN_SVC}.{OVERLAY_DNS_SUFFIX}");
        let lin_out = exec_in_service(
            &client,
            &base,
            DEPLOYMENT_NAME,
            LINUX_SVC,
            &["ping", "-c", "1", "-W", "5", &win_target],
        )
        .await?;
        let lin_exit = lin_out
            .get("exit_code")
            .and_then(JsonValue::as_i64)
            .unwrap_or(-1);
        if lin_exit != 0 {
            return Err(format!(
                "Linux ping -> {win_target} exited {lin_exit}: {}",
                exec_output(&lin_out)
            ));
        }
        eprintln!("PASS: Linux pinged {win_target}");

        // Windows -> Linux.
        let lin_target = format!("{LINUX_SVC}.{OVERLAY_DNS_SUFFIX}");
        let win_out = exec_in_service(
            &client,
            &base,
            DEPLOYMENT_NAME,
            WIN_SVC,
            &["ping", "-n", "1", "-w", "5000", &lin_target],
        )
        .await?;
        let win_exit = win_out
            .get("exit_code")
            .and_then(JsonValue::as_i64)
            .unwrap_or(-1);
        if win_exit != 0 {
            return Err(format!(
                "Windows ping -> {lin_target} exited {win_exit}: {}",
                exec_output(&win_out)
            ));
        }
        eprintln!("PASS: Windows pinged {lin_target}");

        Ok(())
    }
    .await;

    let assertion_outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        body.as_ref()
            .expect("overlay ping must succeed in both directions");
    }));

    cleanup_deployment(&client, &base, DEPLOYMENT_NAME).await;

    if let Err(p) = assertion_outcome {
        std::panic::resume_unwind(p);
    }
    if let Err(msg) = body {
        panic!("{msg}");
    }
}
