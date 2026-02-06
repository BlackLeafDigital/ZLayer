//! Shared utility functions for the ZLayer runtime CLI.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tracing::info;
use zlayer_spec::DeploymentSpec;

/// Discover spec path from explicit argument or auto-discovery
pub(crate) fn discover_spec_path(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    let candidates = [".zlayer.yml", ".zlayer.yaml", "zlayer.yml", "zlayer.yaml"];
    for candidate in &candidates {
        let path = PathBuf::from(candidate);
        if path.exists() {
            return Ok(path);
        }
    }
    anyhow::bail!("No deployment spec found. Create .zlayer.yml or specify a path.");
}

/// Parse and validate a deployment spec file
pub(crate) fn parse_spec(spec_path: &Path) -> Result<DeploymentSpec> {
    info!(path = %spec_path.display(), "Parsing deployment spec");

    let spec = zlayer_spec::from_yaml_file(spec_path)
        .with_context(|| format!("Failed to parse spec file: {}", spec_path.display()))?;

    info!(
        deployment = %spec.deployment,
        version = %spec.version,
        services = spec.services.len(),
        "Spec parsed successfully"
    );

    Ok(spec)
}

/// Parse an image reference into name and tag components
pub(crate) fn parse_image_reference(image: &str) -> (String, String) {
    // Handle digest references (image@sha256:...)
    if let Some(at_pos) = image.rfind('@') {
        let name = &image[..at_pos];
        let digest = &image[at_pos + 1..];
        return (name.to_string(), digest.to_string());
    }

    // Handle tag references (image:tag)
    if let Some(colon_pos) = image.rfind(':') {
        // Make sure we're not splitting on a port number
        let after_colon = &image[colon_pos + 1..];
        if !after_colon.contains('/') {
            let name = &image[..colon_pos];
            let tag = after_colon;
            return (name.to_string(), tag.to_string());
        }
    }

    // No tag specified, use "latest"
    (image.to_string(), "latest".to_string())
}

/// Parse a duration string like "1h", "30m", "3600s"
pub(crate) fn parse_duration(s: &str) -> Result<u64> {
    let s = s.trim();

    // Try to parse as plain seconds
    if let Ok(secs) = s.parse::<u64>() {
        return Ok(secs);
    }

    // Parse with suffix
    if let Some(num_str) = s.strip_suffix('s') {
        let num: u64 = num_str
            .trim()
            .parse()
            .with_context(|| format!("Invalid duration number: {}", num_str))?;
        return Ok(num);
    }

    if let Some(num_str) = s.strip_suffix('m') {
        let num: u64 = num_str
            .trim()
            .parse()
            .with_context(|| format!("Invalid duration number: {}", num_str))?;
        return Ok(num * 60);
    }

    if let Some(num_str) = s.strip_suffix('h') {
        let num: u64 = num_str
            .trim()
            .parse()
            .with_context(|| format!("Invalid duration number: {}", num_str))?;
        return Ok(num * 3600);
    }

    anyhow::bail!(
        "Invalid duration format: {}. Use formats like: 1h, 30m, 3600s",
        s
    )
}

/// Decode a base64url-encoded JSON string
pub(crate) fn decode_base64_json(input: &str) -> Result<serde_json::Value> {
    use base64::Engine;

    // JWT uses base64url encoding without padding
    // Try URL-safe first, then standard base64
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(input)
        .or_else(|_| {
            // Add padding if needed and try again
            let padded = match input.len() % 4 {
                2 => format!("{}==", input),
                3 => format!("{}=", input),
                _ => input.to_string(),
            };
            base64::engine::general_purpose::URL_SAFE.decode(&padded)
        })
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(input))
        .context("Failed to decode base64")?;

    serde_json::from_slice(&decoded).context("Failed to parse JSON")
}
