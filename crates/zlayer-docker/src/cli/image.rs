//! Docker image management commands — build, pull, push, tag, list, remove, login/logout.
//!
//! Each handler delegates to the running `zlayer` daemon via
//! [`zlayer_client::DaemonClient`] over the local Unix socket. Output is
//! formatted to mirror `docker` CLI conventions so existing tooling/scripts
//! that parse stdout keep working.

use anyhow::Context;
use clap::Parser;
use std::collections::HashMap;
use zlayer_client::{BuildSpec, DaemonClient};

/// Arguments for `docker build`.
#[derive(Debug, Parser)]
#[allow(clippy::struct_excessive_bools)]
pub struct BuildArgs {
    /// Build context directory
    #[clap(default_value = ".")]
    pub context: String,

    /// Name and optionally a tag in the `name:tag` format
    #[clap(short, long = "tag")]
    pub tag: Vec<String>,

    /// Name of the Dockerfile (default: `PATH/Dockerfile`)
    #[clap(short, long = "file")]
    pub file: Option<String>,

    /// Set build-time variables
    #[clap(long = "build-arg")]
    pub build_arg: Vec<String>,

    /// Set the target build stage to build
    #[clap(long)]
    pub target: Option<String>,

    /// Do not use cache when building the image
    #[clap(long)]
    pub no_cache: bool,

    /// Set platform if server is multi-platform capable
    #[clap(long)]
    pub platform: Option<String>,

    /// Push the image after building
    #[clap(long)]
    pub push: bool,

    /// Always attempt to pull a newer version of the image
    #[clap(long)]
    pub pull: bool,

    /// Suppress the build output and print image ID on success
    #[clap(short, long)]
    pub quiet: bool,
}

/// Arguments for `docker pull`.
#[derive(Debug, Parser)]
pub struct PullArgs {
    /// Image name to pull
    pub image: String,

    /// Set platform if server is multi-platform capable
    #[clap(long)]
    pub platform: Option<String>,

    /// Download all tagged images in the repository
    #[clap(short = 'a', long = "all-tags")]
    pub all_tags: bool,

    /// Suppress verbose output
    #[clap(short, long)]
    pub quiet: bool,
}

/// Arguments for `docker push`.
#[derive(Debug, Parser)]
pub struct PushArgs {
    /// Image name or name:tag to push
    pub image: String,

    /// Push all tagged images in the repository
    #[clap(short = 'a', long = "all-tags")]
    pub all_tags: bool,

    /// Suppress verbose output
    #[clap(short, long)]
    pub quiet: bool,
}

/// Arguments for `docker images`.
#[derive(Debug, Parser)]
#[allow(clippy::struct_excessive_bools)]
pub struct ImagesArgs {
    /// Restrict output to images matching the repository name
    pub repository: Option<String>,

    /// Show all images (default hides intermediate images)
    #[clap(short, long)]
    pub all: bool,

    /// Only show image IDs
    #[clap(short, long)]
    pub quiet: bool,

    /// Filter output based on conditions provided
    #[clap(long = "filter")]
    pub filter: Vec<String>,

    /// Format the output using the given Go template
    #[clap(long)]
    pub format: Option<String>,

    /// Show digests
    #[clap(long)]
    pub digests: bool,

    /// Don't truncate output
    #[clap(long)]
    pub no_trunc: bool,
}

/// Arguments for `docker rmi`.
#[derive(Debug, Parser)]
pub struct RmiArgs {
    /// Images to remove (name or ID)
    #[clap(required = true)]
    pub images: Vec<String>,

    /// Force removal of the image
    #[clap(short, long)]
    pub force: bool,

    /// Do not delete untagged parents
    #[clap(long)]
    pub no_prune: bool,
}

/// Arguments for `docker tag`.
#[derive(Debug, Parser)]
pub struct TagArgs {
    /// Source image name or ID
    pub source: String,

    /// Target image name with optional tag
    pub target: String,
}

/// Arguments for `docker login`.
#[derive(Debug, Parser)]
pub struct LoginArgs {
    /// Registry server (default: Docker Hub)
    pub server: Option<String>,

    /// Username
    #[clap(short = 'u', long)]
    pub username: Option<String>,

    /// Password
    #[clap(short = 'p', long)]
    pub password: Option<String>,

