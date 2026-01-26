//! Buildah installation and discovery
//!
//! This module provides functionality to find existing buildah installations
//! or provide helpful error messages for installing buildah on various platforms.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;
use tracing::{debug, info, trace, warn};

/// Minimum required buildah version
const MIN_BUILDAH_VERSION: &str = "1.28.0";

/// Buildah installation manager
///
/// Handles finding existing buildah installations and providing
/// installation guidance when buildah is not found.
#[derive(Debug, Clone)]
pub struct BuildahInstaller {
    /// Where to store downloaded buildah binary (for future use)
    install_dir: PathBuf,
    /// Minimum required buildah version
    min_version: &'static str,
}

/// Information about a discovered buildah installation
#[derive(Debug, Clone)]
pub struct BuildahInstallation {
    /// Path to buildah binary
    pub path: PathBuf,
    /// Installed version
    pub version: String,
}

/// Errors that can occur during buildah installation/discovery
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    /// Buildah was not found on the system
    #[error("Buildah not found. {}", install_instructions())]
    NotFound,

    /// Buildah version is too old
    #[error("Buildah version {found} is below minimum required version {required}")]
    VersionTooOld {
        /// The version that was found
        found: String,
        /// The minimum required version
        required: String,
    },

    /// Platform is not supported
    #[error("Unsupported platform: {os}/{arch}")]
    UnsupportedPlatform {
        /// Operating system
        os: String,
        /// CPU architecture
        arch: String,
    },

    /// Download failed (for future binary download support)
    #[error("Download failed: {0}")]
    DownloadFailed(String),

    /// IO error during installation
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Failed to parse version output
    #[error("Failed to parse buildah version output: {0}")]
    VersionParse(String),

    /// Failed to execute buildah
    #[error("Failed to execute buildah: {0}")]
    ExecutionFailed(String),
}

impl Default for BuildahInstaller {
    fn default() -> Self {
        Self::new()
    }
}

impl BuildahInstaller {
    /// Create installer with default paths
    ///
    /// User install directory: `~/.zlayer/bin/`
    /// System install directory: `/usr/local/lib/zlayer/`
    pub fn new() -> Self {
        let install_dir = default_install_dir();
        Self {
            install_dir,
            min_version: MIN_BUILDAH_VERSION,
        }
    }

    /// Create with custom install directory
    pub fn with_install_dir(dir: PathBuf) -> Self {
        Self {
            install_dir: dir,
            min_version: MIN_BUILDAH_VERSION,
        }
    }

    /// Get the install directory
    pub fn install_dir(&self) -> &Path {
        &self.install_dir
    }

    /// Get the minimum required version
    pub fn min_version(&self) -> &str {
        self.min_version
    }

    /// Find existing buildah installation
    ///
    /// Searches for buildah in the following locations (in order):
    /// 1. PATH environment variable
    /// 2. `~/.zlayer/bin/buildah`
    /// 3. `/usr/local/lib/zlayer/buildah`
    /// 4. `/usr/bin/buildah`
    /// 5. `/usr/local/bin/buildah`
    pub async fn find_existing(&self) -> Option<BuildahInstallation> {
        // Search paths in priority order
        let search_paths = get_search_paths(&self.install_dir);

        for path in search_paths {
            trace!("Checking for buildah at: {}", path.display());

            if path.exists() && path.is_file() {
                match Self::get_version(&path).await {
                    Ok(version) => {
                        info!("Found buildah at {} (version {})", path.display(), version);
                        return Some(BuildahInstallation { path, version });
                    }
                    Err(e) => {
                        debug!(
                            "Found buildah at {} but couldn't get version: {}",
                            path.display(),
                            e
                        );
                    }
                }
            }
        }

        // Also try using `which` to find buildah in PATH
        if let Some(path) = find_in_path("buildah").await {
            match Self::get_version(&path).await {
                Ok(version) => {
                    info!(
                        "Found buildah in PATH at {} (version {})",
                        path.display(),
                        version
                    );
                    return Some(BuildahInstallation { path, version });
                }
                Err(e) => {
                    debug!(
                        "Found buildah in PATH at {} but couldn't get version: {}",
                        path.display(),
                        e
                    );
                }
            }
        }

        None
    }

    /// Check if buildah is installed and meets version requirements
    ///
    /// Returns the installation if found and valid, otherwise returns an error.
    pub async fn check(&self) -> Result<BuildahInstallation, InstallError> {
        let installation = self.find_existing().await.ok_or(InstallError::NotFound)?;

        // Check version meets minimum requirements
        if !version_meets_minimum(&installation.version, self.min_version) {
            return Err(InstallError::VersionTooOld {
                found: installation.version,
                required: self.min_version.to_string(),
            });
        }

        Ok(installation)
    }

