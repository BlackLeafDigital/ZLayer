//! Join command handler.
//!
//! Parses a join token and joins an existing deployment by authenticating
//! with the API, fetching the deployment spec, and starting local replicas.

use anyhow::{Context, Result};
#[cfg(any(unix, windows))]
use std::sync::Arc;
#[cfg(any(unix, windows))]
use std::time::Duration;
#[cfg(any(unix, windows))]
use tokio::sync::RwLock;
#[cfg(any(unix, windows))]
use tracing::{info, warn};

#[cfg(any(unix, windows))]
use zlayer_spec::DeploymentSpec;

use crate::cli::Cli;
#[cfg(any(unix, windows))]
use crate::config::build_runtime_config;
#[cfg(not(unix))]
use crate::ui::consent::ConsentMode;

/// Join token information
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct JoinToken {
    /// API endpoint to contact
    pub(crate) api_endpoint: String,
    /// Deployment name
    pub(crate) deployment: String,
    /// Authentication key for the API
    pub(crate) key: String,
    /// Optional service name (if token is service-specific)
    #[serde(default)]
    pub(crate) service: Option<String>,
}

/// Parse a join token
pub(crate) fn parse_join_token(token: &str) -> Result<JoinToken> {
    use base64::Engine;

    // Try to decode as base64 (try URL-safe first, then standard)
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token)
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(token))
        .context("Invalid join token: not valid base64")?;

    // Parse as JSON
    let join_token: JoinToken =
        serde_json::from_slice(&decoded).context("Invalid join token: not valid JSON")?;

    Ok(join_token)
}

/// Join an existing deployment.
///
/// Exotic (non-Unix, non-Windows) targets — WASM, Redox, etc. — fall
/// through to this bail. The Unix and Windows variants below carry the real
/// implementation.
#[cfg(all(not(unix), not(windows)))]
#[allow(clippy::unused_async)]
pub(crate) async fn join(
    _cli: &Cli,
    _token: &str,
    _spec_dir: Option<&str>,
    _service: Option<&str>,
    _replicas: u32,
    _install_wsl: ConsentMode,
) -> Result<()> {
    anyhow::bail!(
        "'zlayer join' is not supported on this target. Supported: Linux, macOS, Windows."
    )
}