    /// Take the password from stdin
    #[clap(long)]
    pub password_stdin: bool,
}

/// Arguments for `docker logout`.
#[derive(Debug, Parser)]
pub struct LogoutArgs {
    /// Registry server (default: Docker Hub)
    pub server: Option<String>,
}

// ---------------------------------------------------------------------------
// Handlers — all bridge to the zlayer daemon via DaemonClient.
// ---------------------------------------------------------------------------

/// Handle the `docker build` command.
///
/// Maps Docker-style arguments onto a [`BuildSpec`] and submits the build
/// request to the daemon's `POST /api/v1/build/json` endpoint. Prints the
/// resulting `build_id` on success; callers can poll
/// `GET /api/v1/build/{id}` or stream from `GET /api/v1/build/{id}/stream`
/// for live progress.
///
/// Docker → `BuildSpec` mapping:
/// - `PATH` (positional, default `.`) → `context_path`
/// - `-t` / `--tag` (repeatable)     → `tags`
/// - `--no-cache`                    → `no_cache`
/// - `--push`                        → `push`
/// - `--target <stage>`              → `target`
/// - `--build-arg KEY=VAL` (repeatable) → `build_args`
/// - `-f` / `--file` is applied by the daemon using the Dockerfile located
///   inside `context_path`; a non-default path is currently best-effort
///   (the server resolves `<context>/Dockerfile` by default).
/// - `--platform`, `--pull`, `--quiet` are accepted but unused in v1.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable, a `--build-arg` is not in
/// `KEY=VALUE` form, or the daemon rejects the build request.
pub async fn handle_build(args: BuildArgs) -> anyhow::Result<()> {
    // Resolve the context path to an absolute path so the daemon (which may
    // be running with a different cwd) can find it.
    let context_path = std::path::Path::new(&args.context);
    let context_path = if context_path.is_absolute() {
        args.context.clone()
    } else {
        std::env::current_dir()
            .context("Failed to resolve current working directory")?
            .join(context_path)
            .to_string_lossy()
            .into_owned()
    };

    // Parse `--build-arg KEY=VALUE` flags into a map.
    let mut build_args: HashMap<String, String> = HashMap::new();
    for raw in &args.build_arg {
        let (key, value) = raw
            .split_once('=')
            .with_context(|| format!("Invalid --build-arg '{raw}': expected KEY=VALUE format"))?;
        build_args.insert(key.to_string(), value.to_string());
    }

    if args.file.is_some() {
        tracing::warn!(
            file = %args.file.as_deref().unwrap_or(""),
            "docker build -f/--file is not yet forwarded to the daemon; \
             falling back to <context>/Dockerfile",
        );
    }
    if args.platform.is_some() {
        tracing::warn!(
            platform = %args.platform.as_deref().unwrap_or(""),
            "docker build --platform is not yet forwarded to the daemon",
        );
    }

    let spec = BuildSpec {
        context_path,
        runtime: None,
        build_args,
        target: args.target.clone(),
        tags: args.tag.clone(),
        no_cache: args.no_cache,
        push: args.push,
    };

    let client = DaemonClient::connect()
        .await
        .context("Failed to connect to the zlayer daemon")?;
    let handle = client
        .start_build(spec)
        .await
        .context("Failed to start build via daemon")?;

    if args.quiet {
        println!("{}", handle.build_id);
    } else {
        println!("Build started: {}", handle.build_id);
        if !handle.message.is_empty() {
            println!("{}", handle.message);
        }
        println!(
            "Follow progress with: zlayer logs --build {}",
            handle.build_id
        );
    }
    Ok(())
}

/// Handle the `docker pull` command.
///
/// Delegates to `DaemonClient::pull_image_from_server`. The daemon handles
/// the actual OCI pull against its cache; this client prints the canonical
/// reference and digest on success.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable or the pull fails.
pub async fn handle_pull(args: PullArgs) -> anyhow::Result<()> {
    if args.all_tags {
        tracing::warn!(
            "docker pull --all-tags is not yet supported; pulling only the requested reference",
        );
    }
    if args.platform.is_some() {
        tracing::warn!(
            platform = %args.platform.as_deref().unwrap_or(""),
            "docker pull --platform is not yet forwarded to the daemon",
        );
    }

    let client = DaemonClient::connect()
        .await
        .context("Failed to connect to the zlayer daemon")?;
    let resp = client
        .pull_image_from_server(&args.image, None)
        .await
        .with_context(|| format!("Failed to pull image '{}'", args.image))?;

    if args.quiet {
        println!("{}", resp.reference);
    } else {
        println!("{}: pulled", resp.reference);
        if let Some(digest) = resp.digest.as_deref() {
            println!("Digest: {digest}");
        }
        println!("Status: Downloaded newer image for {}", resp.reference);
    }
    Ok(())
}

