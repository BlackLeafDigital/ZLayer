//! Idempotent merger for `%UserProfile%\.wslconfig` that sets the WSL2 VHD cap and sparse flag.
//!
//! The `.wslconfig` file controls global WSL2 settings for the current user. `ZLayer` needs
//! `[wsl2] vhdSize` to be large enough on big disks and `sparseVhd = true` so the virtual
//! disk reclaims space. This module parses the existing file with `toml_edit` to preserve
//! comments and unrelated keys, merges only the two values it owns, and writes atomically.

// The merge helpers are exercised by cross-platform tests and by the Windows-gated
// `ensure_wslconfig` path; on a non-Windows release build they appear unused to the
// lib target even though they are live code.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// Minimum VHD cap in GiB. We never write a value below this.
const MIN_FLOOR_GB: u64 = 64;

/// Fraction of free space (as a percentage) to use when computing the default cap.
const DEFAULT_FREE_SPACE_PERCENT: u64 = 80;

/// Environment variable override for the VHD cap in GiB.
const ENV_OVERRIDE: &str = "ZLAYER_WSL_VHD_GB";

/// Result of an [`ensure_wslconfig`] call.
#[derive(Debug, Clone)]
pub struct WslconfigOutcome {
    /// Absolute path to the `.wslconfig` file we targeted.
    pub path: PathBuf,
    /// The previously-set cap in GiB, or `None` if the key was absent before.
    pub previous_cap_gb: Option<u64>,
    /// The cap in GiB that is present in the file after the call (whether we wrote or not).
    pub new_cap_gb: u64,
    /// Whether we actually wrote bytes to disk.
    pub changed: bool,
}

/// Ensure `%UserProfile%\.wslconfig` has `[wsl2] vhdSize=<N>GB` and `sparseVhd=true`.
///
/// Merge semantics: preserve every key the caller has already set in every section.
/// Only `[wsl2] vhdSize` and `[wsl2] sparseVhd` are touched. If the `[wsl2]` section
/// is absent, it is created at the end of the file. If `vhdSize` is already equal to
/// or greater than `min_gb`, it is left unchanged (only the floor is raised).
///
/// The write is atomic: bytes go to `.wslconfig.tmp` next to the target and are then
/// renamed into place.
///
/// # Errors
///
/// Returns an error when the `USERPROFILE` environment variable is missing, when the
/// existing `.wslconfig` cannot be read or parsed, or when the atomic write fails.
#[cfg(target_os = "windows")]
pub async fn ensure_wslconfig(min_gb: u64) -> anyhow::Result<WslconfigOutcome> {
    let profile = std::env::var_os("USERPROFILE").ok_or_else(|| {
        anyhow::anyhow!("cannot determine user profile path; %USERPROFILE% is not set")
    })?;
    let path = PathBuf::from(profile).join(".wslconfig");

    let existing = match tokio::fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(
                anyhow::Error::from(e).context(format!("failed to read {}", path.display()))
            );
        }
    };

    let previous_cap_gb = parse_vhd_size_gb(&existing);
    let (merged, changed) = merge_doc(&existing, min_gb);
    let new_cap_gb = parse_vhd_size_gb(&merged).unwrap_or(min_gb);

    if changed {
        let tmp = path.with_extension("wslconfig.tmp");
        tokio::fs::write(&tmp, merged.as_bytes())
            .await
            .map_err(|e| {
                anyhow::Error::from(e).context(format!("failed to write {}", tmp.display()))
            })?;
        tokio::fs::rename(&tmp, &path).await.map_err(|e| {
            anyhow::Error::from(e).context(format!("failed to rename into {}", path.display()))
        })?;
        tracing::info!(
            path = %path.display(),
            previous_cap_gb = ?previous_cap_gb,
            new_cap_gb,
            "updated .wslconfig"
        );
    } else {
        tracing::debug!(path = %path.display(), "no changes to .wslconfig");
    }

    Ok(WslconfigOutcome {
        path,
        previous_cap_gb,
        new_cap_gb,
        changed,
    })
}

/// Non-Windows stub — `.wslconfig` has no meaning off Windows.
///
/// # Errors
///
/// Always returns an error on non-Windows platforms.
#[cfg(not(target_os = "windows"))]
#[allow(clippy::unused_async)]
pub async fn ensure_wslconfig(_min_gb: u64) -> anyhow::Result<WslconfigOutcome> {
    anyhow::bail!("WSL2 configuration is only available on Windows")
}