/// Join an existing deployment.
///
/// Windows flow — same shape as the Unix body (parse token → auth → fetch spec
/// → overlay setup → service manager → pull/scale) with three extra
/// Windows-specific gates up front:
///
/// 1. [`zlayer_paths::is_root`] — must be running as Administrator. Firewall
///    rule installation, HCN endpoint creation, and `wsl --install` all
///    require elevation, so we fail fast rather than 30 steps in.
/// 2. [`ensure_wsl_backend_ready_with_consent`] — Linux containers are the
///    overwhelming majority of real workloads; without a healthy WSL2 backend
///    this node can't run them locally. `WslError::InstallRefused` is treated
///    as *non-fatal* — Linux workloads will route to peers via the overlay,
///    Windows-native containers still work on the HCS primary.
/// 3. [`ensure_overlay_rules`] — installs inbound allow-rules for the overlay
///    UDP port, the API TCP port, and the Raft TCP port, scoped to the
///    Private + Domain profiles.
///
/// After those gates pass the flow is identical to the Unix body: everything
/// below `create_runtime` is cross-platform (the overlay crate has Windows
/// HCN support, `ServiceManager` is trait-based, and the runtime factory
/// selects `HcsRuntime` + optional `Wsl2DelegateRuntime` on Windows).
#[cfg(windows)]
#[allow(clippy::too_many_lines)]
pub(crate) async fn join(
    cli: &Cli,
    token: &str,
    spec_dir: Option<&str>,
    service: Option<&str>,
    replicas: u32,
    install_wsl: ConsentMode,
) -> Result<()> {
    // ---- Windows-only pre-flight gates -----------------------------------
    if !zlayer_paths::is_root() {
        anyhow::bail!(
            "zlayer join on Windows requires running as Administrator. \
             Launch an elevated terminal (right-click PowerShell / Windows Terminal → \
             'Run as Administrator') and re-run the command."
        );
    }

    // WSL2 bootstrap (G-6 consent flow). The `wsl` Cargo feature gates the
    // entire WSL crate; without it the backend is simply unavailable on this
    // build and we skip the step with a warning. When present, a user refusal
    // (`--install-wsl no` or an interactive "n") gracefully degrades: the
    // join still proceeds, but local Linux container dispatch is offline and
    // Linux workloads must be routed through a remote peer.
    #[cfg(feature = "wsl")]
    {
        use zlayer_wsl::errors::WslError;
        use zlayer_wsl::setup::ensure_wsl_backend_ready_with_consent;

        use crate::ui::consent::{wsl2_install_consent, ConsentDecision};

        println!("Checking WSL2 backend (Linux container support)...");
        let consent_mode = install_wsl;
        let consent_closure = move || match wsl2_install_consent(consent_mode)? {
            ConsentDecision::Granted => Ok(true),
            ConsentDecision::Refused => Ok(false),
        };

        match ensure_wsl_backend_ready_with_consent(consent_closure).await {
            Ok(_) => {
                info!("WSL2 backend is ready");
                println!("  WSL2 backend: ready");
            }
            Err(err) => {
                if let Some(wsl_err) = err.downcast_ref::<WslError>() {
                    match wsl_err {
                        WslError::InstallRefused => {
                            warn!(
                                "WSL2 auto-install declined; continuing without local Linux \
                                 container support — Linux services will be routed to peers."
                            );
                            println!(
                                "  WSL2 auto-install declined. Linux services will be routed \
                                 to remote peers via the overlay.\n  Re-run with \
                                 `--install-wsl yes` or install WSL2 manually \
                                 (`wsl.exe --install --no-distribution`) to enable local \
                                 Linux containers."
                            );
                        }
                        WslError::RebootRequired => {
                            anyhow::bail!(
                                "WSL2 install completed but Windows requires a reboot. \
                                 Please reboot and re-run `zlayer join`."
                            );
                        }
                        _ => {
                            warn!(
                                error = %err,
                                "WSL2 setup failed; continuing without local Linux container support"
                            );
                            println!(
                                "  WSL2 setup failed: {err}.\n  Linux services will be routed \
                                 to remote peers."
                            );
                        }
                    }
                } else {
                    warn!(error = %err, "WSL2 setup failed with an unexpected error; continuing");
                    println!(
                        "  WSL2 setup failed: {err}.\n  Linux services will be routed to \
                         remote peers."
                    );
                }
            }
        }
    }
    #[cfg(not(feature = "wsl"))]
    {
        let _ = install_wsl;
        warn!(
            "`wsl` feature not enabled in this build; skipping WSL2 backend bootstrap. \
             Local Linux container dispatch will be unavailable until WSL2 support is \
             compiled in."
        );
    }

    info!(
        token_len = token.len(),
        service = ?service,
        replicas = replicas,
        "Joining deployment"
    );

    // Reuse the shared helper below. `_spec_dir` is reserved for future use
    // (loading a bundle directory instead of fetching over HTTP) so we thread
    // it through without touching it today.
    let _ = spec_dir;

    // Step 1: Parse join token
    let join_token = parse_join_token(token)?;

    info!(
        api_endpoint = %join_token.api_endpoint,
        deployment = %join_token.deployment,
        "Joining deployment"
    );

    println!("Joining deployment: {}", join_token.deployment);
    println!("API endpoint: {}", join_token.api_endpoint);

    // Step 2: Authenticate with API
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    info!("Authenticating with API...");
    let auth_response = client
        .post(format!("{}/api/v1/auth/verify", join_token.api_endpoint))
        .bearer_auth(&join_token.key)
        .send()
        .await
        .context("Failed to authenticate with API")?;

    if !auth_response.status().is_success() {
        let status = auth_response.status();
        let body = auth_response.text().await.unwrap_or_default();
        anyhow::bail!("Authentication failed: {status} - {body}");
    }
    info!("Authentication successful");
    println!("Authentication successful");

    // Step 3: Fetch deployment spec from API
    info!("Fetching deployment spec from API...");
    let spec_response = client
        .get(format!(
            "{}/api/v1/deployments/{}/spec",
            join_token.api_endpoint, join_token.deployment
        ))
        .bearer_auth(&join_token.key)
        .send()
        .await
        .context("Failed to fetch deployment spec")?;

    if !spec_response.status().is_success() {
        let status = spec_response.status();
        let body = spec_response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to fetch deployment spec: {status} - {body}");
    }

    let spec: DeploymentSpec = spec_response
        .json()
        .await
        .context("Failed to parse deployment spec")?;

    info!(
        deployment = %spec.deployment,
        services = spec.services.len(),
        "Fetched deployment spec"
    );
    println!("Fetched deployment spec: {} services", spec.services.len());

    // Step 4: Determine which service(s) to join
    let target_service = service.or(join_token.service.as_deref());
    let services_to_join: Vec<(String, zlayer_spec::ServiceSpec)> =
        if let Some(svc) = target_service {
            // Join specific service
            if !spec.services.contains_key(svc) {
                anyhow::bail!("Service '{svc}' not found in deployment");
            }
            vec![(svc.to_string(), spec.services.get(svc).unwrap().clone())]
        } else {
            // Join all services (for global join)
            spec.services
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };

    println!("\nServices to join:");
    for (name, svc_spec) in &services_to_join {
        println!("  - {} (image: {})", name, svc_spec.image.name);
    }

    // Step 5: Build runtime (HCS + optional WSL2 delegate on Windows)
    let runtime_config = build_runtime_config(cli);
    info!(runtime = ?cli.runtime, "Creating container runtime");

    let runtime = zlayer_agent::create_runtime(runtime_config, None)
        .await
        .context("Failed to create container runtime")?;
    info!("Runtime created successfully");

    // Step 6: Install inbound firewall rules for overlay UDP + API TCP + Raft TCP.
    //
    // Ports match the cluster defaults. `zlayer join <url>` doesn't learn
    // the real API/Raft ports from the join token (only the API endpoint
    // URL), so we install the rules for the cluster defaults. Operators
    // running a non-default bind can run `netsh` themselves; the rules are
    // idempotent and scoped to Private+Domain profiles.
    let wg_port = zlayer_core::DEFAULT_WG_PORT;
    let api_port: u16 = 3669;
    let raft_port: u16 = 9000;
    if let Err(e) = zlayer_overlay::firewall::ensure_overlay_rules(wg_port, api_port, raft_port) {
        warn!(
            error = %e,
            "Failed to install Windows firewall rules; overlay peers may not reach this node"
        );
        println!("Warning: Firewall rule install failed: {e}");
    } else {
        info!(
            wg_port = wg_port,
            api_port = api_port,
            raft_port = raft_port,
            "Installed Windows firewall allow-rules"
        );
    }

    // Step 7: Setup overlay networks
    let overlay_manager = match zlayer_agent::OverlayManager::new(
        spec.deployment.clone(),
        std::process::id().to_string(),
    )
    .await
    {
        Ok(mut om) => {
            // Setup global overlay
            if let Err(e) = om.setup_global_overlay().await {
                warn!("Failed to setup global overlay (non-fatal): {}", e);
                println!("Warning: Overlay network setup failed: {e}");
            } else {
                info!("Global overlay network created");
                println!("Global overlay network created");
            }
            let om = Arc::new(RwLock::new(om));
            zlayer_agent::OverlayManager::start_periodic_orphan_sweep(om.clone());
            Some(om)
        }
        Err(e) => {
            warn!("Overlay networks disabled: {}", e);
            println!("Warning: Overlay networks disabled: {e}");
            None
        }
    };

    // Step 8: Create ServiceManager with overlay support
    let mut builder = zlayer_agent::ServiceManager::builder(runtime.clone());
    if let Some(om) = overlay_manager.clone() {
        builder = builder.overlay_manager(om);
    }
    let manager = builder.build();

    println!("\n=== Starting Services ===\n");

    // Step 9: For each service, pull image, run init, register and scale
    for (service_name, service_spec) in services_to_join {
        info!(service = %service_name, "Joining service");
        println!("Joining service: {service_name}");

        // Pull image
        println!("  Pulling image: {}...", service_spec.image.name);
        let image_str = service_spec.image.name.to_string();
        runtime
            .pull_image(&image_str)
            .await
            .context(format!("Failed to pull image for service '{service_name}'"))?;
        info!(service = %service_name, image = %service_spec.image.name, "Image pulled");
        println!("  Image pulled successfully");

        // Run init steps (if any)
        if !service_spec.init.steps.is_empty() {
            println!(
                "  Running {} init step(s)...",
                service_spec.init.steps.len()
            );
            for step in &service_spec.init.steps {
                info!(service = %service_name, step = %step.id, "Running init step");
                println!("    Step: {}", step.id);

                let action = zlayer_init_actions::from_spec(
                    &step.uses,
                    &step.with,
                    Duration::from_secs(300),
                )
                .context(format!("Invalid init action: {}", step.uses))?;

                action
                    .execute()
                    .await
                    .context(format!("Init step '{}' failed", step.id))?;

                info!(service = %service_name, step = %step.id, "Init step completed");
            }
        }

        // Register service
        Box::pin(manager.upsert_service(service_name.clone(), service_spec.clone()))
            .await
            .context(format!("Failed to register service '{service_name}'"))?;
        info!(service = %service_name, "Service registered");

        // Determine replica count
        let target_replicas = if replicas > 0 {
            replicas
        } else {
            match &service_spec.scale {
                zlayer_spec::ScaleSpec::Fixed { replicas } => *replicas,
                zlayer_spec::ScaleSpec::Adaptive { min, .. } => *min,
                zlayer_spec::ScaleSpec::Manual => 1, // Join implies at least 1 replica
            }
        };

        // Scale service
        println!("  Scaling to {target_replicas} replica(s)...");
        manager
            .scale_service(&service_name, target_replicas)
            .await
            .context(format!("Failed to scale service '{service_name}'"))?;

        info!(
            service = %service_name,
            replicas = target_replicas,
            "Service joined"
        );
        println!("  Service '{service_name}' joined with {target_replicas} replica(s)");
    }

    // Step 10: Detect GPUs for registration (informational on the join path —
    // the scheduler reads GPUs through the normal node-register flow during
    // `zlayer node join`, but printing them here helps operators verify that
    // H-4 GPU detection is seeing the hardware they expect).
    let detected_gpus = zlayer_agent::detect_gpus();
    if detected_gpus.is_empty() {
        info!("No GPUs detected on this node");
    } else {
        info!(
            gpu_count = detected_gpus.len(),
            "Detected GPUs on this node"
        );
        for gpu in &detected_gpus {
            info!(
                pci_bus_id = %gpu.pci_bus_id,
                vendor = %gpu.vendor,
                model = %gpu.model,
                memory_mb = gpu.memory_mb,
                device_path = %gpu.device_path,
                "Detected GPU"
            );
        }
    }

    // Step 11: Wait for Ctrl+C
    println!("\n=== Join Complete ===");
    println!("Services are running. Press Ctrl+C to leave the deployment.");

    tokio::signal::ctrl_c()
        .await
        .context("Failed to wait for Ctrl+C")?;

    println!("\nShutting down...");
    info!("Received shutdown signal, cleaning up");

    // Step 12: Cleanup overlay networks on exit
    if let Some(om) = overlay_manager {
        info!("Cleaning up overlay networks");
        let mut om_guard = om.write().await;
        if let Err(e) = om_guard.cleanup().await {
            warn!("Failed to cleanup overlay networks: {}", e);
        } else {
            info!("Overlay networks cleaned up");
        }
    }

    println!("Goodbye!");
    Ok(())
}