/// Handle the `docker push` command.
///
/// Delegates to `DaemonClient::push_image`. Credentials configured via
/// `zlayer login` (on the daemon side) are used automatically.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable or the push fails.
pub async fn handle_push(args: PushArgs) -> anyhow::Result<()> {
    if args.all_tags {
        tracing::warn!(
            "docker push --all-tags is not yet supported; pushing only the requested reference",
        );
    }

    let client = DaemonClient::connect()
        .await
        .context("Failed to connect to the zlayer daemon")?;
    let resp = client
        .push_image(&args.image, None, None)
        .await
        .with_context(|| format!("Failed to push image '{}'", args.image))?;

    if !args.quiet {
        if let Some(msg) = resp.get("message").and_then(|v| v.as_str()) {
            println!("{msg}");
        } else {
            println!("Pushed {}", args.image);
        }
    }
    Ok(())
}

/// Handle the `docker images` command.
///
/// Delegates to `DaemonClient::list_images` and prints a Docker-compatible
/// table: `REPOSITORY  TAG  IMAGE ID  CREATED  SIZE`. `CREATED` is reported
/// as `-` until the daemon exposes per-image creation timestamps.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable or the list call fails.
pub async fn handle_images(args: ImagesArgs) -> anyhow::Result<()> {
    let client = DaemonClient::connect()
        .await
        .context("Failed to connect to the zlayer daemon")?;
    let mut images = client
        .list_images()
        .await
        .context("Failed to list images from daemon")?;

    // Apply optional repository filter (prefix match on `name` part).
    if let Some(filter) = args.repository.as_deref() {
        images.retain(|info| {
            let (repo, _) = split_reference(&info.reference);
            repo == filter
        });
    }

    if args.quiet {
        for info in &images {
            let id = info.digest.as_deref().map_or_else(
                || truncate_id(&info.reference, args.no_trunc),
                |d| truncate_id(d, args.no_trunc),
            );
            println!("{id}");
        }
        return Ok(());
    }

    // Docker-style column header.
    println!(
        "{:<40} {:<20} {:<20} {:<15} {:>10}",
        "REPOSITORY", "TAG", "IMAGE ID", "CREATED", "SIZE"
    );
    for info in &images {
        let (repo, tag) = split_reference(&info.reference);
        let id = info
            .digest
            .as_deref()
            .map_or("-", |d| d.strip_prefix("sha256:").unwrap_or(d));
        let id_display = if args.no_trunc {
            id.to_string()
        } else {
            id.chars().take(12).collect::<String>()
        };
        let size = info
            .size_bytes
            .map_or_else(|| "-".to_string(), format_bytes);
        println!(
            "{:<40} {:<20} {:<20} {:<15} {:>10}",
            repo, tag, id_display, "-", size
        );
    }
    if images.is_empty() {
        // Preserve exit-success with no output when there are no matches
        // (matches Docker CLI behaviour under `docker images`).
    }
    Ok(())
}

/// Handle the `docker rmi` command.
///
/// Delegates to `DaemonClient::remove_image` for each image argument.
/// Aggregates successes and failures so one failed removal doesn't abort
/// the batch.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable or every removal failed.
pub async fn handle_rmi(args: RmiArgs) -> anyhow::Result<()> {
    if args.no_prune {
        tracing::warn!(
            "docker rmi --no-prune is not yet forwarded to the daemon; untagged parents may be pruned",
        );
    }

    let client = DaemonClient::connect()
        .await
        .context("Failed to connect to the zlayer daemon")?;

    let mut any_success = false;
    let mut last_err: Option<anyhow::Error> = None;
    for image in &args.images {
        match client.remove_image(image, args.force).await {
            Ok(()) => {
                println!("Untagged: {image}");
                println!("Deleted: {image}");
                any_success = true;
            }
            Err(err) => {
                eprintln!("Error response from daemon: {err}");
                last_err = Some(err.context(format!("Failed to remove image '{image}'")));
            }
        }
    }

    if !any_success {
        if let Some(err) = last_err {
            return Err(err);
        }
    }
    Ok(())
}

