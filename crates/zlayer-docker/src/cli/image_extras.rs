//! Long-tail Docker image / container subcommands: history, prune, search,
//! save, load, import, export, commit.
//!
//! Each handler delegates to the running zlayer daemon via
//! [`DaemonClient`]; output is reshaped to match Docker CLI conventions.

use anyhow::Context;
use bytes::Bytes;
use bytes::BytesMut;
use clap::Parser;
use futures_util::StreamExt;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use zlayer_client::{CommitContainerRequest, DaemonClient};

// ---------------------------------------------------------------------------
// docker history
// ---------------------------------------------------------------------------

/// Arguments for `docker history`.
#[derive(Debug, Parser)]
pub struct HistoryArgs {
    /// Image name to inspect.
    pub image: String,

    /// Print sizes in bytes (no human-friendly formatting).
    #[clap(long = "human", default_value_t = true)]
    pub human: bool,

    /// Don't truncate output.
    #[clap(long = "no-trunc")]
    pub no_trunc: bool,

    /// Only show image IDs.
    #[clap(short, long)]
    pub quiet: bool,

    /// Format the output using the given Go template.
    #[clap(long)]
    pub format: Option<String>,
}

/// Handle the `docker history` command.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable or the history call fails.
pub async fn handle_history(args: HistoryArgs) -> anyhow::Result<()> {
    let client = DaemonClient::connect()
        .await
        .context("Failed to connect to the zlayer daemon")?;
    let history = client
        .image_history(&args.image)
        .await
        .with_context(|| format!("Failed to fetch history for '{}'", args.image))?;

    if args.quiet {
        for entry in &history {
            let id = if args.no_trunc {
                entry.id.clone()
            } else {
                entry
                    .id
                    .strip_prefix("sha256:")
                    .unwrap_or(&entry.id)
                    .chars()
                    .take(12)
                    .collect::<String>()
            };
            println!("{id}");
        }
        return Ok(());
    }

    if args.format.is_none() {
        println!(
            "{:<20} {:<15} {:<60} {:>12} {:<20}",
            "IMAGE", "CREATED", "CREATED BY", "SIZE", "COMMENT"
        );
    }
    for entry in &history {
        let id = if args.no_trunc {
            entry.id.clone()
        } else {
            entry
                .id
                .strip_prefix("sha256:")
                .unwrap_or(&entry.id)
                .chars()
                .take(12)
                .collect()
        };
        let created_by = if args.no_trunc {
            entry.created_by.clone()
        } else {
            entry.created_by.chars().take(58).collect()
        };
        let size = if args.human {
            human_bytes(entry.size)
        } else {
            format!("{}B", entry.size)
        };
        if let Some(template) = args.format.as_deref() {
            let line = template
                .replace("{{.ID}}", &id)
                .replace("{{.CreatedBy}}", &entry.created_by)
                .replace("{{.Size}}", &size)
                .replace("{{.Comment}}", &entry.comment);
            println!("{line}");
        } else {
            println!(
                "{id:<20} {:<15} {created_by:<60} {size:>12} {:<20}",
                entry.created, entry.comment,
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// docker search
// ---------------------------------------------------------------------------

/// Arguments for `docker search`.
#[derive(Debug, Parser)]
pub struct SearchArgs {
    /// Search term.
    pub term: String,

    /// Maximum number of search results.
    #[clap(long, default_value = "25")]
    pub limit: u32,

    /// Filter output based on conditions provided (repeatable).
    #[clap(long = "filter")]
    pub filter: Vec<String>,

    /// Format the output using the given Go template.
    #[clap(long)]
    pub format: Option<String>,

    /// Don't truncate output.
    #[clap(long = "no-trunc")]
    pub no_trunc: bool,
}

/// Handle the `docker search` command.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable or the search call fails.
pub async fn handle_search(args: SearchArgs) -> anyhow::Result<()> {
    let client = DaemonClient::connect()
        .await
        .context("Failed to connect to the zlayer daemon")?;
    let results = client
        .search_images(&args.term, args.limit)
        .await
        .with_context(|| format!("Failed to search for '{}'", args.term))?;

    if let Some(template) = args.format.as_deref() {
        for r in &results {
            let line = template
                .replace("{{.Name}}", &r.name)
                .replace("{{.Description}}", &r.description)
                .replace("{{.StarCount}}", &r.star_count.to_string())
                .replace("{{.IsOfficial}}", &r.official.to_string())
                .replace("{{.IsAutomated}}", &r.automated.to_string());
            println!("{line}");
        }
        return Ok(());
    }

    println!(
        "{:<40} {:<60} {:<8} {:<10} {:<10}",
        "NAME", "DESCRIPTION", "STARS", "OFFICIAL", "AUTOMATED"
    );
    for r in &results {
        let desc = if args.no_trunc {
            r.description.clone()
        } else {
            r.description.chars().take(58).collect::<String>()
        };
        let official = if r.official { "[OK]" } else { "" };
        let automated = if r.automated { "[OK]" } else { "" };
        println!(
            "{:<40} {desc:<60} {:<8} {official:<10} {automated:<10}",
            r.name, r.star_count
        );
    }
    let _ = args.filter; // accepted for compat — registry-side filtering only
    Ok(())
}

// ---------------------------------------------------------------------------
// docker save
// ---------------------------------------------------------------------------

/// Arguments for `docker save`.
#[derive(Debug, Parser)]
pub struct SaveArgs {
    /// Images to save (one or more).
    #[clap(required = true)]
    pub images: Vec<String>,

    /// Write to a file instead of stdout.
    #[clap(short = 'o', long)]
    pub output: Option<PathBuf>,
}

/// Handle the `docker save` command.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable, the save stream fails,
/// or writing to the output file fails.
pub async fn handle_save(args: SaveArgs) -> anyhow::Result<()> {
    let client = DaemonClient::connect()
        .await
        .context("Failed to connect to the zlayer daemon")?;
    let mut stream = client
        .save_images(&args.images)
        .await
        .context("Failed to start save stream")?;

    let mut writer = match &args.output {
        Some(path) => {
            let file = tokio::fs::File::create(path)
                .await
                .with_context(|| format!("Failed to create '{}'", path.display()))?;
            WriteSink::File(file)
        }
        None => WriteSink::Stdout(tokio::io::stdout()),
    };

    while let Some(chunk) = stream.next().await {
        let bytes = chunk.context("save stream error")?;
        writer
            .write_all(&bytes)
            .await
            .context("Failed to write tar bytes")?;
    }
    writer.flush().await.context("Failed to flush output")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// docker load
// ---------------------------------------------------------------------------

/// Arguments for `docker load`.
#[derive(Debug, Parser)]
pub struct LoadArgs {
    /// Read from a file instead of stdin.
    #[clap(short = 'i', long)]
    pub input: Option<PathBuf>,

    /// Suppress per-line progress output.
    #[clap(short, long)]
    pub quiet: bool,
}

/// Handle the `docker load` command.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable, reading the input fails,
/// or the load call fails.
pub async fn handle_load(args: LoadArgs) -> anyhow::Result<()> {
    use tokio::io::AsyncReadExt;
    let mut bytes = BytesMut::new();
    if let Some(path) = args.input.as_deref() {
        let mut file = tokio::fs::File::open(path)
            .await
            .with_context(|| format!("Failed to open '{}'", path.display()))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)
            .await
            .with_context(|| format!("Failed to read '{}'", path.display()))?;
        bytes.extend_from_slice(&buf);
    } else {
        let mut stdin = tokio::io::stdin();
        let mut buf = Vec::new();
        stdin
            .read_to_end(&mut buf)
            .await
            .context("Failed to read tar archive from stdin")?;
        bytes.extend_from_slice(&buf);
    }

    let client = DaemonClient::connect()
        .await
        .context("Failed to connect to the zlayer daemon")?;
    let resp = client
        .load_images(bytes.freeze(), args.quiet)
        .await
        .context("Failed to load images")?;

    if !args.quiet {
        // Forward NDJSON status lines so the user sees per-image progress.
        if let Ok(text) = std::str::from_utf8(&resp) {
            for line in text.lines() {
                if line.is_empty() {
                    continue;
                }
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
                    if let Some(status) = value.get("status").and_then(|v| v.as_str()) {
                        println!("{status}");
                    } else if let Some(payload) = value.get("Status").and_then(|v| v.as_str()) {
                        println!("{payload}");
                    } else if let Some(refs) = value.get("references").and_then(|v| v.as_array()) {
                        for r in refs {
                            if let Some(s) = r.as_str() {
                                println!("Loaded image: {s}");
                            }
                        }
                    }
                } else {
                    println!("{line}");
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// docker import
// ---------------------------------------------------------------------------

/// Arguments for `docker import`.
#[derive(Debug, Parser)]
pub struct ImportArgs {
    /// Tar archive file (use `-` for stdin).
    pub source: String,

    /// Optional repository:tag to apply to the imported image.
    pub reference: Option<String>,

    /// Apply Dockerfile instructions to the created image.
    #[clap(short = 'c', long = "change")]
    pub changes: Vec<String>,

    /// Set commit message for the imported image.
    #[clap(short = 'm', long)]
    pub message: Option<String>,

    /// Set platform if server is multi-platform capable.
    #[clap(long)]
    pub platform: Option<String>,
}

/// Handle the `docker import` command.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable, reading the source fails,
/// or the import call fails.
pub async fn handle_import(args: ImportArgs) -> anyhow::Result<()> {
    use tokio::io::AsyncReadExt;
    let bytes = if args.source == "-" {
        let mut stdin = tokio::io::stdin();
        let mut buf = Vec::new();
        stdin
            .read_to_end(&mut buf)
            .await
            .context("Failed to read tar archive from stdin")?;
        Bytes::from(buf)
    } else {
        // Best-effort URL handling: bare paths only. URL imports require
        // server-side fetch which is out of scope here.
        if args.source.starts_with("http://") || args.source.starts_with("https://") {
            anyhow::bail!(
                "URL-based docker import is not yet supported; download the archive locally first"
            );
        }
        let path = PathBuf::from(&args.source);
        let mut file = tokio::fs::File::open(&path)
            .await
            .with_context(|| format!("Failed to open '{}'", path.display()))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)
            .await
            .with_context(|| format!("Failed to read '{}'", path.display()))?;
        Bytes::from(buf)
    };

    let (repo, tag) = match args.reference.as_deref() {
        Some(reference) if !reference.is_empty() => {
            let (r, t) = split_reference(reference);
            (Some(r.to_string()), Some(t.to_string()))
        }
        _ => (None, None),
    };

    let client = DaemonClient::connect()
        .await
        .context("Failed to connect to the zlayer daemon")?;
    let resp = client
        .import_image(bytes, repo.as_deref(), tag.as_deref())
        .await
        .context("Failed to import image")?;

    println!("{}", resp.id);
    let _ = args.changes;
    let _ = args.message;
    let _ = args.platform;
    Ok(())
}

// ---------------------------------------------------------------------------
// docker export
// ---------------------------------------------------------------------------

/// Arguments for `docker export`.
#[derive(Debug, Parser)]
pub struct ExportArgs {
    /// Container id or name.
    pub container: String,

    /// Write to a file instead of stdout.
    #[clap(short = 'o', long)]
    pub output: Option<PathBuf>,
}

/// Handle the `docker export` command.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable, the export stream fails,
/// or writing to the output file fails.
pub async fn handle_export(args: ExportArgs) -> anyhow::Result<()> {
    let client = DaemonClient::connect()
        .await
        .context("Failed to connect to the zlayer daemon")?;
    let mut stream = client
        .export_container(&args.container)
        .await
        .with_context(|| format!("Failed to start export of '{}'", args.container))?;

    let mut writer = match &args.output {
        Some(path) => {
            let file = tokio::fs::File::create(path)
                .await
                .with_context(|| format!("Failed to create '{}'", path.display()))?;
            WriteSink::File(file)
        }
        None => WriteSink::Stdout(tokio::io::stdout()),
    };

    while let Some(chunk) = stream.next().await {
        let bytes = chunk.context("export stream error")?;
        writer
            .write_all(&bytes)
            .await
            .context("Failed to write tar bytes")?;
    }
    writer.flush().await.context("Failed to flush output")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// docker commit
// ---------------------------------------------------------------------------

/// Arguments for `docker commit`.
#[derive(Debug, Parser)]
pub struct CommitArgs {
    /// Container id or name.
    pub container: String,

    /// Optional repository:tag for the new image.
    pub reference: Option<String>,

    /// Apply Dockerfile instructions to the created image.
    #[clap(short = 'c', long = "change")]
    pub changes: Vec<String>,

    /// Commit message.
    #[clap(short = 'm', long)]
    pub message: Option<String>,

    /// Author of the image.
    #[clap(short = 'a', long)]
    pub author: Option<String>,

    /// Pause the container during commit (default true).
    #[clap(short = 'p', long, default_value_t = true)]
    pub pause: bool,
}

/// Handle the `docker commit` command.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable or the commit call fails.
pub async fn handle_commit(args: CommitArgs) -> anyhow::Result<()> {
    let (repo, tag) = match args.reference.as_deref() {
        Some(reference) if !reference.is_empty() => {
            let (r, t) = split_reference(reference);
            (Some(r.to_string()), Some(t.to_string()))
        }
        _ => (None, None),
    };

    let changes = if args.changes.is_empty() {
        None
    } else {
        Some(args.changes.join("\n"))
    };

    let req = CommitContainerRequest {
        container: args.container.clone(),
        repo,
        tag,
        comment: args.message,
        author: args.author,
        pause: args.pause,
        changes,
    };

    let client = DaemonClient::connect()
        .await
        .context("Failed to connect to the zlayer daemon")?;
    let resp = client
        .commit_container_image(&req)
        .await
        .with_context(|| format!("Failed to commit container '{}'", args.container))?;

    println!("{}", resp.id);
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Split `repo:tag` (or `repo@digest`) into `(repo, tag)`.
fn split_reference(reference: &str) -> (&str, &str) {
    if let Some((repo, tag)) = reference.rsplit_once(':') {
        if !tag.contains('/') {
            return (repo, tag);
        }
    }
    (reference, "latest")
}

/// Format a byte count as a human-friendly string.
#[allow(clippy::cast_precision_loss)]
fn human_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB {
        format!("{:.1}GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1}MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1}KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes}B")
    }
}

/// Enum-based output sink that lets us write to either a file or stdout
/// without dyn-trait object safety issues from `async fn` methods.
enum WriteSink {
    File(tokio::fs::File),
    Stdout(tokio::io::Stdout),
}

impl WriteSink {
    async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        match self {
            Self::File(f) => f.write_all(buf).await,
            Self::Stdout(s) => s.write_all(buf).await,
        }
    }

    async fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::File(f) => f.flush().await,
            Self::Stdout(s) => s.flush().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_reference_with_tag() {
        assert_eq!(split_reference("nginx:1.21"), ("nginx", "1.21"));
    }

    #[test]
    fn split_reference_without_tag_defaults_latest() {
        assert_eq!(split_reference("nginx"), ("nginx", "latest"));
    }

    #[test]
    fn split_reference_with_registry_port() {
        assert_eq!(
            split_reference("registry.io:5000/foo"),
            ("registry.io:5000/foo", "latest")
        );
    }

    #[test]
    fn human_bytes_units() {
        assert_eq!(human_bytes(512), "512B");
        assert_eq!(human_bytes(2048), "2.0KB");
        assert_eq!(human_bytes(3 * 1024 * 1024), "3.0MB");
        assert_eq!(human_bytes(5 * 1024 * 1024 * 1024), "5.0GB");
    }
}