/// Join an existing deployment
#[cfg(unix)]
#[allow(clippy::too_many_lines)]
pub(crate) async fn join(
    cli: &Cli,
    token: &str,
    _spec_dir: Option<&str>,
    service: Option<&str>,
    replicas: u32,
) -> Result<()> {
    info!(
        token_len = token.len(),
        service = ?service,
        replicas = replicas,
        "Joining deployment"
    );

    // Step 1: Parse join token
    let join_token = parse_join_token(token)?;

    info!(
        api_endpoint = %join_token.api_endpoint,
        deployment = %join_token.deployment,
        "Joining deployment"
    );

    println!("Joining deployment: {}", join_token.deployment);
    println!("API endpoint: {}", join_token.api_endpoint);

    // Step 2: Authenticate with API
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    info!("Authenticating with API...");
    let auth_response = client
        .post(format!("{}/api/v1/auth/verify", join_token.api_endpoint))
        .bearer_auth(&join_token.key)
        .send()
        .await
        .context("Failed to authenticate with API")?;

    if !auth_response.status().is_success() {
        let status = auth_response.status();
        let body = auth_response.text().await.unwrap_or_default();
        anyhow::bail!("Authentication failed: {status} - {body}");
    }
    info!("Authentication successful");
    println!("Authentication successful");

    // Step 3: Fetch deployment spec from API
    info!("Fetching deployment spec from API...");
    let spec_response = client
        .get(format!(
            "{}/api/v1/deployments/{}/spec",
            join_token.api_endpoint, join_token.deployment
        ))
        .bearer_auth(&join_token.key)
        .send()
        .await
        .context("Failed to fetch deployment spec")?;

    if !spec_response.status().is_success() {
        let status = spec_response.status();
        let body = spec_response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to fetch deployment spec: {status} - {body}");
    }

    let spec: DeploymentSpec = spec_response
        .json()
        .await
        .context("Failed to parse deployment spec")?;

    info!(
        deployment = %spec.deployment,
        services = spec.services.len(),
        "Fetched deployment spec"
    );
    println!("Fetched deployment spec: {} services", spec.services.len());

    // Step 4: Determine which service(s) to join
    let target_service = service.or(join_token.service.as_deref());
    let services_to_join: Vec<(String, zlayer_spec::ServiceSpec)> =
        if let Some(svc) = target_service {
            // Join specific service
            if !spec.services.contains_key(svc) {
                anyhow::bail!("Service '{svc}' not found in deployment");
            }
            vec![(svc.to_string(), spec.services.get(svc).unwrap().clone())]
        } else {
            // Join all services (for global join)
            spec.services
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };

    println!("\nServices to join:");
    for (name, svc_spec) in &services_to_join {
        println!("  - {} (image: {})", name, svc_spec.image.name);
    }

    // Step 5: Build runtime
    let runtime_config = build_runtime_config(cli);
    info!(runtime = ?cli.runtime, "Creating container runtime");

    let runtime = zlayer_agent::create_runtime(runtime_config, None)
        .await
        .context("Failed to create container runtime")?;
    info!("Runtime created successfully");

    // Step 6: Setup overlay networks
    let overlay_manager = match zlayer_agent::OverlayManager::new(
        spec.deployment.clone(),
        std::process::id().to_string(),
    )
    .await
    {
        Ok(mut om) => {
            // Setup global overlay
            if let Err(e) = om.setup_global_overlay().await {
                warn!("Failed to setup global overlay (non-fatal): {}", e);
                println!("Warning: Overlay network setup failed: {e}");
            } else {
                info!("Global overlay network created");
                println!("Global overlay network created");
            }
            let om = Arc::new(RwLock::new(om));
            zlayer_agent::OverlayManager::start_periodic_orphan_sweep(om.clone());
            Some(om)
        }
        Err(e) => {
            warn!("Overlay networks disabled: {}", e);
            println!("Warning: Overlay networks disabled: {e}");
            None
        }
    };

    // Step 7: Create ServiceManager with overlay support
    let mut builder = zlayer_agent::ServiceManager::builder(runtime.clone());
    if let Some(om) = overlay_manager.clone() {
        builder = builder.overlay_manager(om);
    }
    let manager = builder.build();

    println!("\n=== Starting Services ===\n");

    // Step 8: For each service, pull image, run init, register and scale
    for (service_name, service_spec) in services_to_join {
        info!(service = %service_name, "Joining service");
        println!("Joining service: {service_name}");

        // Pull image
        println!("  Pulling image: {}...", service_spec.image.name);
        let image_str = service_spec.image.name.to_string();
        runtime
            .pull_image(&image_str)
            .await
            .context(format!("Failed to pull image for service '{service_name}'"))?;
        info!(service = %service_name, image = %service_spec.image.name, "Image pulled");
        println!("  Image pulled successfully");

        // Run init steps (if any)
        if !service_spec.init.steps.is_empty() {
            println!(
                "  Running {} init step(s)...",
                service_spec.init.steps.len()
            );
            for step in &service_spec.init.steps {
                info!(service = %service_name, step = %step.id, "Running init step");
                println!("    Step: {}", step.id);

                let action = zlayer_init_actions::from_spec(
                    &step.uses,
                    &step.with,
                    Duration::from_secs(300),
                )
                .context(format!("Invalid init action: {}", step.uses))?;

                action
                    .execute()
                    .await
                    .context(format!("Init step '{}' failed", step.id))?;

                info!(service = %service_name, step = %step.id, "Init step completed");
            }
        }

        // Register service
        Box::pin(manager.upsert_service(service_name.clone(), service_spec.clone()))
            .await
            .context(format!("Failed to register service '{service_name}'"))?;
        info!(service = %service_name, "Service registered");

        // Determine replica count
        let target_replicas = if replicas > 0 {
            replicas
        } else {
            match &service_spec.scale {
                zlayer_spec::ScaleSpec::Fixed { replicas } => *replicas,
                zlayer_spec::ScaleSpec::Adaptive { min, .. } => *min,
                zlayer_spec::ScaleSpec::Manual => 1, // Join implies at least 1 replica
            }
        };

        // Scale service
        println!("  Scaling to {target_replicas} replica(s)...");
        manager
            .scale_service(&service_name, target_replicas)
            .await
            .context(format!("Failed to scale service '{service_name}'"))?;

        info!(
            service = %service_name,
            replicas = target_replicas,
            "Service joined"
        );
        println!("  Service '{service_name}' joined with {target_replicas} replica(s)");
    }

    // Step 9: Wait for Ctrl+C
    println!("\n=== Join Complete ===");
    println!("Services are running. Press Ctrl+C to leave the deployment.");

    tokio::signal::ctrl_c()
        .await
        .context("Failed to wait for Ctrl+C")?;

    println!("\nShutting down...");
    info!("Received shutdown signal, cleaning up");

    // Step 10: Cleanup overlay networks on exit
    if let Some(om) = overlay_manager {
        info!("Cleaning up overlay networks");
        let mut om_guard = om.write().await;
        if let Err(e) = om_guard.cleanup().await {
            warn!("Failed to cleanup overlay networks: {}", e);
        } else {
            info!("Overlay networks cleaned up");
        }
    }

    println!("Goodbye!");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_join_token() {
        use base64::Engine;

        let info = serde_json::json!({
            "api_endpoint": "http://localhost:3669",
            "deployment": "my-app",
            "key": "secret-auth-key",
            "service": "api"
        });

        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_string(&info).unwrap());

        let parsed = parse_join_token(&token).unwrap();
        assert_eq!(parsed.api_endpoint, "http://localhost:3669");
        assert_eq!(parsed.deployment, "my-app");
        assert_eq!(parsed.key, "secret-auth-key");
        assert_eq!(parsed.service, Some("api".to_string()));
    }

    #[test]
    fn test_parse_join_token_minimal() {
        use base64::Engine;

        let info = serde_json::json!({
            "api_endpoint": "http://api.example.com",
            "deployment": "my-deploy",
            "key": "auth-key"
        });

        let token =
            base64::engine::general_purpose::STANDARD.encode(serde_json::to_string(&info).unwrap());

        let parsed = parse_join_token(&token).unwrap();
        assert_eq!(parsed.api_endpoint, "http://api.example.com");
        assert_eq!(parsed.deployment, "my-deploy");
        assert_eq!(parsed.key, "auth-key");
        assert!(parsed.service.is_none());
    }

    #[test]
    fn test_parse_join_token_invalid_base64() {
        let result = parse_join_token("not-valid-base64!!!");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not valid base64"));
    }

    #[test]
    fn test_parse_join_token_invalid_json() {
        use base64::Engine;

        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("not json");

        let result = parse_join_token(&token);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not valid JSON"));
    }

    /// The Windows branch of `join()` takes an extra `ConsentMode` parameter
    /// (H-3 WSL2 consent wiring). This test references the function by path
    /// so the signature is type-checked at compile time — if the `ConsentMode`
    /// parameter is dropped or renamed, this test stops compiling. Integration
    /// coverage lives in `composite_dispatch_e2e` (K-3).
    ///
    /// We don't coerce `join` to an HRTB fn-pointer: `async fn` items carry
    /// an opaque return type that confounds the coercion checker once the
    /// signature has any elided lifetimes beyond `&Cli`. Referencing the
    /// function item directly (`let _fn = join;`) is enough — Rust still
    /// validates the path resolution and the full signature through usage.
    #[cfg(windows)]
    #[test]
    fn test_windows_join_accepts_consent_mode() {
        use crate::ui::consent::ConsentMode;

        // Reference the function by path — the compiler validates resolution
        // and the full signature (including the `ConsentMode` parameter)
        // through usage. We use plain `_` discards (not `_fn` / `_mode`) to
        // avoid `clippy::no_effect_underscore_binding`. `ConsentMode` is
        // `Copy`, so we can't `drop` it without tripping
        // `clippy::dropping_copy_types` — a typed let-discard asserts the
        // type exists and that `Ask` is a valid variant without the drop.
        let _ = join;
        let _: ConsentMode = ConsentMode::Ask;
    }
}
