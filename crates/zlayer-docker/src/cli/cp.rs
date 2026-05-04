//! Docker `cp` command.
//!
//! Implements the Docker CLI's `docker cp SRC DEST` semantics by talking to
//! the local zlayer daemon's container-archive endpoints
//! (`GET|PUT|HEAD /containers/{id}/archive`).
//!
//! `SRC` and `DEST` are each one of:
//!
//! * `<container>:<path>` — points inside a container.
//! * `<local path>` — points to a file or directory on the host.
//!
//! Exactly one side must be a container reference. Three modes are
//! supported:
//!
//! 1. **Container → host** (`docker cp web:/etc/nginx/nginx.conf ./conf`) —
//!    download the TAR archive of the container path, then unpack it to the
//!    host destination. If the destination is a directory the archive is
//!    extracted as-is; if the destination is a non-existent path that
//!    matches the archive's single entry, the entry is renamed to that
//!    path (Docker's "copy a file to a target name" shortcut).
//! 2. **Host → container** (`docker cp ./conf web:/etc/nginx/nginx.conf`) —
//!    package the host source into a TAR archive (with the destination's
//!    basename as the archive entry name when the destination is a file
//!    name) and `PUT` it into the container.
//! 3. **Container → container** — chain modes 1 and 2 by buffering the TAR
//!    archive in memory. Docker's CLI implements this client-side via a
//!    local pipe; we mirror that behaviour exactly.

use std::io::Cursor;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context};
use bytes::Bytes;
use clap::Parser;
use zlayer_client::{default_socket_path, DaemonClient};

/// Arguments for `docker cp`.
///
/// Mirrors Docker's `docker cp SRC DEST` invocation. Each side is
/// `[CONTAINER:]PATH`; exactly one side must carry a container reference.
#[derive(Debug, Parser)]
pub struct CpArgs {
    /// Source path. `[CONTAINER:]PATH` — the container reference is
    /// optional; when absent, this side refers to a local host path.
    pub source: String,

    /// Destination path. `[CONTAINER:]PATH` — same shape as `source`.
    pub destination: String,
}

/// Parsed shape of a single side of `docker cp`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Endpoint {
    /// A path inside a named container.
    Container { container: String, path: String },
    /// A path on the local host filesystem.
    Local { path: PathBuf },
}

/// Split a `docker cp` argument into [`Endpoint::Container`] vs
/// [`Endpoint::Local`].
///
/// Docker's heuristic: a colon-prefixed string identifies a container only
/// when the part before the first colon contains no path separator and is a
/// non-empty identifier. Windows-style absolute paths like `C:\Users\foo` are
/// detected by the second character being a path separator and the first
/// being a single ASCII letter; those stay as local paths.
fn parse_endpoint(arg: &str) -> Endpoint {
    if let Some((maybe_container, rest)) = arg.split_once(':') {
        let looks_like_container = !(maybe_container.is_empty()
            || maybe_container.contains('/')
            || maybe_container.contains('\\')
            // Reject `C:` style Windows drives.
            || (maybe_container.len() == 1
                && maybe_container.chars().all(|c| c.is_ascii_alphabetic())));
        if looks_like_container {
            return Endpoint::Container {
                container: maybe_container.to_string(),
                path: rest.to_string(),
            };
        }
    }
    Endpoint::Local {
        path: PathBuf::from(arg),
    }
}

/// Handle the `docker cp` command.
///
/// # Errors
///
/// Returns an error when:
/// - both arguments are local paths (no container side),
/// - the daemon connection fails,
/// - the source path doesn't exist on the host (host → container),
/// - the runtime backing the container can't materialize the archive.
pub async fn handle_cp(args: CpArgs) -> anyhow::Result<()> {
    tracing::info!("docker cp: {} -> {}", args.source, args.destination);

    let src = parse_endpoint(&args.source);
    let dest = parse_endpoint(&args.destination);

    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    match (&src, &dest) {
        (Endpoint::Local { .. }, Endpoint::Local { .. }) => {
            bail!("`docker cp` requires at least one of SRC or DEST to be a `<container>:<path>` reference");
        }
        (Endpoint::Container { container, path }, Endpoint::Local { path: local }) => {
            copy_from_container(&client, container, path, local).await
        }
        (Endpoint::Local { path: local }, Endpoint::Container { container, path }) => {
            copy_to_container(&client, local, container, path).await
        }
        (
            Endpoint::Container {
                container: src_c,
                path: src_p,
            },
            Endpoint::Container {
                container: dst_c,
                path: dst_p,
            },
        ) => copy_container_to_container(&client, src_c, src_p, dst_c, dst_p).await,
    }
}

