//! Daemon lifecycle management inside WSL2.
//!
//! Starts, stops, and health-checks the `ZLayer` daemon running
//! inside the dedicated `zlayer` WSL2 distribution.

use std::net::SocketAddr;
use std::time::Duration;

/// Configuration for the WSL2 backend connection.
#[derive(Debug, Clone)]
pub struct WslBackendConfig {
    /// TCP address the daemon listens on (reachable from Windows via localhost).
    pub api_addr: SocketAddr,
    /// Data directory inside WSL2.
    pub data_dir: String,
}

impl Default for WslBackendConfig {
    fn default() -> Self {
        Self {
            api_addr: "127.0.0.1:3669".parse().expect("valid socket addr"),
            data_dir: "/var/lib/zlayer".to_string(),
        }
    }
}

/// Start the ZLayer daemon inside WSL2.
///
/// # Errors
///
/// Returns an error if the daemon fails to start or the health check times out.
#[cfg(target_os = "windows")]
pub async fn start_daemon(config: &WslBackendConfig) -> anyhow::Result<()> {
    use tokio::process::Command;

    // Check if already running
    if check_daemon_health(config).await.unwrap_or(false) {
        tracing::debug!("WSL2 daemon already running");
        return Ok(());
    }

    let bind_addr = config.api_addr.to_string();
    tracing::info!("Starting ZLayer daemon inside WSL2 on {bind_addr}");

    // Start daemon in background inside WSL2
    // Using nohup + & to detach from the wsl.exe process
    let cmd = format!(
        "nohup /usr/local/bin/zlayer serve --bind {bind_addr} \
         --data-dir {} > /var/log/zlayer-daemon.log 2>&1 &",
        config.data_dir
    );

    let output = Command::new("wsl.exe")
        .args(["-d", super::distro::DISTRO_NAME, "--", "sh", "-c", &cmd])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to start daemon in WSL2: {stderr}");
    }

    // Wait for daemon to become healthy
    wait_for_daemon(config, Duration::from_secs(30)).await?;

    tracing::info!("ZLayer daemon is running inside WSL2");
    Ok(())
}

/// Start the `ZLayer` daemon inside WSL2 (non-Windows stub).
///
/// # Errors
///
/// Always returns an error on non-Windows platforms.
#[cfg(not(target_os = "windows"))]
#[allow(clippy::unused_async)]
pub async fn start_daemon(_config: &WslBackendConfig) -> anyhow::Result<()> {
    anyhow::bail!("WSL2 daemon management is only available on Windows")
}

/// Stop the ZLayer daemon inside WSL2.
///
/// # Errors
///
/// Returns an error if the stop command fails.
#[cfg(target_os = "windows")]
pub async fn stop_daemon() -> anyhow::Result<()> {
    let output = super::distro::wsl_exec("pkill", &["-TERM", "zlayer"]).await?;
    if !output.status.success() {
        tracing::warn!("pkill returned non-zero (daemon may not have been running)");
    }
    tracing::info!("Sent SIGTERM to ZLayer daemon in WSL2");
    Ok(())
}

/// Stop the `ZLayer` daemon inside WSL2 (non-Windows stub).
///
/// # Errors
///
/// Always returns an error on non-Windows platforms.
#[cfg(not(target_os = "windows"))]
#[allow(clippy::unused_async)]
pub async fn stop_daemon() -> anyhow::Result<()> {
    anyhow::bail!("WSL2 daemon management is only available on Windows")
}

/// Check if the daemon is running and healthy.
///
/// # Errors
///
/// Returns an error if the health check encounters an unexpected failure.
pub async fn check_daemon_health(config: &WslBackendConfig) -> anyhow::Result<bool> {
    match tokio::net::TcpStream::connect(config.api_addr).await {
        Ok(_) => {
            // TCP connection succeeded -- daemon is listening.
            // For a more thorough check, we could HTTP GET the /health/live
            // endpoint, but TCP connect is sufficient for startup detection.
            Ok(true)
        }
        Err(_) => Ok(false),
    }
}

/// Poll daemon with exponential backoff until healthy.
///
/// # Errors
///
/// Returns an error if the daemon doesn't become healthy within `timeout`.
pub async fn wait_for_daemon(config: &WslBackendConfig, timeout: Duration) -> anyhow::Result<()> {
    let start = std::time::Instant::now();
    let mut interval = Duration::from_millis(100);

    while start.elapsed() < timeout {
        if check_daemon_health(config).await.unwrap_or(false) {
            return Ok(());
        }
        tokio::time::sleep(interval).await;
        interval = (interval * 2).min(Duration::from_secs(2));
    }

    anyhow::bail!(
        "ZLayer daemon did not become healthy within {timeout:?}. \
         Check WSL2 logs: wsl.exe -d zlayer -- cat /var/log/zlayer-daemon.log"
    )
}
