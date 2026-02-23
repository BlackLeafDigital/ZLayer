//! Join command handler.
//!
//! Parses a join token and joins an existing deployment by authenticating
//! with the API, fetching the deployment spec, and starting local replicas.

use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{info, warn};

use zlayer_spec::DeploymentSpec;

use crate::cli::Cli;
use crate::config::build_runtime_config;

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

/// Join an existing deployment
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
        anyhow::bail!("Authentication failed: {} - {}", status, body);
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
        anyhow::bail!("Failed to fetch deployment spec: {} - {}", status, body);
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
                anyhow::bail!("Service '{}' not found in deployment", svc);
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

    let runtime = zlayer_agent::create_runtime(runtime_config)
        .await
        .context("Failed to create container runtime")?;
    info!("Runtime created successfully");

    // Step 6: Setup overlay networks
    let overlay_manager = match zlayer_agent::OverlayManager::new(spec.deployment.clone()).await {
        Ok(mut om) => {
            // Setup global overlay
            if let Err(e) = om.setup_global_overlay().await {
                warn!("Failed to setup global overlay (non-fatal): {}", e);
                println!("Warning: Overlay network setup failed: {}", e);
            } else {
                info!("Global overlay network created");
                println!("Global overlay network created");
            }
            Some(Arc::new(RwLock::new(om)))
        }
        Err(e) => {
            warn!("Overlay networks disabled: {}", e);
            println!("Warning: Overlay networks disabled: {}", e);
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
        println!("Joining service: {}", service_name);

        // Pull image
        println!("  Pulling image: {}...", service_spec.image.name);
        runtime
            .pull_image(&service_spec.image.name)
            .await
            .context(format!(
                "Failed to pull image for service '{}'",
                service_name
            ))?;
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
        manager
            .upsert_service(service_name.clone(), service_spec.clone())
            .await
            .context(format!("Failed to register service '{}'", service_name))?;
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
        println!("  Scaling to {} replica(s)...", target_replicas);
        manager
            .scale_service(&service_name, target_replicas)
            .await
            .context(format!("Failed to scale service '{}'", service_name))?;

        info!(
            service = %service_name,
            replicas = target_replicas,
            "Service joined"
        );
        println!(
            "  Service '{}' joined with {} replica(s)",
            service_name, target_replicas
        );
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
}
