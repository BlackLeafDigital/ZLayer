//! `zlayer self-update` -- in-process upgrade command.
//!
//! Fetches a newer zlayer release from GitHub Releases and atomically
//! replaces the currently running binary. Asset naming and the release
//! URL pattern match `install.sh`:
//!
//!   <https://github.com/BlackLeafDigital/ZLayer/releases/download/{tag}/zlayer-{version}-{os}-{arch}.{ext}>
//!
//! - `tag`     -- the GitHub release tag, e.g. `v0.11.24` (with `v` prefix).
//! - `version` -- the same tag without the leading `v`, e.g. `0.11.24`.
//! - `os`      -- `linux`, `darwin`, or `windows`.
//! - `arch`    -- `amd64` or `arm64`.
//! - `ext`     -- `tar.gz` on Linux/macOS, `zip` on Windows.
//!
//! SHA-256 sidecar verification (`{asset}.sha256`) is best-effort: if the
//! sidecar 404s we log a message and continue. If it exists and the digest
//! doesn't match we abort.

use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};

/// HTTP User-Agent sent to the GitHub API. GitHub rejects requests without
/// a UA, so build a recognisable one from the package version.
const USER_AGENT: &str = concat!("zlayer-self-update/", env!("CARGO_PKG_VERSION"));

/// Entry point for `zlayer self-update`.
pub async fn run(version: Option<String>, yes: bool, restart: bool, repo: &str) -> Result<()> {
    let current_exe = std::env::current_exe().context("Failed to resolve current executable")?;
    let (os, arch) = current_target()?;

    let current_version = env!("CARGO_PKG_VERSION").to_string();

    let tag_with_v = match version.as_deref() {
        Some(v) => {
            if v.starts_with('v') {
                v.to_string()
            } else {
                format!("v{v}")
            }
        }
        None => fetch_latest_tag(repo).await?,
    };
    let ver_no_v = strip_v_prefix(&tag_with_v).to_string();

    // Idempotent no-op: same version, no force. `--yes` doubles as "force"
    // for tests / explicit reinstall scenarios.
    if !yes && versions_equal(&current_version, &ver_no_v) {
        println!("Already on v{ver_no_v}; nothing to do.");
        return Ok(());
    }

    let asset = asset_filename(&ver_no_v, os, arch);
    let url = download_url(repo, &tag_with_v, &asset);

    if !yes {
        print!(
            "About to replace {} (v{current_version}) with v{ver_no_v}. Continue? [y/N] ",
            current_exe.display()
        );
        std::io::stdout().flush().ok();
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .context("Failed to read confirmation from stdin")?;
        let trimmed = line.trim().to_ascii_lowercase();
        if !(trimmed == "y" || trimmed == "yes") {
            eprintln!("Aborted.");
            return Ok(());
        }
    }

    let tmp = tempfile::TempDir::new().context("Failed to create temp directory")?;
    let archive_path = tmp.path().join(&asset);

    println!("Downloading {url}");
    download_to_file(&url, &archive_path).await?;

    // Optional integrity check via .sha256 sidecar.
    verify_sha256_if_available(&url, &archive_path).await?;

    println!("Extracting {asset}");
    let staged = extract_binary(&archive_path, tmp.path())?;

    // Ensure the staged binary is executable on Unix before we swap it in.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&staged)
            .with_context(|| format!("Failed to stat staged binary {}", staged.display()))?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&staged, perms).with_context(|| {
            format!(
                "Failed to set executable bit on staged binary {}",
                staged.display()
            )
        })?;
    }

    swap_binary(&staged, &current_exe)?;

    println!(
        "Replaced {} ({current_version} -> {ver_no_v})",
        current_exe.display()
    );

    if restart {
        restart_self(&current_exe);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Target detection
// ---------------------------------------------------------------------------

/// Resolve the current target as `(os, arch)` using the release asset's
/// naming convention.
///
/// Supported combinations:
/// - linux/macOS x amd64/arm64
/// - windows x amd64
fn current_target() -> Result<(&'static str, &'static str)> {
    let os = if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        bail!("self-update: unsupported OS (no published release asset)");
    };

    let arch = if cfg!(target_arch = "x86_64") {
        "amd64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        bail!("self-update: unsupported CPU architecture");
    };

    // No Windows arm64 release builds yet.
    if os == "windows" && arch != "amd64" {
        bail!("self-update: only windows-amd64 is published");
    }

    Ok((os, arch))
}