/// Compute the default cap in GiB: approximately 80% of free space on the install drive,
/// floored at 64 GiB. Honors `ZLAYER_WSL_VHD_GB` as a hard override (parsed as u64 GiB).
#[cfg(target_os = "windows")]
#[must_use]
pub fn compute_default_gb(install_dir: &Path) -> u64 {
    if let Some(v) = env_override() {
        return v.max(MIN_FLOOR_GB);
    }

    let probe = walk_up_to_existing(install_dir);
    match probe.and_then(|p| fs4::available_space(p).ok()) {
        Some(bytes) => {
            let gib = bytes / (1024 * 1024 * 1024);
            let capped = gib.saturating_mul(DEFAULT_FREE_SPACE_PERCENT) / 100;
            capped.max(MIN_FLOOR_GB)
        }
        None => {
            tracing::warn!(
                install_dir = %install_dir.display(),
                "could not read free disk space; falling back to {}GB cap",
                MIN_FLOOR_GB
            );
            MIN_FLOOR_GB
        }
    }
}

/// Non-Windows stub — returns the floor so callers can treat the value uniformly.
#[cfg(not(target_os = "windows"))]
#[must_use]
pub fn compute_default_gb(_install_dir: &Path) -> u64 {
    if let Some(v) = env_override() {
        return v.max(MIN_FLOOR_GB);
    }
    MIN_FLOOR_GB
}

fn env_override() -> Option<u64> {
    std::env::var(ENV_OVERRIDE)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
}

fn walk_up_to_existing(start: &Path) -> Option<PathBuf> {
    let mut cur = Some(start);
    while let Some(p) = cur {
        if p.exists() {
            return Some(p.to_path_buf());
        }
        cur = p.parent();
    }
    None
}

/// Merge the target keys into an existing `.wslconfig` body. Returns the new body
/// and whether it differs from the original. This function is pure and has no I/O
/// so it can be exercised by cross-platform unit tests.
fn merge_doc(existing: &str, min_gb: u64) -> (String, bool) {
    let mut doc = match existing.parse::<toml_edit::DocumentMut>() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "failed to parse existing .wslconfig; starting from empty document"
            );
            toml_edit::DocumentMut::new()
        }
    };

    ensure_wsl2_table(&mut doc);

    let wsl2 = doc["wsl2"]
        .as_table_mut()
        .expect("wsl2 table ensured above");

    apply_vhd_size(wsl2, min_gb);
    apply_sparse_vhd(wsl2);

    let rendered = doc.to_string();
    let changed = rendered != existing;
    (rendered, changed)
}

fn ensure_wsl2_table(doc: &mut toml_edit::DocumentMut) {
    if !doc.contains_key("wsl2") {
        let mut t = toml_edit::Table::new();
        t.set_implicit(false);
        doc.insert("wsl2", toml_edit::Item::Table(t));
    } else if let Some(t) = doc["wsl2"].as_table_mut() {
        t.set_implicit(false);
    }
}

fn apply_vhd_size(wsl2: &mut toml_edit::Table, min_gb: u64) {
    let current = wsl2.get("vhdSize").and_then(toml_edit::Item::as_str);

    match current {
        Some(raw) => match parse_size_to_gb(raw) {
            Some(gib) if gib >= min_gb => {
                // User already has a cap at or above the floor — leave it alone.
            }
            Some(_) => {
                wsl2["vhdSize"] = toml_edit::value(format!("{min_gb}GB"));
            }
            None => {
                tracing::warn!(
                    raw = raw,
                    "unrecognized vhdSize value in .wslconfig; leaving it unchanged"
                );
            }
        },
        None => {
            wsl2["vhdSize"] = toml_edit::value(format!("{min_gb}GB"));
        }
    }
}

fn apply_sparse_vhd(wsl2: &mut toml_edit::Table) {
    let current = wsl2.get("sparseVhd").and_then(toml_edit::Item::as_bool);
    if current != Some(true) {
        wsl2["sparseVhd"] = toml_edit::value(true);
    }
}

/// Parse a `vhdSize`-style string (e.g. `"1TB"`, `"500GB"`, `"80GiB"`) into GiB.
/// Returns `None` when the value is a bare integer, empty, or uses an unrecognized suffix.
fn parse_size_to_gb(raw: &str) -> Option<u64> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }

    let split_at = s.find(|c: char| !c.is_ascii_digit())?;
    if split_at == 0 {
        return None;
    }
    let (num_part, suffix_part) = s.split_at(split_at);
    let num: u64 = num_part.parse().ok()?;
    let suffix = suffix_part.trim().to_ascii_lowercase();

    // Treat binary and decimal suffixes the same: WSL accepts KB/MB/GB/TB and our
    // consumers don't care about the 2.4% discrepancy here.
    let multiplier_in_gib: u64 = match suffix.as_str() {
        "kb" | "kib" => {
            // Less than 1 GiB — round down to 0; caller will treat 0 as "below floor"
            // and raise it.
            return Some(num / (1024 * 1024));
        }
        "mb" | "mib" => {
            return Some(num / 1024);
        }
        "gb" | "gib" => 1,
        "tb" | "tib" => 1024,
        _ => return None,
    };

    Some(num.saturating_mul(multiplier_in_gib))
}

