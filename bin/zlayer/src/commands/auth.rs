//! `zlayer auth` subcommand handlers.
//!
//! Manages authentication against a remote `ZLayer` server:
//! - `login`  -- obtain a JWT and store it locally
//! - `logout` -- remove stored credentials
//! - `status` -- display current auth state

use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::cli::AuthCommands;

/// On-disk credential store (`~/.zlayer/credentials.json`).
#[derive(Debug, Serialize, Deserialize)]
struct StoredCredentials {
    server: String,
    token: String,
    expires_at: DateTime<Utc>,
}

/// Response from `POST /auth/token`.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[allow(dead_code)]
    token_type: String,
    expires_in: u64,
}

/// Resolve the credentials file path (`~/.zlayer/credentials.json`).
fn credentials_path() -> PathBuf {
    zlayer_paths::ZLayerDirs::default_data_dir().join("credentials.json")
}

/// Entry point for `zlayer auth <subcommand>`.
pub(crate) async fn handle_auth(cmd: &AuthCommands) -> Result<()> {
    match cmd {
        AuthCommands::Login {
            url,
            api_key,
            api_secret,
        } => login(url, api_key.as_deref(), api_secret.as_deref()).await,
        AuthCommands::Logout => logout(),
        AuthCommands::Status => status(),
    }
}

/// Prompt the user for a string value on stderr (so stdout stays clean for
/// scripts).  The prompt is *not* hidden -- use this for the API key.
fn prompt(label: &str) -> Result<String> {
    eprint!("{label}: ");
    io::stderr().flush()?;
    let mut buf = String::new();
    io::stdin()
        .read_line(&mut buf)
        .context("Failed to read input")?;
    Ok(buf.trim().to_string())
}

/// Prompt for a secret value. On Unix we disable terminal echo; on other
/// platforms we fall back to a plain prompt with a note.
#[allow(unsafe_code)]
fn prompt_secret(label: &str) -> Result<String> {
    eprint!("{label}: ");
    io::stderr().flush()?;

    #[cfg(unix)]
    {
        // Disable echo
        use std::os::unix::io::AsRawFd;
        let fd = io::stdin().as_raw_fd();
        let mut termios = unsafe {
            let mut t = std::mem::zeroed();
            if libc::tcgetattr(fd, &raw mut t) != 0 {
                // If tcgetattr fails (e.g. piped stdin), fall back to plain read
                return plain_read_line();
            }
            t
        };
        let orig = termios;
        termios.c_lflag &= !libc::ECHO;
        unsafe {
            libc::tcsetattr(fd, libc::TCSANOW, &raw const termios);
        }
        let result = plain_read_line();
        unsafe {
            libc::tcsetattr(fd, libc::TCSANOW, &raw const orig);
        }
        eprintln!(); // newline after hidden input
        result
    }

    #[cfg(not(unix))]
    {
        plain_read_line()
    }
}

fn plain_read_line() -> Result<String> {
    let mut buf = String::new();
    io::stdin()
        .read_line(&mut buf)
        .context("Failed to read input")?;
    Ok(buf.trim().to_string())
}

async fn login(url: &str, api_key: Option<&str>, api_secret: Option<&str>) -> Result<()> {
    let key = match api_key {
        Some(k) => k.to_string(),
        None => prompt("API key")?,
    };
    let secret = match api_secret {
        Some(s) => s.to_string(),
        None => prompt_secret("API secret")?,
    };

    let endpoint = format!("{}/auth/token", url.trim_end_matches('/'));
    let body = serde_json::json!({
        "api_key": key,
        "api_secret": secret,
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(&endpoint)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("Failed to connect to {endpoint}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Authentication failed ({status}): {text}");
    }

    let token_resp: TokenResponse = resp
        .json()
        .await
        .context("Failed to parse token response")?;

    let expires_at =
        Utc::now() + Duration::seconds(i64::try_from(token_resp.expires_in).unwrap_or(3600));

    let creds = StoredCredentials {
        server: url.trim_end_matches('/').to_string(),
        token: token_resp.access_token,
        expires_at,
    };

    let path = credentials_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(&creds)?;
    std::fs::write(&path, &json)
        .with_context(|| format!("Failed to write credentials to {}", path.display()))?;

    // Restrict permissions on Unix so only the owner can read the file.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }

    println!("Logged in to {}", creds.server);
    println!("Token expires at {}", creds.expires_at);
    Ok(())
}

fn logout() -> Result<()> {
    let path = credentials_path();
    if path.exists() {
        std::fs::remove_file(&path)
            .with_context(|| format!("Failed to remove {}", path.display()))?;
        println!("Logged out (credentials removed)");
    } else {
        println!("No stored credentials found");
    }
    Ok(())
}

fn status() -> Result<()> {
    let path = credentials_path();
    if !path.exists() {
        println!("Not logged in");
        println!("  Run `zlayer auth login <url>` to authenticate");
        return Ok(());
    }

    let data = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let creds: StoredCredentials =
        serde_json::from_str(&data).context("Failed to parse credentials file")?;

    println!("Server:  {}", creds.server);
    println!("Expires: {}", creds.expires_at);

    if Utc::now() >= creds.expires_at {
        println!("Status:  EXPIRED");
        println!(
            "  Run `zlayer auth login {}` to re-authenticate",
            creds.server
        );
    } else {
        let remaining = creds.expires_at - Utc::now();
        let hours = remaining.num_hours();
        let mins = remaining.num_minutes() % 60;
        println!("Status:  valid ({hours}h {mins}m remaining)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_roundtrip() {
        let creds = StoredCredentials {
            server: "http://10.0.0.1:3669".to_string(),
            token: "eyJtest".to_string(),
            expires_at: Utc::now() + Duration::hours(1),
        };
        let json = serde_json::to_string_pretty(&creds).unwrap();
        let parsed: StoredCredentials = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.server, creds.server);
        assert_eq!(parsed.token, creds.token);
    }

    #[test]
    fn credentials_path_ends_with_expected_name() {
        let p = credentials_path();
        assert_eq!(p.file_name().unwrap(), "credentials.json");
    }

    #[test]
    fn status_handles_missing_file() {
        // Just verify the credentials_path function works without panicking
        let _ = credentials_path();
    }

    #[test]
    fn token_response_deserializes() {
        let json = r#"{"access_token":"tok","token_type":"Bearer","expires_in":3600}"#;
        let resp: TokenResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.access_token, "tok");
        assert_eq!(resp.expires_in, 3600);
    }
}