/// Container → host implementation.
///
/// Downloads the TAR archive of `container_path` and unpacks it into
/// `local_dest`. When `local_dest` does not exist and the archive contains
/// exactly one top-level entry that's a file, the entry is renamed to
/// `local_dest` (matching Docker's "copy file to target name" shortcut).
async fn copy_from_container(
    client: &DaemonClient,
    container: &str,
    container_path: &str,
    local_dest: &Path,
) -> anyhow::Result<()> {
    let (tar_bytes, _stat) = client
        .archive_get(container, container_path)
        .await
        .with_context(|| {
            format!("Failed to download archive of '{container}:{container_path}' from daemon")
        })?;

    // Determine the unpack root. If `local_dest` is an existing directory
    // we extract directly into it; otherwise we treat `local_dest` as a
    // target name for the archive's single top-level entry.
    let dest_is_dir = std::fs::metadata(local_dest).is_ok_and(|m| m.is_dir());
    if dest_is_dir {
        unpack_into_directory(&tar_bytes, local_dest)?;
    } else {
        // Inspect the archive: if it has exactly one top-level entry that's
        // a regular file, rename it to `local_dest`. If it has multiple
        // entries (e.g. a directory copy), require `local_dest` to be a
        // directory we can create.
        let entries = tar_top_level_entries(&tar_bytes)?;
        if entries.len() == 1 && !entries[0].is_dir {
            // Rewrite the single entry's name to match `local_dest`.
            unpack_single_file(&tar_bytes, &entries[0].name, local_dest)?;
        } else {
            // Treat the destination as a directory we should create.
            std::fs::create_dir_all(local_dest).with_context(|| {
                format!(
                    "Failed to create destination directory '{}'",
                    local_dest.display()
                )
            })?;
            unpack_into_directory(&tar_bytes, local_dest)?;
        }
    }
    Ok(())
}

/// Host → container implementation.
///
/// Packs the host `local_src` into a TAR archive and `PUT`s it into the
/// container's parent directory. Docker's semantics: when `container_dest`
/// is an existing directory, the archive is extracted as-is; when
/// `container_dest` is a non-existent path, the archive is built with the
/// destination's basename as the entry name and extracted into the
/// destination's parent directory.
async fn copy_to_container(
    client: &DaemonClient,
    local_src: &Path,
    container: &str,
    container_dest: &str,
) -> anyhow::Result<()> {
    if !local_src.exists() {
        bail!(
            "Source path '{}' does not exist on host",
            local_src.display()
        );
    }

    // Probe the destination path inside the container. If it stat's as a
    // directory, we extract the archive into it directly using the
    // basename of `local_src` as the entry name. Otherwise we extract into
    // the parent directory and rename the single entry to the destination's
    // basename.
    let head_result = client.archive_head(container, container_dest).await;
    let dest_exists_as_dir = match head_result {
        Ok(Some(header)) => decoded_stat_is_dir(&header).unwrap_or(false),
        Ok(None) | Err(_) => false,
    };

    let (extract_dir, entry_name) = if dest_exists_as_dir {
        let entry = local_src
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(".")
            .to_string();
        (container_dest.to_string(), entry)
    } else {
        let parent = parent_path(container_dest);
        let entry = basename(container_dest);
        (parent, entry)
    };

    let tar_bytes = build_tar_from_local(local_src, &entry_name)
        .context("Failed to build TAR archive from host source")?;

    client
        .archive_put(container, &extract_dir, tar_bytes, false, false)
        .await
        .with_context(|| {
            format!("Failed to upload archive to '{container}:{extract_dir}' via daemon")
        })?;
    Ok(())
}

/// Container → container implementation.
///
/// Buffers the source archive in memory and forwards it as the destination
/// archive's body, mirroring Docker's local-pipe approach.
async fn copy_container_to_container(
    client: &DaemonClient,
    src_container: &str,
    src_path: &str,
    dst_container: &str,
    dst_path: &str,
) -> anyhow::Result<()> {
    let (tar_bytes, _stat) = client
        .archive_get(src_container, src_path)
        .await
        .with_context(|| format!("Failed to download archive of '{src_container}:{src_path}'"))?;
    // Decide on extract dir similar to host → container: when the
    // destination stat'd as a directory, extract directly; otherwise extract
    // into the parent.
    let head_result = client.archive_head(dst_container, dst_path).await;
    let dest_exists_as_dir = match head_result {
        Ok(Some(header)) => decoded_stat_is_dir(&header).unwrap_or(false),
        Ok(None) | Err(_) => false,
    };
    let extract_dir = if dest_exists_as_dir {
        dst_path.to_string()
    } else {
        parent_path(dst_path)
    };
    client
        .archive_put(dst_container, &extract_dir, tar_bytes, false, false)
        .await
        .with_context(|| format!("Failed to upload archive to '{dst_container}:{extract_dir}'"))?;
    Ok(())
}