/// Parse `[wsl2] vhdSize` out of a raw document. Returns `None` when the key is
/// missing or unparseable. Used to populate `WslconfigOutcome::previous_cap_gb`.
fn parse_vhd_size_gb(body: &str) -> Option<u64> {
    let doc = body.parse::<toml_edit::DocumentMut>().ok()?;
    let raw = doc.get("wsl2")?.as_table()?.get("vhdSize")?.as_str()?;
    parse_size_to_gb(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_produces_full_section() {
        let (out, changed) = merge_doc("", 200);
        assert!(changed);
        assert!(out.contains("[wsl2]"));
        assert!(out.contains(r#"vhdSize = "200GB""#));
        assert!(out.contains("sparseVhd = true"));
    }

    #[test]
    fn preserves_existing_unrelated_keys_in_wsl2() {
        let input = "[wsl2]\nmemory = \"4GB\"\n";
        let (out, changed) = merge_doc(input, 200);
        assert!(changed);
        assert!(out.contains(r#"memory = "4GB""#));
        assert!(out.contains(r#"vhdSize = "200GB""#));
        assert!(out.contains("sparseVhd = true"));
    }

    #[test]
    fn leaves_larger_existing_cap_untouched() {
        let input = "[wsl2]\nvhdSize = \"1TB\"\nmemory = \"4GB\"\n";
        let (out, changed) = merge_doc(input, 200);
        // vhdSize stays at 1TB; sparseVhd gets added.
        assert!(out.contains(r#"vhdSize = "1TB""#));
        assert!(out.contains(r#"memory = "4GB""#));
        // We still need to add sparseVhd, so this should have changed.
        assert!(changed);
        assert!(out.contains("sparseVhd = true"));
    }

    #[test]
    fn leaves_larger_existing_cap_untouched_when_sparse_already_true() {
        let input = "[wsl2]\nvhdSize = \"1TB\"\nsparseVhd = true\nmemory = \"4GB\"\n";
        let (out, changed) = merge_doc(input, 200);
        assert!(
            !changed,
            "no-op expected when both target keys already satisfy policy"
        );
        assert_eq!(out, input);
    }

    #[test]
    fn raises_cap_that_is_below_floor() {
        let input = "[wsl2]\nvhdSize = \"100GB\"\n";
        let (out, changed) = merge_doc(input, 200);
        assert!(changed);
        assert!(out.contains(r#"vhdSize = "200GB""#));
        assert!(out.contains("sparseVhd = true"));
    }

    #[test]
    fn preserves_other_sections_and_creates_wsl2_changes_in_place() {
        let input = "[other]\nkey = 1\n\n[wsl2]\nmemory = \"4GB\"\n";
        let (out, changed) = merge_doc(input, 200);
        assert!(changed);
        assert!(out.contains("[other]"));
        assert!(out.contains("key = 1"));
        assert!(out.contains("[wsl2]"));
        assert!(out.contains(r#"memory = "4GB""#));
        assert!(out.contains(r#"vhdSize = "200GB""#));
        assert!(out.contains("sparseVhd = true"));
    }

    #[test]
    fn flips_sparse_vhd_false_to_true() {
        let input = "[wsl2]\nsparseVhd = false\n";
        let (out, changed) = merge_doc(input, 200);
        assert!(changed);
        assert!(out.contains("sparseVhd = true"));
        assert!(!out.contains("sparseVhd = false"));
        assert!(out.contains(r#"vhdSize = "200GB""#));
    }

    #[test]
    fn parse_size_handles_common_suffixes() {
        assert_eq!(parse_size_to_gb("1TB"), Some(1024));
        assert_eq!(parse_size_to_gb("500GB"), Some(500));
        assert_eq!(parse_size_to_gb("80GiB"), Some(80));
        assert_eq!(parse_size_to_gb("80gib"), Some(80));
        assert_eq!(parse_size_to_gb("2TiB"), Some(2048));
    }

    #[test]
    fn parse_size_rejects_bare_integers_and_unknown_suffixes() {
        assert_eq!(parse_size_to_gb("200"), None);
        assert_eq!(parse_size_to_gb(""), None);
        assert_eq!(parse_size_to_gb("GB"), None);
        assert_eq!(parse_size_to_gb("100ZB"), None);
    }

    #[test]
    fn unrecognized_existing_value_is_left_alone() {
        let input = "[wsl2]\nvhdSize = \"200\"\n";
        let (out, _changed) = merge_doc(input, 500);
        // Bare integer is not understood; we must not overwrite it.
        assert!(out.contains(r#"vhdSize = "200""#));
        // But sparseVhd should still be added.
        assert!(out.contains("sparseVhd = true"));
    }

    #[test]
    fn parse_vhd_size_gb_reads_from_document() {
        let body = "[wsl2]\nvhdSize = \"500GB\"\n";
        assert_eq!(parse_vhd_size_gb(body), Some(500));
        assert_eq!(parse_vhd_size_gb(""), None);
        assert_eq!(parse_vhd_size_gb("[wsl2]\nmemory = \"4GB\"\n"), None);
    }
}
