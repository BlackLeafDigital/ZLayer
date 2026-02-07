use anyhow::{Context, Result};
use std::time::Duration;
use tracing::warn;
use zlayer_api::create_token;

use crate::cli::TokenCommands;
use crate::util::decode_base64_json;

pub(crate) fn handle_token(action: &TokenCommands) -> Result<()> {
    match action {
        TokenCommands::Create {
            subject,
            secret,
            hours,
            roles,
            quiet,
        } => {
            let subject = subject.clone();
            let secret = secret.clone();
            let hours = *hours;
            let roles = roles.clone();
            let quiet = *quiet;
            let secret = secret
                .or_else(|| std::env::var("ZLAYER_JWT_SECRET").ok())
                .unwrap_or_else(|| {
                    if !quiet {
                        warn!("Using default secret - tokens will only work with default server config");
                    }
                    "CHANGE_ME_IN_PRODUCTION".to_string()
                });

            let roles: Vec<String> = roles
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            let expiry = Duration::from_secs(hours * 3600);

            let token = create_token(&secret, &subject, expiry, roles.clone())
                .context("Failed to create token")?;

            if quiet {
                println!("{}", token);
            } else {
                println!("Token created successfully!\n");
                println!("Subject: {}", subject);
                println!("Roles: {}", roles.join(", "));
                println!("Expires in: {} hours", hours);
                println!("\nToken:");
                println!("{}", token);
                println!("\nUsage:");
                println!(
                    "  curl -H 'Authorization: Bearer {}' http://localhost:8080/api/v1/deployments",
                    token
                );
            }
            Ok(())
        }

        TokenCommands::Decode { token, json } => {
            let token = token.clone();
            let json = *json;
            let parts: Vec<&str> = token.split('.').collect();
            if parts.len() != 3 {
                anyhow::bail!("Invalid JWT format: expected 3 parts separated by dots");
            }

            let header = decode_base64_json(parts[0]).context("Failed to decode token header")?;
            let claims = decode_base64_json(parts[1]).context("Failed to decode token payload")?;

            if json {
                let output = serde_json::json!({
                    "header": header,
                    "claims": claims,
                    "signature": parts[2]
                });
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                println!("Token Header:");
                println!("{}", serde_json::to_string_pretty(&header)?);
                println!("\nToken Claims:");
                println!("{}", serde_json::to_string_pretty(&claims)?);

                if let Some(exp) = claims.get("exp").and_then(|v| v.as_u64()) {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs();
                    if exp < now {
                        println!("\n[!] Token is EXPIRED");
                    } else {
                        let remaining = exp - now;
                        let hours = remaining / 3600;
                        let mins = (remaining % 3600) / 60;
                        println!("\n[OK] Token expires in {}h {}m", hours, mins);
                    }
                }
            }
            Ok(())
        }

        TokenCommands::Info => {
            println!("ZLayer Token System");
            println!("===================\n");
            println!("Available Roles:");
            println!("  admin    - Full access to all operations");
            println!("  operator - Can scale services, view logs, manage deployments");
            println!("  deployer - Can create and update deployments");
            println!("  reader   - Read-only access to deployments and services");
            println!();
            println!("Token Format: JWT (HS256)");
            println!("Default Expiry: 24 hours");
            println!();
            println!("Environment Variables:");
            println!("  ZLAYER_JWT_SECRET - JWT signing secret (required for production)");
            println!("  ZLAYER_TOKEN      - Bearer token for API requests");
            Ok(())
        }
    }
}