// ---------------------------------------------------------------------------
// URL / filename construction
// ---------------------------------------------------------------------------

fn asset_filename(ver_no_v: &str, os: &str, arch: &str) -> String {
    let ext = if os == "windows" { "zip" } else { "tar.gz" };
    format!("zlayer-{ver_no_v}-{os}-{arch}.{ext}")
}

fn download_url(repo: &str, tag_with_v: &str, asset: &str) -> String {
    format!("https://github.com/{repo}/releases/download/{tag_with_v}/{asset}")
}

fn strip_v_prefix(s: &str) -> &str {
    s.strip_prefix('v').unwrap_or(s)
}

/// Compare semver-ish strings ignoring a leading `v` on either side.
fn versions_equal(a: &str, b: &str) -> bool {
    strip_v_prefix(a) == strip_v_prefix(b)
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

async fn fetch_latest_tag(repo: &str) -> Result<String> {
    #[derive(serde::Deserialize)]
    struct LatestRelease {
        tag_name: String,
    }

    let api_url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .context("Failed to build reqwest client")?;

    let resp = client
        .get(&api_url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .with_context(|| format!("Failed to GET {api_url}"))?;
    if !resp.status().is_success() {
        bail!(
            "GitHub API returned HTTP {} for {api_url}",
            resp.status().as_u16()
        );
    }
    let body: LatestRelease = resp
        .json()
        .await
        .context("Failed to parse GitHub release JSON (missing tag_name?)")?;

    if body.tag_name.is_empty() {
        bail!("GitHub API returned an empty tag_name");
    }
    Ok(body.tag_name)
}

/// Stream-download `url` into `dest`. Chunked so we never buffer the
/// entire archive in memory.
async fn download_to_file(url: &str, dest: &Path) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .context("Failed to build reqwest client")?;

    let mut resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to GET {url}"))?;
    if !resp.status().is_success() {
        bail!("Download failed: HTTP {} for {url}", resp.status().as_u16());
    }

    let mut file = File::create(dest)
        .with_context(|| format!("Failed to create download file {}", dest.display()))?;
    while let Some(chunk) = resp
        .chunk()
        .await
        .with_context(|| format!("Failed to read response chunk from {url}"))?
    {
        file.write_all(&chunk)
            .with_context(|| format!("Failed to write to {}", dest.display()))?;
    }
    file.flush().ok();
    Ok(())
}

/// If `{url}.sha256` exists, parse its first whitespace-delimited token as
/// a hex digest and verify the downloaded file against it. 404 -> skip.
async fn verify_sha256_if_available(url: &str, file_path: &Path) -> Result<()> {
    let sha_url = format!("{url}.sha256");
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .context("Failed to build reqwest client")?;

    let resp = client
        .get(&sha_url)
        .send()
        .await
        .with_context(|| format!("Failed to GET {sha_url}"))?;

    if resp.status().as_u16() == 404 {
        eprintln!("note: no sha256 sidecar at {sha_url}; skipping integrity check");
        return Ok(());
    }
    if !resp.status().is_success() {
        eprintln!(
            "warning: sha256 sidecar fetch returned HTTP {} for {sha_url}; skipping integrity check",
            resp.status().as_u16()
        );
        return Ok(());
    }

    let body = resp
        .text()
        .await
        .with_context(|| format!("Failed to read body of {sha_url}"))?;
    let expected = body
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow!("sha256 sidecar at {sha_url} was empty"))?
        .to_ascii_lowercase();

    let actual = sha256_file(file_path)?;
    if actual != expected {
        bail!(
            "SHA-256 mismatch for downloaded artifact: expected {expected}, got {actual}\n\
             (sidecar: {sha_url})"
        );
    }
    println!("SHA-256 OK ({})", short_digest(&actual));
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let file = File::open(path)
        .with_context(|| format!("Failed to open {} for hashing", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    // 64 KiB read buffer on the heap -- well above clippy's 16 KiB stack-array
    // threshold but small enough to keep allocator pressure trivial.
    let mut buf = vec![0u8; 64 * 1024].into_boxed_slice();
    loop {
        let n = reader
            .read(&mut buf)
            .with_context(|| format!("Failed to read {} for hashing", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn short_digest(hex: &str) -> &str {
    if hex.len() > 12 {
        &hex[..12]
    } else {
        hex
    }
}

// ---------------------------------------------------------------------------
// Archive extraction
// ---------------------------------------------------------------------------

/// Extract the `zlayer` binary from the downloaded archive into `tmp_dir`
/// and return the staged path.
fn extract_binary(archive: &Path, tmp_dir: &Path) -> Result<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        extract_zip_binary(archive, tmp_dir)
    }
    #[cfg(not(target_os = "windows"))]
    {
        extract_targz_binary(archive, tmp_dir)
    }
}

#[cfg(not(target_os = "windows"))]
fn extract_targz_binary(archive: &Path, tmp_dir: &Path) -> Result<PathBuf> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let file = File::open(archive)
        .with_context(|| format!("Failed to open archive {}", archive.display()))?;
    let mut tar = Archive::new(GzDecoder::new(file));

    let dest = tmp_dir.join("zlayer-new");
    let mut found = false;
    for entry in tar
        .entries()
        .with_context(|| format!("Failed to read tar entries from {}", archive.display()))?
    {
        let mut entry =
            entry.with_context(|| format!("Failed to read tar entry in {}", archive.display()))?;
        let path = entry
            .path()
            .with_context(|| format!("Bad tar entry path in {}", archive.display()))?
            .into_owned();
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if name == "zlayer" {
            entry.unpack(&dest).with_context(|| {
                format!("Failed to unpack {} to {}", path.display(), dest.display())
            })?;
            found = true;
            break;
        }
    }
    if !found {
        bail!(
            "archive {} did not contain a `zlayer` binary",
            archive.display()
        );
    }
    Ok(dest)
}

#[cfg(target_os = "windows")]
fn extract_zip_binary(archive: &Path, tmp_dir: &Path) -> Result<PathBuf> {
    use std::io::copy;

    let file = File::open(archive)
        .with_context(|| format!("Failed to open archive {}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(file)
        .with_context(|| format!("Failed to read zip archive {}", archive.display()))?;

    let dest = tmp_dir.join("zlayer.exe.new");
    let mut found = false;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .with_context(|| format!("Failed to read zip entry {i}"))?;
        let name = entry
            .enclosed_name()
            .and_then(|p| p.file_name().map(|s| s.to_owned()))
            .and_then(|s| s.into_string().ok())
            .unwrap_or_default();
        if name.eq_ignore_ascii_case("zlayer.exe") {
            let mut out = File::create(&dest)
                .with_context(|| format!("Failed to create staged binary {}", dest.display()))?;
            copy(&mut entry, &mut out)
                .with_context(|| format!("Failed to extract {} to {}", name, dest.display()))?;
            found = true;
            break;
        }
    }
    if !found {
        bail!(
            "archive {} did not contain a `zlayer.exe` binary",
            archive.display()
        );
    }
    Ok(dest)
}

// ---------------------------------------------------------------------------
// Atomic swap
// ---------------------------------------------------------------------------

/// Replace the currently running binary on disk.
///
/// On Unix `rename(2)` onto a running binary is safe: the kernel keeps the
/// old text segment alive via the open file descriptor, and the inode flips
/// to the new file atomically. On Windows the equivalent isn't possible
/// while the binary is mapped, so we leave a sibling `.new` and ask the
/// user to restart.
fn swap_binary(staged: &Path, current_exe: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        fs::rename(staged, current_exe).with_context(|| {
            format!(
                "Failed to rename {} onto {}",
                staged.display(),
                current_exe.display()
            )
        })?;
        Ok(())
    }
    #[cfg(windows)]
    {
        // Best-effort: try a direct rename first (works if the binary isn't
        // running, e.g. invoked via a launcher). Fall back to staging next
        // to the current exe and instructing the user to restart.
        if fs::rename(staged, current_exe).is_ok() {
            return Ok(());
        }
        let sibling = current_exe.with_extension("exe.new");
        fs::copy(staged, &sibling).with_context(|| {
            format!(
                "Failed to stage new binary at {} (could not swap {} either)",
                sibling.display(),
                current_exe.display()
            )
        })?;
        eprintln!(
            "Windows could not replace the running binary in-place. New \
             binary staged at {}. Stop zlayer and rename it over {} to \
             complete the update.",
            sibling.display(),
            current_exe.display()
        );
        Ok(())
    }
}

/// Re-exec the new binary with `--version` to demonstrate that the swap
/// took effect. Daemon resumption is out of scope: the caller (or a
/// supervisor like systemd) is responsible for restarting long-running
/// processes with their original arguments.
#[allow(clippy::needless_pass_by_value)]
fn restart_self(current_exe: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(current_exe)
            .arg("--version")
            .exec();
        // `exec` only returns on failure.
        eprintln!("Failed to re-exec {}: {err}", current_exe.display());
    }
    #[cfg(windows)]
    {
        let _ = current_exe;
        eprintln!("Restart not yet supported on Windows; please re-run zlayer manually.");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_target_returns_supported_tuple_or_errors() {
        // We don't know the host at compile-time of this comment, but we do
        // know the supported matrix: assert we land in it or get a clean
        // error on uncommon platforms.
        if let Ok((os, arch)) = current_target() {
            let ok = matches!(
                (os, arch),
                ("linux" | "darwin" | "windows", "amd64") | ("linux" | "darwin", "arm64")
            );
            assert!(ok, "unexpected (os, arch) tuple: ({os}, {arch})");
        }
        // Err is acceptable on uncommon hosts (e.g. freebsd, riscv).
    }

    #[test]
    fn asset_filename_linux_amd64() {
        assert_eq!(
            asset_filename("0.11.24", "linux", "amd64"),
            "zlayer-0.11.24-linux-amd64.tar.gz"
        );
    }

    #[test]
    fn asset_filename_darwin_arm64() {
        assert_eq!(
            asset_filename("0.12.0", "darwin", "arm64"),
            "zlayer-0.12.0-darwin-arm64.tar.gz"
        );
    }

    #[test]
    fn asset_filename_windows_amd64_is_zip() {
        assert_eq!(
            asset_filename("0.12.0", "windows", "amd64"),
            "zlayer-0.12.0-windows-amd64.zip"
        );
    }

    #[test]
    fn download_url_includes_tag_and_asset() {
        let url = download_url(
            "BlackLeafDigital/ZLayer",
            "v0.11.24",
            "zlayer-0.11.24-linux-amd64.tar.gz",
        );
        assert_eq!(
            url,
            "https://github.com/BlackLeafDigital/ZLayer/releases/download/v0.11.24/zlayer-0.11.24-linux-amd64.tar.gz"
        );
    }

    #[test]
    fn strip_v_prefix_idempotent() {
        assert_eq!(strip_v_prefix("v1.2.3"), "1.2.3");
        assert_eq!(strip_v_prefix("1.2.3"), "1.2.3");
    }

    #[test]
    fn versions_equal_handles_v_prefix() {
        assert!(versions_equal("v1.2.3", "1.2.3"));
        assert!(versions_equal("1.2.3", "v1.2.3"));
        assert!(!versions_equal("v1.2.3", "v1.2.4"));
    }

    #[test]
    fn sha256_file_matches_known_digest() {
        // sha256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut f = tmp.reopen().unwrap();
        f.write_all(b"hello").unwrap();
        f.flush().unwrap();

        let digest = sha256_file(tmp.path()).unwrap();
        assert_eq!(
            digest,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn short_digest_truncates_to_12() {
        let d = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        assert_eq!(short_digest(d), "2cf24dba5fb0");
        assert_eq!(short_digest("abc"), "abc");
    }
}