    /// Get buildah version from a binary
    ///
    /// Runs `buildah --version` and parses the output.
    /// Expected format: "buildah version 1.33.0 (image-spec 1.0.2-dev, runtime-spec 1.0.2-dev)"
    pub async fn get_version(path: &Path) -> Result<String, InstallError> {
        let output = Command::new(path)
            .arg("--version")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| InstallError::ExecutionFailed(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(InstallError::ExecutionFailed(format!(
                "buildah --version failed: {}",
                stderr.trim()
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_version(&stdout)
    }

    /// Ensure buildah is available (find existing or return helpful error)
    ///
    /// This is the primary entry point for ensuring buildah is available.
    /// If buildah is not found, it returns an error with installation instructions.
    pub async fn ensure(&self) -> Result<BuildahInstallation, InstallError> {
        // First try to find existing installation
        match self.check().await {
            Ok(installation) => {
                info!(
                    "Using buildah {} at {}",
                    installation.version,
                    installation.path.display()
                );
                Ok(installation)
            }
            Err(InstallError::VersionTooOld { found, required }) => {
                warn!(
                    "Found buildah {} but minimum required version is {}",
                    found, required
                );
                Err(InstallError::VersionTooOld { found, required })
            }
            Err(InstallError::NotFound) => {
                warn!("Buildah not found on system");
                Err(InstallError::NotFound)
            }
            Err(e) => Err(e),
        }
    }

    /// Download buildah binary for current platform
    ///
    /// Currently returns an error with installation instructions.
    /// Future versions may download static binaries from GitHub releases.
    pub async fn download(&self) -> Result<BuildahInstallation, InstallError> {
        let (os, arch) = current_platform();

        // Check if platform is supported
        if !is_platform_supported() {
            return Err(InstallError::UnsupportedPlatform {
                os: os.to_string(),
                arch: arch.to_string(),
            });
        }

        // For now, return helpful error with installation instructions
        // Future: Download static binary from GitHub releases
        Err(InstallError::DownloadFailed(format!(
            "Automatic download not yet implemented. {}",
            install_instructions()
        )))
    }
}

/// Get the current platform (OS and architecture)
pub fn current_platform() -> (&'static str, &'static str) {
    let os = std::env::consts::OS; // "linux", "macos", "windows"
    let arch = std::env::consts::ARCH; // "x86_64", "aarch64"
    (os, arch)
}

/// Check if the current platform is supported for buildah
///
/// Buildah is primarily a Linux tool, though it can work on macOS
/// through virtualization.
pub fn is_platform_supported() -> bool {
    let (os, arch) = current_platform();
    matches!(
        (os, arch),
        ("linux", "x86_64" | "aarch64") | ("macos", "x86_64" | "aarch64")
    )
}

/// Get installation instructions for the current platform
pub fn install_instructions() -> String {
    let (os, _arch) = current_platform();

    match os {
        "linux" => {
            // Try to detect the Linux distribution
            if let Some(distro) = detect_linux_distro() {
                match distro.as_str() {
                    "ubuntu" | "debian" | "pop" | "mint" | "elementary" => {
                        "Install with: sudo apt install buildah".to_string()
                    }
                    "fedora" | "rhel" | "centos" | "rocky" | "alma" => {
                        "Install with: sudo dnf install buildah".to_string()
                    }
                    "arch" | "manjaro" | "endeavouros" => {
                        "Install with: sudo pacman -S buildah".to_string()
                    }
                    "opensuse" | "suse" => "Install with: sudo zypper install buildah".to_string(),
                    "alpine" => "Install with: sudo apk add buildah".to_string(),
                    "gentoo" => "Install with: sudo emerge app-containers/buildah".to_string(),
                    "void" => "Install with: sudo xbps-install buildah".to_string(),
                    _ => "Install buildah using your distribution's package manager.\n\
                         Common commands:\n\
                         - Debian/Ubuntu: sudo apt install buildah\n\
                         - Fedora/RHEL: sudo dnf install buildah\n\
                         - Arch: sudo pacman -S buildah\n\
                         - openSUSE: sudo zypper install buildah"
                        .to_string(),
                }
            } else {
                "Install buildah using your distribution's package manager.\n\
                 Common commands:\n\
                 - Debian/Ubuntu: sudo apt install buildah\n\
                 - Fedora/RHEL: sudo dnf install buildah\n\
                 - Arch: sudo pacman -S buildah\n\
                 - openSUSE: sudo zypper install buildah"
                    .to_string()
            }
        }
        "macos" => "Install with: brew install buildah\n\
             Note: Buildah on macOS requires a Linux VM for container operations."
            .to_string(),
        "windows" => "Buildah is not natively supported on Windows.\n\
             Consider using WSL2 with a Linux distribution and installing buildah there."
            .to_string(),
        _ => format!("Buildah is not supported on {os}. Use a Linux system."),
    }
}

// ============================================================================
// Internal Helper Functions
// ============================================================================

/// Get the default install directory for ZLayer binaries
fn default_install_dir() -> PathBuf {
    // Try user directory first
    if let Some(home) = dirs::home_dir() {
        return home.join(".zlayer").join("bin");
    }

    // Fall back to system directory
    PathBuf::from("/usr/local/lib/zlayer")
}

/// Get all paths to search for buildah
fn get_search_paths(install_dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // Custom install directory
    paths.push(install_dir.join("buildah"));

    // User ZLayer directory
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".zlayer").join("bin").join("buildah"));
    }

    // System ZLayer directory
    paths.push(PathBuf::from("/usr/local/lib/zlayer/buildah"));

    // Standard system paths
    paths.push(PathBuf::from("/usr/bin/buildah"));
    paths.push(PathBuf::from("/usr/local/bin/buildah"));
    paths.push(PathBuf::from("/bin/buildah"));

    // Deduplicate while preserving order
    let mut seen = std::collections::HashSet::new();
    paths.retain(|p| seen.insert(p.clone()));

    paths
}

/// Find a binary in PATH using the `which` command
async fn find_in_path(binary: &str) -> Option<PathBuf> {
    let output = Command::new("which")
        .arg(binary)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;

    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }

