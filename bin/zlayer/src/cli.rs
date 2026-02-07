//! CLI argument definitions for the `zlayer` entry point.
//!
//! When invoked without a subcommand (or with `tui`), the interactive TUI is
//! launched.  The `build` subcommand is handled in-process via `zlayer-builder`.
//! All other subcommands are delegated to the `zlayer-runtime` binary which must
//! be available on `$PATH` or in a well-known install location.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// ZLayer container orchestration platform
#[derive(Parser)]
#[command(
    name = "zlayer",
    version,
    about = "ZLayer container orchestration platform"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Build context directory (for TUI mode)
    #[arg(short, long)]
    pub context: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Launch interactive TUI
    Tui {
        #[arg(short, long)]
        context: Option<PathBuf>,
    },

    /// Build container images
    Build {
        /// Remaining args passed to build
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    // -----------------------------------------------------------------
    // Runtime passthrough commands
    // -----------------------------------------------------------------
    /// Deploy services from a spec file
    Deploy {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Join an existing deployment
    Join {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Deploy and start services (like docker compose up)
    Up {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Stop all services in a deployment
    Down {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// View service logs
    Logs {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Show runtime status
    Status {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Stop services or deployments
    Stop {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Validate a spec file
    Validate {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Start API server
    Serve {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Pull an image from a registry
    Pull {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Export images
    Export {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Import images
    Import {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// JWT token management
    Token {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Spec inspection
    Spec {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// WASM management
    Wasm {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Tunnel management
    Tunnel {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// ZLayer Manager commands
    Manager {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Cluster node management
    Node {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// List available runtime templates
    Runtimes {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

// ---------------------------------------------------------------------------
// Runtime passthrough helpers
// ---------------------------------------------------------------------------

/// Locate the `zlayer-runtime` binary.
///
/// Checks well-known install paths first, then falls back to `$PATH`.
pub fn which_runtime() -> anyhow::Result<PathBuf> {
    // Check common install locations
    for path in &["/usr/local/bin/zlayer-runtime", "/usr/bin/zlayer-runtime"] {
        let p = PathBuf::from(path);
        if p.exists() {
            return Ok(p);
        }
    }

    // Check next to the current executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("zlayer-runtime");
            if sibling.exists() {
                return Ok(sibling);
            }
        }
    }

    // Fall back to PATH via `which`
    if let Ok(output) = std::process::Command::new("which")
        .arg("zlayer-runtime")
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Ok(PathBuf::from(path));
            }
        }
    }

    anyhow::bail!(
        "zlayer-runtime not found. Install it or add it to your PATH.\n\
         The runtime binary handles deployment, orchestration, and cluster management."
    )
}

/// Execute `zlayer-runtime <subcommand> [args...]`, replacing the current
/// process on Unix or waiting for exit on Windows.
pub fn exec_runtime(subcommand: &str, args: &[String]) -> ! {
    let runtime_bin = match which_runtime() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {e:#}");
            std::process::exit(127);
        }
    };

    // On Unix, use exec to replace the process entirely
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&runtime_bin)
            .arg(subcommand)
            .args(args)
            .exec();
        // exec() only returns on error
        eprintln!("Error: failed to execute {}: {err}", runtime_bin.display());
        std::process::exit(127);
    }

    // On non-Unix, spawn and wait
    #[cfg(not(unix))]
    {
        let status = std::process::Command::new(&runtime_bin)
            .arg(subcommand)
            .args(args)
            .status();
        match status {
            Ok(s) => std::process::exit(s.code().unwrap_or(1)),
            Err(e) => {
                eprintln!("Error: failed to execute {}: {e}", runtime_bin.display());
                std::process::exit(127);
            }
        }
    }
}