/// Build a TAR archive from a host file or directory. The archive contains a
/// single top-level entry named `entry_name`.
fn build_tar_from_local(local_src: &Path, entry_name: &str) -> anyhow::Result<Bytes> {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut buf);
        builder.follow_symlinks(false);
        let meta = std::fs::symlink_metadata(local_src)
            .with_context(|| format!("Failed to stat '{}'", local_src.display()))?;
        if meta.is_dir() {
            builder
                .append_dir_all(entry_name, local_src)
                .context("Failed to append directory to TAR")?;
        } else {
            let mut f = std::fs::File::open(local_src)
                .with_context(|| format!("Failed to open '{}'", local_src.display()))?;
            builder
                .append_file(entry_name, &mut f)
                .context("Failed to append file to TAR")?;
        }
        builder.finish().context("Failed to finalize TAR archive")?;
    }
    Ok(Bytes::from(buf))
}

/// Lightweight description of one top-level archive entry, used by the
/// container → host path to decide whether the destination should be
/// renamed.
struct TopLevelEntry {
    name: String,
    is_dir: bool,
}

/// Enumerate the top-level entries of a TAR archive without unpacking it.
fn tar_top_level_entries(tar_bytes: &[u8]) -> anyhow::Result<Vec<TopLevelEntry>> {
    let mut archive = tar::Archive::new(Cursor::new(tar_bytes));
    let mut out = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for entry in archive.entries().context("Failed to read TAR entries")? {
        let entry = entry.context("Invalid TAR entry")?;
        let path = entry.path().context("Invalid TAR entry path")?;
        let mut comps = path.components();
        let Some(Component::Normal(first)) = comps.next() else {
            continue;
        };
        let Some(name) = first.to_str() else {
            continue;
        };
        if !seen.insert(name.to_string()) {
            continue;
        }
        let is_dir = entry.header().entry_type().is_dir() || comps.next().is_some();
        out.push(TopLevelEntry {
            name: name.to_string(),
            is_dir,
        });
    }
    Ok(out)
}

/// Unpack an archive verbatim into `dest` (which must already be a directory).
fn unpack_into_directory(tar_bytes: &[u8], dest: &Path) -> anyhow::Result<()> {
    let mut archive = tar::Archive::new(Cursor::new(tar_bytes));
    archive.set_preserve_permissions(true);
    archive
        .unpack(dest)
        .with_context(|| format!("Failed to unpack TAR into '{}'", dest.display()))?;
    Ok(())
}

/// Unpack a TAR archive whose single top-level entry is named `from`,
/// rewriting that entry to land at `to` on the host.
fn unpack_single_file(tar_bytes: &[u8], from: &str, to: &Path) -> anyhow::Result<()> {
    let mut archive = tar::Archive::new(Cursor::new(tar_bytes));
    for entry in archive.entries().context("Failed to read TAR entries")? {
        let mut entry = entry.context("Invalid TAR entry")?;
        let path = entry.path().context("Invalid TAR entry path")?.into_owned();
        let mut comps = path.components();
        let Some(Component::Normal(first)) = comps.next() else {
            continue;
        };
        if first.to_str() != Some(from) {
            continue;
        }
        // Single-file archive: write to `to` directly.
        entry
            .unpack(to)
            .with_context(|| format!("Failed to unpack entry to '{}'", to.display()))?;
        return Ok(());
    }
    bail!("Archive entry '{from}' not found")
}

/// Decode a base64-encoded `X-Docker-Container-Path-Stat` header and check
/// whether the path is a directory (Unix mode bit `S_IFDIR`).
fn decoded_stat_is_dir(header: &str) -> Option<bool> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(header)
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let mode = v.get("mode").and_then(serde_json::Value::as_u64)?;
    // Unix `S_IFMT = 0o170000`, `S_IFDIR = 0o040000`. The wire mode also
    // encodes Go's os.ModeDir bit (1 << 31), so check both.
    let is_dir_unix = (mode & 0o170_000) == 0o040_000;
    let is_dir_go = (mode & (1 << 31)) != 0;
    Some(is_dir_unix || is_dir_go)
}