    // Also try `command -v` as a fallback (works in more shells)
    let output = Command::new("sh")
        .args(["-c", &format!("command -v {}", binary)])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;

    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }

    None
}

/// Parse version from buildah --version output
///
/// Expected formats:
/// - "buildah version 1.33.0 (image-spec 1.0.2-dev, runtime-spec 1.0.2-dev)"
/// - "buildah version 1.33.0"
/// - "buildah version 1.34.0-dev"
fn parse_version(output: &str) -> Result<String, InstallError> {
    // Look for "buildah version X.Y.Z" pattern
    let output = output.trim();

    // Try to extract version after "version"
    if let Some(pos) = output.to_lowercase().find("version") {
        let after_version = &output[pos + "version".len()..].trim_start();

        // Extract the version number (digits, dots, and optional suffix like -dev, -rc1)
        // Version format: X.Y.Z or X.Y.Z-suffix
        let version: String = after_version
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
            .collect();

        // Clean up: trim trailing hyphens if any
        let version = version.trim_end_matches('-');

        if !version.is_empty() && version.contains('.') {
            return Ok(version.to_string());
        }
    }

    Err(InstallError::VersionParse(format!(
        "Could not parse version from: {}",
        output
    )))
}

/// Check if a version string meets the minimum requirement
///
/// Uses simple semantic version comparison.
fn version_meets_minimum(version: &str, minimum: &str) -> bool {
    let parse_version = |s: &str| -> Option<Vec<u32>> {
        // Strip any trailing non-numeric parts (like "-dev")
        let clean = s.split('-').next()?;
        clean
            .split('.')
            .map(|p| p.parse::<u32>().ok())
            .collect::<Option<Vec<_>>>()
    };

    let version_parts = match parse_version(version) {
        Some(v) => v,
        None => return false,
    };

    let minimum_parts = match parse_version(minimum) {
        Some(v) => v,
        None => return true, // If we can't parse minimum, assume it's met
    };

    // Compare version parts
    for (v, m) in version_parts.iter().zip(minimum_parts.iter()) {
        match v.cmp(m) {
            std::cmp::Ordering::Greater => return true,
            std::cmp::Ordering::Less => return false,
            std::cmp::Ordering::Equal => continue,
        }
    }

    // If all compared parts are equal, check if version has at least as many parts
    version_parts.len() >= minimum_parts.len()
}

