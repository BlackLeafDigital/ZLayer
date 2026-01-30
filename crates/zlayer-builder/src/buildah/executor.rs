//! Buildah command execution
//!
//! This module provides functionality to execute buildah commands,
//! with support for both synchronous and streaming output.

use std::path::PathBuf;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{debug, error, instrument, trace};

use crate::error::{BuildError, Result};

use super::BuildahCommand;

/// Output from a buildah command execution
#[derive(Debug, Clone)]
pub struct CommandOutput {
    /// Standard output from the command
    pub stdout: String,

    /// Standard error from the command
    pub stderr: String,

    /// Exit code (0 = success)
    pub exit_code: i32,
}

impl CommandOutput {
    /// Returns true if the command succeeded (exit code 0)
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }

    /// Returns the combined stdout and stderr
    pub fn combined_output(&self) -> String {
        if self.stderr.is_empty() {
            self.stdout.clone()
        } else if self.stdout.is_empty() {
            self.stderr.clone()
        } else {
            format!("{}\n{}", self.stdout, self.stderr)
        }
    }
}

/// Executor for buildah commands
#[derive(Debug, Clone)]
pub struct BuildahExecutor {
    /// Path to the buildah binary
    buildah_path: PathBuf,

    /// Default storage driver (if set)
    storage_driver: Option<String>,

    /// Root directory for buildah storage
    root: Option<PathBuf>,

    /// Run directory for buildah state
    runroot: Option<PathBuf>,
}

impl Default for BuildahExecutor {
    fn default() -> Self {
        Self {
            buildah_path: PathBuf::from("buildah"),
            storage_driver: None,
            root: None,
            runroot: None,
        }
    }
}

impl BuildahExecutor {
    /// Create a new BuildahExecutor, locating the buildah binary (sync version)
    ///
    /// This will search for buildah in common system locations and PATH.
    /// For more comprehensive discovery with version checking, use [`new_async`].
    pub fn new() -> Result<Self> {
        let buildah_path = which_buildah()?;
        Ok(Self {
            buildah_path,
            storage_driver: None,
            root: None,
            runroot: None,
        })
    }

    /// Create a new BuildahExecutor using the BuildahInstaller
    ///
    /// This async version uses [`BuildahInstaller`] to find buildah and verify
    /// it meets minimum version requirements. If buildah is not found, it returns
    /// a helpful error with installation instructions.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use zlayer_builder::BuildahExecutor;
    ///
    /// # async fn example() -> Result<(), zlayer_builder::BuildError> {
    /// let executor = BuildahExecutor::new_async().await?;
    /// let version = executor.version().await?;
    /// println!("Using buildah version: {}", version);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn new_async() -> Result<Self> {
        use super::install::BuildahInstaller;

        let installer = BuildahInstaller::new();
        let installation = installer
            .ensure()
            .await
            .map_err(|e| BuildError::BuildahNotFound {
                message: e.to_string(),
            })?;

