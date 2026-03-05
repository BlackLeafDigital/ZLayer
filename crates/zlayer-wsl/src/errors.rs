use thiserror::Error;

#[derive(Debug, Error)]
pub enum WslError {
    #[error("WSL2 is not installed. Install it with: wsl.exe --install")]
    WslNotInstalled,

    #[error("WSL2 is installed but only WSL1 is available. Enable WSL2: wsl.exe --set-default-version 2")]
    Wsl1Only,

    #[error("Failed to create ZLayer WSL2 distro: {0}")]
    DistroCreationFailed(String),

    #[error("ZLayer daemon failed to start inside WSL2: {0}")]
    DaemonStartFailed(String),

    #[error("ZLayer daemon health check timed out after {0:?}")]
    DaemonTimeout(std::time::Duration),

    #[error("WSL command failed: {0}")]
    CommandFailed(String),

    #[error("Path translation failed: {0}")]
    PathTranslation(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}