/// Detect the Linux distribution
fn detect_linux_distro() -> Option<String> {
    // Try /etc/os-release first (most modern distros)
    if let Ok(contents) = std::fs::read_to_string("/etc/os-release") {
        for line in contents.lines() {
            if let Some(id) = line.strip_prefix("ID=") {
                return Some(id.trim_matches('"').to_lowercase());
            }
        }
    }

    // Try /etc/lsb-release as fallback
    if let Ok(contents) = std::fs::read_to_string("/etc/lsb-release") {
        for line in contents.lines() {
            if let Some(id) = line.strip_prefix("DISTRIB_ID=") {
                return Some(id.trim_matches('"').to_lowercase());
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version_full() {
        let output = "buildah version 1.33.0 (image-spec 1.0.2-dev, runtime-spec 1.0.2-dev)";
        let version = parse_version(output).unwrap();
        assert_eq!(version, "1.33.0");
    }

    #[test]
    fn test_parse_version_simple() {
        let output = "buildah version 1.28.0";
        let version = parse_version(output).unwrap();
        assert_eq!(version, "1.28.0");
    }

    #[test]
    fn test_parse_version_with_newline() {
        let output = "buildah version 1.33.0\n";
        let version = parse_version(output).unwrap();
        assert_eq!(version, "1.33.0");
    }

    #[test]
    fn test_parse_version_dev() {
        let output = "buildah version 1.34.0-dev";
        let version = parse_version(output).unwrap();
        assert_eq!(version, "1.34.0-dev");
    }

    #[test]
    fn test_parse_version_invalid() {
        let output = "some random output";
        assert!(parse_version(output).is_err());
    }

    #[test]
    fn test_version_meets_minimum_equal() {
        assert!(version_meets_minimum("1.28.0", "1.28.0"));
    }

    #[test]
    fn test_version_meets_minimum_greater_patch() {
        assert!(version_meets_minimum("1.28.1", "1.28.0"));
    }

    #[test]
    fn test_version_meets_minimum_greater_minor() {
        assert!(version_meets_minimum("1.29.0", "1.28.0"));
    }

    #[test]
    fn test_version_meets_minimum_greater_major() {
        assert!(version_meets_minimum("2.0.0", "1.28.0"));
    }

    #[test]
    fn test_version_meets_minimum_less_patch() {
        assert!(!version_meets_minimum("1.27.5", "1.28.0"));
    }

    #[test]
    fn test_version_meets_minimum_less_minor() {
        assert!(!version_meets_minimum("1.20.0", "1.28.0"));
    }

    #[test]
    fn test_version_meets_minimum_less_major() {
        assert!(!version_meets_minimum("0.99.0", "1.28.0"));
    }

    #[test]
    fn test_version_meets_minimum_with_dev_suffix() {
        assert!(version_meets_minimum("1.34.0-dev", "1.28.0"));
    }

    #[test]
    fn test_current_platform() {
        let (os, arch) = current_platform();
        // Just verify it returns non-empty strings
        assert!(!os.is_empty());
        assert!(!arch.is_empty());
    }

    #[test]
    fn test_is_platform_supported() {
        // This test will pass on Linux and macOS x86_64/aarch64
        let (os, arch) = current_platform();
        let supported = is_platform_supported();

        match (os, arch) {
            ("linux", "x86_64" | "aarch64") => assert!(supported),
            ("macos", "x86_64" | "aarch64") => assert!(supported),
            _ => assert!(!supported),
        }
    }

    #[test]
    fn test_install_instructions_not_empty() {
        let instructions = install_instructions();
        assert!(!instructions.is_empty());
    }

    #[test]
    fn test_default_install_dir() {
        let dir = default_install_dir();
        // Should end with bin
        assert!(dir.to_string_lossy().contains("bin") || dir.to_string_lossy().contains("zlayer"));
    }

    #[test]
    fn test_get_search_paths_not_empty() {
        let install_dir = PathBuf::from("/tmp/zlayer-test");
        let paths = get_search_paths(&install_dir);
        assert!(!paths.is_empty());
        // First path should be in our custom install dir
        assert!(paths[0].starts_with("/tmp/zlayer-test"));
    }

    #[test]
    fn test_installer_creation() {
        let installer = BuildahInstaller::new();
        assert_eq!(installer.min_version(), MIN_BUILDAH_VERSION);
    }

    #[test]
    fn test_installer_with_custom_dir() {
        let custom_dir = PathBuf::from("/custom/path");
        let installer = BuildahInstaller::with_install_dir(custom_dir.clone());
        assert_eq!(installer.install_dir(), custom_dir);
    }

    #[tokio::test]
    async fn test_find_in_path_nonexistent() {
        // This binary should not exist
        let result = find_in_path("this_binary_should_not_exist_12345").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_find_in_path_exists() {
        // 'sh' should exist on any Unix system
        let result = find_in_path("sh").await;
        assert!(result.is_some());
    }

    // Integration test - only runs if buildah is installed
    #[tokio::test]
    #[ignore = "requires buildah to be installed"]
    async fn test_find_existing_buildah() {
        let installer = BuildahInstaller::new();
        let result = installer.find_existing().await;
        assert!(result.is_some());
        let installation = result.unwrap();
        assert!(installation.path.exists());
        assert!(!installation.version.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires buildah to be installed"]
    async fn test_check_buildah() {
        let installer = BuildahInstaller::new();
        let result = installer.check().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    #[ignore = "requires buildah to be installed"]
    async fn test_ensure_buildah() {
        let installer = BuildahInstaller::new();
        let result = installer.ensure().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    #[ignore = "requires buildah to be installed"]
    async fn test_get_version() {
        let installer = BuildahInstaller::new();
        if let Some(installation) = installer.find_existing().await {
            let version = BuildahInstaller::get_version(&installation.path).await;
            assert!(version.is_ok());
            let version = version.unwrap();
            assert!(version.contains('.'));
        }
    }
}
