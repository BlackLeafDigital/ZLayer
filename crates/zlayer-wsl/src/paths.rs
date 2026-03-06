//! Windows <-> WSL2 path translation.
//!
//! Converts Windows paths (e.g., `C:\Users\zach\project`) to their
//! WSL2 equivalents (`/mnt/c/Users/zach/project`) and vice versa.

use std::path::{Path, PathBuf};

/// Convert a Windows path to its WSL2 equivalent.
///
/// `C:\Users\zach\project` -> `/mnt/c/Users/zach/project`
///
/// Returns `None` if the path doesn't have a recognizable Windows drive letter.
#[must_use]
pub fn windows_to_wsl(path: &Path) -> Option<String> {
    let path_str = path.to_string_lossy();

    // Handle UNC paths like \\wsl$\distro\path
    if path_str.starts_with(r"\\wsl$\") || path_str.starts_with(r"\\wsl.localhost\") {
        // Already a WSL path reference -- extract the inner path
        let parts: Vec<&str> = path_str.splitn(4, '\\').collect();
        if parts.len() >= 4 {
            return Some(format!("/{}", parts[3].replace('\\', "/")));
        }
        return None;
    }

    // Handle drive letter paths: C:\foo\bar -> /mnt/c/foo/bar
    let mut chars = path_str.chars();
    let drive = chars.next()?;
    if !drive.is_ascii_alphabetic() {
        return None;
    }
    let colon = chars.next()?;
    if colon != ':' {
        return None;
    }

    let rest = &path_str[2..];
    let unix_rest = rest.replace('\\', "/");
    Some(format!("/mnt/{}{unix_rest}", drive.to_ascii_lowercase()))
}

/// Convert a WSL2 path to its Windows UNC equivalent.
///
/// `/home/zlayer/data` -> `\\wsl$\zlayer\home\zlayer\data`
#[must_use]
pub fn wsl_to_windows(wsl_path: &str, distro: &str) -> PathBuf {
    let windows_path = wsl_path.replace('/', "\\");
    PathBuf::from(format!(r"\\wsl$\{distro}{windows_path}"))
}

/// Check if a path looks like a Windows path (has drive letter).
#[must_use]
pub fn is_windows_path(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.len() >= 2 && s.as_bytes()[0].is_ascii_alphabetic() && s.as_bytes()[1] == b':'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_windows_to_wsl_drive_letter() {
        let path = Path::new(r"C:\Users\zach\project");
        assert_eq!(
            windows_to_wsl(path),
            Some("/mnt/c/Users/zach/project".to_string())
        );
    }

    #[test]
    fn test_windows_to_wsl_lowercase() {
        let path = Path::new(r"D:\data\files");
        assert_eq!(windows_to_wsl(path), Some("/mnt/d/data/files".to_string()));
    }

    #[test]
    fn test_windows_to_wsl_root() {
        let path = Path::new(r"C:\");
        assert_eq!(windows_to_wsl(path), Some("/mnt/c/".to_string()));
    }

    #[test]
    fn test_windows_to_wsl_unix_path_returns_none() {
        let path = Path::new("/usr/local/bin");
        assert_eq!(windows_to_wsl(path), None);
    }

    #[test]
    fn test_wsl_to_windows() {
        let result = wsl_to_windows("/var/lib/zlayer", "zlayer");
        assert_eq!(result, PathBuf::from(r"\\wsl$\zlayer\var\lib\zlayer"));
    }

    #[test]
    fn test_is_windows_path() {
        assert!(is_windows_path(Path::new(r"C:\Users")));
        assert!(is_windows_path(Path::new(r"D:\data")));
        assert!(!is_windows_path(Path::new("/usr/local")));
        assert!(!is_windows_path(Path::new("relative/path")));
    }
}
