//! Docker image management commands — build, pull, push, tag, list, remove, login/logout.

use clap::Parser;

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
// Stub handlers
// ---------------------------------------------------------------------------

/// Handle the `docker build` command.
///
/// # Errors
///
/// Always returns an error; this command is not yet implemented.
#[allow(clippy::unused_async)]
pub async fn handle_build(args: BuildArgs) -> anyhow::Result<()> {
    let tags: String = args.tag.join(", ");
    tracing::info!(context = %args.context, tags = %tags, "docker build requested");
    anyhow::bail!("not yet implemented — use 'zlayer build'")
}

/// Handle the `docker pull` command.
///
/// # Errors
///
/// Always returns an error; this command is not yet implemented.
#[allow(clippy::unused_async)]
pub async fn handle_pull(args: PullArgs) -> anyhow::Result<()> {
    tracing::info!(image = %args.image, "docker pull requested");
    anyhow::bail!("not yet implemented — use 'zlayer pull'")
}

/// Handle the `docker push` command.
///
/// # Errors
///
/// Always returns an error; this command is not yet implemented.
#[allow(clippy::unused_async)]
pub async fn handle_push(args: PushArgs) -> anyhow::Result<()> {
    tracing::info!(image = %args.image, "docker push requested");
    anyhow::bail!("not yet implemented — use 'zlayer push'")
}

/// Handle the `docker images` command.
///
/// # Errors
///
/// Always returns an error; this command is not yet implemented.
#[allow(clippy::unused_async)]
pub async fn handle_images(args: ImagesArgs) -> anyhow::Result<()> {
    let repo = args.repository.as_deref().unwrap_or("<all>");
    tracing::info!(repository = %repo, "docker images requested");
    anyhow::bail!("not yet implemented — use 'zlayer images'")
}

/// Handle the `docker rmi` command.
///
/// # Errors
///
/// Always returns an error; this command is not yet implemented.
#[allow(clippy::unused_async)]
pub async fn handle_rmi(args: RmiArgs) -> anyhow::Result<()> {
    let targets: String = args.images.join(", ");
    tracing::info!(images = %targets, force = %args.force, "docker rmi requested");
    anyhow::bail!("not yet implemented — use 'zlayer rmi'")
}

/// Handle the `docker tag` command.
///
/// # Errors
///
/// Always returns an error; this command is not yet implemented.
#[allow(clippy::unused_async)]
pub async fn handle_tag(args: TagArgs) -> anyhow::Result<()> {
    tracing::info!(source = %args.source, target = %args.target, "docker tag requested");
    anyhow::bail!("not yet implemented — use 'zlayer tag'")
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
