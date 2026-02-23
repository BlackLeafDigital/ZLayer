//! CLI argument parsing for zlayer-manager

#![allow(clippy::doc_markdown)]

use std::path::PathBuf;

use clap::Parser;

/// Connection mode for `ZLayer` manager
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum ConnectionMode {
    /// Connect to a remote ZLayer API
    Remote { url: String, token: Option<String> },
    /// Start an embedded ZLayer instance
    Embedded { config: Option<PathBuf> },
}

/// ZLayer Manager - Web UI for managing ZLayer deployments
#[derive(Parser, Debug)]
#[command(name = "zlayer-manager")]
#[command(version, about, long_about = None)]
pub struct Args {
    /// Connect to an existing ZLayer API at the specified URL
    #[arg(long = "connect", value_name = "URL", conflicts_with = "embedded")]
    pub connect_url: Option<String>,

    /// Start an embedded ZLayer instance
    #[arg(long, conflicts_with = "connect_url")]
    pub embedded: bool,

    /// Port to bind the web server
    #[arg(long, short, default_value = "9120", env = "PORT")]
    pub port: u16,

    /// Path to configuration file
    #[arg(long, short, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// API token for authentication (used with --connect)
    #[arg(long, env = "ZLAYER_API_TOKEN")]
    pub token: Option<String>,
}

impl Args {
    /// Determine the connection mode based on CLI arguments
    #[allow(dead_code)]
    pub fn connection_mode(&self) -> ConnectionMode {
        if let Some(url) = &self.connect_url {
            ConnectionMode::Remote {
                url: url.clone(),
                token: self.token.clone(),
            }
        } else {
            ConnectionMode::Embedded {
                config: self.config.clone(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_port() {
        let args = Args::parse_from(["zlayer-manager"]);
        assert_eq!(args.port, 9120);
    }

    #[test]
    fn test_embedded_mode() {
        let args = Args::parse_from(["zlayer-manager", "--embedded"]);
        assert!(args.embedded);
        assert!(matches!(
            args.connection_mode(),
            ConnectionMode::Embedded { .. }
        ));
    }

    #[test]
    fn test_connect_mode() {
        let args = Args::parse_from(["zlayer-manager", "--connect", "http://localhost:3669"]);
        assert_eq!(args.connect_url.as_deref(), Some("http://localhost:3669"));
        match args.connection_mode() {
            ConnectionMode::Remote { url, token } => {
                assert_eq!(url, "http://localhost:3669");
                assert!(token.is_none());
            }
            _ => panic!("Expected Remote mode"),
        }
    }

    #[test]
    fn test_connect_with_token() {
        let args = Args::parse_from([
            "zlayer-manager",
            "--connect",
            "http://localhost:3669",
            "--token",
            "secret123",
        ]);
        match args.connection_mode() {
            ConnectionMode::Remote { url, token } => {
                assert_eq!(url, "http://localhost:3669");
                assert_eq!(token.as_deref(), Some("secret123"));
            }
            _ => panic!("Expected Remote mode"),
        }
    }

    #[test]
    fn test_custom_port() {
        let args = Args::parse_from(["zlayer-manager", "--port", "3000"]);
        assert_eq!(args.port, 3000);
    }

    #[test]
    fn test_config_path() {
        let args = Args::parse_from(["zlayer-manager", "--config", "/etc/zlayer/config.yaml"]);
        assert_eq!(
            args.config.as_deref(),
            Some(PathBuf::from("/etc/zlayer/config.yaml").as_path())
        );
    }
}