/// Handle the `docker tag` command.
///
/// Delegates to `DaemonClient::tag_image`. Creates a new reference
/// (`target`) pointing at an already-cached image (`source`).
///
/// # Errors
///
/// Returns an error if the daemon is unreachable or the tag call fails.
#[allow(clippy::unused_async)]
pub async fn handle_tag(args: TagArgs) -> anyhow::Result<()> {
    let client = DaemonClient::connect()
        .await
        .context("Failed to connect to the zlayer daemon")?;
    client
        .tag_image(&args.source, &args.target)
        .await
        .with_context(|| format!("Failed to tag '{}' as '{}'", args.source, args.target))?;
    Ok(())
}

/// Handle the `docker login` command.
///
/// # Errors
///
/// Always returns an error; this command is not yet implemented.
#[allow(clippy::unused_async)]
pub async fn handle_login(args: LoginArgs) -> anyhow::Result<()> {
    let server = args
        .server
        .as_deref()
        .unwrap_or("https://index.docker.io/v1/");
    tracing::info!(server = %server, "docker login requested");
    anyhow::bail!("not yet implemented — use 'zlayer login'")
}

/// Handle the `docker logout` command.
///
/// # Errors
///
/// Always returns an error; this command is not yet implemented.
#[allow(clippy::unused_async)]
pub async fn handle_logout(args: LogoutArgs) -> anyhow::Result<()> {
    let server = args
        .server
        .as_deref()
        .unwrap_or("https://index.docker.io/v1/");
    tracing::info!(server = %server, "docker logout requested");
    anyhow::bail!("not yet implemented — use 'zlayer logout'")
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Split `repo:tag` or `repo@digest` into `(repo, tag)` — returns
/// `("<reference>", "<none>")` when no tag is present.
fn split_reference(reference: &str) -> (&str, &str) {
    if let Some((repo, tag)) = reference.rsplit_once(':') {
        // Guard against `host:port/name` being misread as `repo:tag`.
        if !tag.contains('/') {
            return (repo, tag);
        }
    }
    (reference, "<none>")
}

/// Truncate an image id/digest to 12 chars unless `no_trunc` is set.
fn truncate_id(id: &str, no_trunc: bool) -> String {
    let stripped = id.strip_prefix("sha256:").unwrap_or(id);
    if no_trunc {
        stripped.to_string()
    } else {
        stripped.chars().take(12).collect()
    }
}

/// Format a byte count into a human-friendly string (B / KB / MB / GB).
#[allow(clippy::cast_precision_loss)]
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_reference_tag() {
        assert_eq!(split_reference("nginx:latest"), ("nginx", "latest"));
        assert_eq!(
            split_reference("library/nginx:1.25"),
            ("library/nginx", "1.25")
        );
    }

    #[test]
    fn split_reference_no_tag() {
        assert_eq!(split_reference("nginx"), ("nginx", "<none>"));
    }

    #[test]
    fn split_reference_with_port() {
        // `host:port/name` must not be split on the port colon.
        assert_eq!(
            split_reference("registry.example.com:5000/app"),
            ("registry.example.com:5000/app", "<none>")
        );
    }

    #[test]
    fn truncate_id_default() {
        assert_eq!(
            truncate_id("sha256:abcdef0123456789deadbeef", false),
            "abcdef012345"
        );
    }

    #[test]
    fn truncate_id_no_trunc() {
        assert_eq!(
            truncate_id("sha256:abcdef0123456789deadbeef", true),
            "abcdef0123456789deadbeef"
        );
    }

    #[test]
    fn format_bytes_units() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2 * 1024), "2.0 KB");
        assert_eq!(format_bytes(3 * 1024 * 1024), "3.0 MB");
        assert_eq!(format_bytes(5 * 1024 * 1024 * 1024), "5.0 GB");
    }
}