        Ok(Self {
            buildah_path: installation.path,
            storage_driver: None,
            root: None,
            runroot: None,
        })
    }

    /// Create a BuildahExecutor with a specific path to the buildah binary
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            buildah_path: path.into(),
            storage_driver: None,
            root: None,
            runroot: None,
        }
    }

    /// Set the storage driver
    pub fn storage_driver(mut self, driver: impl Into<String>) -> Self {
        self.storage_driver = Some(driver.into());
        self
    }

    /// Set the root directory for buildah storage
    pub fn root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = Some(root.into());
        self
    }

    /// Set the runroot directory for buildah state
    pub fn runroot(mut self, runroot: impl Into<PathBuf>) -> Self {
        self.runroot = Some(runroot.into());
        self
    }

    /// Get the path to the buildah binary
    pub fn buildah_path(&self) -> &PathBuf {
        &self.buildah_path
    }

    /// Build the base tokio Command with global options
    fn build_command(&self, cmd: &BuildahCommand) -> Command {
        let mut command = Command::new(&self.buildah_path);

        // Add global options before subcommand
        if let Some(ref driver) = self.storage_driver {
            command.arg("--storage-driver").arg(driver);
        }

        if let Some(ref root) = self.root {
            command.arg("--root").arg(root);
        }

        if let Some(ref runroot) = self.runroot {
            command.arg("--runroot").arg(runroot);
        }

        // Add command arguments
        command.args(&cmd.args);

        // Add environment variables
        for (key, value) in &cmd.env {
            command.env(key, value);
        }

        command
    }

    /// Execute a buildah command and wait for completion
    #[instrument(skip(self), fields(command = %cmd.to_command_string()))]
    pub async fn execute(&self, cmd: &BuildahCommand) -> Result<CommandOutput> {
        debug!("Executing buildah command");
        trace!("Full command: {:?}", cmd);

        let mut command = self.build_command(cmd);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());

        let output = command.output().await.map_err(|e| {
            error!("Failed to spawn buildah process: {}", e);
            BuildError::IoError(e)
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        if !output.status.success() {
            debug!(
                "Buildah command failed with exit code {}: {}",
                exit_code,
                stderr.trim()
            );
        }

        Ok(CommandOutput {
            stdout,
            stderr,
            exit_code,
        })
    }

    /// Execute a buildah command and return an error if it fails
    pub async fn execute_checked(&self, cmd: &BuildahCommand) -> Result<CommandOutput> {
        let output = self.execute(cmd).await?;

        if !output.success() {
            return Err(BuildError::BuildahExecution {
                command: cmd.to_command_string(),
                exit_code: output.exit_code,
                stderr: output.stderr,
            });
        }

        Ok(output)
    }

    /// Execute a buildah command with streaming output
    ///
    /// The callback is called for each line of output (both stdout and stderr).
    /// The first parameter indicates whether it's stdout (true) or stderr (false).
    #[instrument(skip(self, on_output), fields(command = %cmd.to_command_string()))]
    pub async fn execute_streaming<F>(
        &self,
        cmd: &BuildahCommand,
        mut on_output: F,
    ) -> Result<CommandOutput>
    where
        F: FnMut(bool, &str),
    {
        debug!("Executing buildah command with streaming output");

        let mut command = self.build_command(cmd);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = command.spawn().map_err(|e| {
            error!("Failed to spawn buildah process: {}", e);
            BuildError::IoError(e)
        })?;

        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");

        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stderr_reader = BufReader::new(stderr).lines();

        let mut stdout_output = String::new();
        let mut stderr_output = String::new();

        // Read stdout and stderr concurrently
        loop {
            tokio::select! {
                line = stdout_reader.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            on_output(true, &line);
                            stdout_output.push_str(&line);
                            stdout_output.push('\n');
                        }
                        Ok(None) => {}
                        Err(e) => {
                            error!("Error reading stdout: {}", e);
                        }
                    }
                }
                line = stderr_reader.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            on_output(false, &line);
                            stderr_output.push_str(&line);
                            stderr_output.push('\n');
                        }
                        Ok(None) => {}
                        Err(e) => {
                            error!("Error reading stderr: {}", e);
                        }
                    }
                }
                status = child.wait() => {
                    let status = status.map_err(BuildError::IoError)?;
                    let exit_code = status.code().unwrap_or(-1);

                    // Drain remaining output
                    while let Ok(Some(line)) = stdout_reader.next_line().await {
                        on_output(true, &line);
                        stdout_output.push_str(&line);
                        stdout_output.push('\n');
                    }
                    while let Ok(Some(line)) = stderr_reader.next_line().await {
                        on_output(false, &line);
                        stderr_output.push_str(&line);
                        stderr_output.push('\n');
                    }

                    return Ok(CommandOutput {
                        stdout: stdout_output,
                        stderr: stderr_output,
                        exit_code,
                    });
                }
            }
        }
    }

    /// Check if buildah is available
    pub async fn is_available(&self) -> bool {
        let cmd = BuildahCommand::new("version");
        self.execute(&cmd)
            .await
            .map(|o| o.success())
            .unwrap_or(false)
    }

    /// Get buildah version information
    pub async fn version(&self) -> Result<String> {
        let cmd = BuildahCommand::new("version");
        let output = self.execute_checked(&cmd).await?;
        Ok(output.stdout.trim().to_string())
    }
}

/// Find the buildah binary in PATH
fn which_buildah() -> Result<PathBuf> {
    // Check common locations
    let candidates = ["/usr/bin/buildah", "/usr/local/bin/buildah", "/bin/buildah"];

    for path in &candidates {
        let path = PathBuf::from(path);
        if path.exists() {
            return Ok(path);
        }
    }

    // Try using `which` command
    let output = std::process::Command::new("which")
        .arg("buildah")
        .output()
        .ok();

    if let Some(output) = output {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Ok(PathBuf::from(path));
            }
        }
    }

    Err(BuildError::IoError(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "buildah not found in PATH",
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_output_success() {
        let output = CommandOutput {
            stdout: "success".to_string(),
            stderr: String::new(),
            exit_code: 0,
        };
        assert!(output.success());
    }

    #[test]
    fn test_command_output_failure() {
        let output = CommandOutput {
            stdout: String::new(),
            stderr: "error".to_string(),
            exit_code: 1,
        };
        assert!(!output.success());
    }

    #[test]
    fn test_command_output_combined() {
        let output = CommandOutput {
            stdout: "out".to_string(),
            stderr: "err".to_string(),
            exit_code: 0,
        };
        assert_eq!(output.combined_output(), "out\nerr");
    }

    #[test]
    fn test_executor_builder() {
        let executor = BuildahExecutor::with_path("/custom/buildah")
            .storage_driver("overlay")
            .root("/var/lib/containers")
            .runroot("/run/containers");

        assert_eq!(executor.buildah_path, PathBuf::from("/custom/buildah"));
        assert_eq!(executor.storage_driver, Some("overlay".to_string()));
        assert_eq!(executor.root, Some(PathBuf::from("/var/lib/containers")));
        assert_eq!(executor.runroot, Some(PathBuf::from("/run/containers")));
    }

    // Integration tests would require buildah to be installed
    #[tokio::test]
    #[ignore = "requires buildah to be installed"]
    async fn test_execute_version() {
        let executor = BuildahExecutor::new().expect("buildah should be available");
        let version = executor.version().await.expect("should get version");
        assert!(!version.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires buildah to be installed"]
    async fn test_execute_streaming() {
        let executor = BuildahExecutor::new().expect("buildah should be available");
        let cmd = BuildahCommand::new("version");

        let mut lines = Vec::new();
        let output = executor
            .execute_streaming(&cmd, |_is_stdout, line| {
                lines.push(line.to_string());
            })
            .await
            .expect("should execute");

        assert!(output.success());
        assert!(!lines.is_empty());
    }
}