/// Compute the parent directory of a Unix-style container path.
fn parent_path(p: &str) -> String {
    if let Some(idx) = p.rfind('/') {
        if idx == 0 {
            "/".to_string()
        } else {
            p[..idx].to_string()
        }
    } else {
        ".".to_string()
    }
}

/// Compute the basename of a Unix-style container path.
fn basename(p: &str) -> String {
    p.rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn parses_two_positional_arguments() {
        let args = CpArgs::try_parse_from(["cp", "web:/etc/hosts", "./hosts"]).unwrap();
        assert_eq!(args.source, "web:/etc/hosts");
        assert_eq!(args.destination, "./hosts");
    }

    #[test]
    fn rejects_missing_arguments() {
        assert!(CpArgs::try_parse_from(["cp"]).is_err());
        assert!(CpArgs::try_parse_from(["cp", "only-one"]).is_err());
    }

    #[test]
    fn cli_metadata_lists_both_args() {
        let cmd = CpArgs::command();
        let names: Vec<&str> = cmd
            .get_arguments()
            .map(clap::Arg::get_id)
            .map(clap::Id::as_str)
            .collect();
        assert!(names.contains(&"source"));
        assert!(names.contains(&"destination"));
    }

    #[test]
    fn parse_endpoint_distinguishes_container_and_local() {
        match parse_endpoint("web:/etc/hosts") {
            Endpoint::Container { container, path } => {
                assert_eq!(container, "web");
                assert_eq!(path, "/etc/hosts");
            }
            Endpoint::Local { .. } => panic!("expected Container for 'web:/etc/hosts'"),
        }
        match parse_endpoint("./hosts") {
            Endpoint::Local { path } => assert_eq!(path, PathBuf::from("./hosts")),
            Endpoint::Container { .. } => panic!("expected Local for './hosts'"),
        }
        // Windows-style drive letters stay as local paths.
        match parse_endpoint("C:/Users/foo") {
            Endpoint::Local { path } => assert_eq!(path, PathBuf::from("C:/Users/foo")),
            Endpoint::Container { .. } => panic!("expected Local for C: drive"),
        }
        // Path-with-slash before the colon stays as local.
        match parse_endpoint("./foo:bar") {
            Endpoint::Local { path } => assert_eq!(path, PathBuf::from("./foo:bar")),
            Endpoint::Container { .. } => panic!("expected Local for './foo:bar'"),
        }
    }

    #[test]
    fn parent_and_basename_handle_edge_cases() {
        assert_eq!(parent_path("/etc/hosts"), "/etc");
        assert_eq!(parent_path("/foo"), "/");
        assert_eq!(parent_path("hosts"), ".");
        assert_eq!(basename("/etc/hosts"), "hosts");
        assert_eq!(basename("/etc/"), "etc");
        assert_eq!(basename("hosts"), "hosts");
    }

    #[test]
    fn build_tar_round_trips_a_single_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("hello.txt");
        std::fs::write(&src, b"hi\n").unwrap();
        let bytes = build_tar_from_local(&src, "hello.txt").unwrap();
        // Inspect: the archive should contain exactly one entry named "hello.txt".
        let mut archive = tar::Archive::new(Cursor::new(&bytes));
        let mut names = Vec::new();
        for e in archive.entries().unwrap() {
            let e = e.unwrap();
            names.push(e.path().unwrap().to_string_lossy().into_owned());
        }
        assert_eq!(names, vec!["hello.txt".to_string()]);
    }

    #[test]
    fn top_level_entries_dedupe_directory_children() {
        // Build an archive with a directory and a file inside it.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a")).unwrap();
        std::fs::write(dir.path().join("a/b.txt"), b"x").unwrap();
        let bytes = build_tar_from_local(dir.path(), "root").unwrap();
        let entries = tar_top_level_entries(&bytes).unwrap();
        assert!(entries.iter().any(|e| e.name == "root" && e.is_dir));
    }

    #[test]
    fn decoded_stat_is_dir_recognises_unix_dir_mode() {
        use base64::Engine;
        let json = serde_json::json!({
            "name": "etc",
            "size": 4096,
            "mode": 0o040_755_u32,
            "mtime": null,
            "linkTarget": "",
        })
        .to_string();
        let header = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());
        assert_eq!(decoded_stat_is_dir(&header), Some(true));
        let json_file = serde_json::json!({
            "name": "f",
            "size": 3,
            "mode": 0o100_644_u32,
            "mtime": null,
            "linkTarget": "",
        })
        .to_string();
        let header_file = base64::engine::general_purpose::STANDARD.encode(json_file.as_bytes());
        assert_eq!(decoded_stat_is_dir(&header_file), Some(false));
    }
}
