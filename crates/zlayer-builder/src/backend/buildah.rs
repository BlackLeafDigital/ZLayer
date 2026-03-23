//! Buildah-backed build backend.
//!
//! Wraps [`BuildahExecutor`] to implement the [`BuildBackend`] trait.

use std::path::Path;

use crate::buildah::{BuildahCommand, BuildahExecutor};
use crate::builder::{BuildOptions, BuiltImage, ImageBuilder, RegistryAuth};
use crate::dockerfile::Dockerfile;
use crate::error::Result;
use crate::tui::BuildEvent;

use super::BuildBackend;

/// Build backend that delegates to the `buildah` CLI.
pub struct BuildahBackend {
    executor: BuildahExecutor,
}

impl BuildahBackend {
    /// Try to create a new `BuildahBackend`.
    ///
    /// Returns `Ok` if buildah is found and functional, `Err` otherwise.
    ///
    /// # Errors
    ///
    /// Returns an error if buildah is not installed or is not responding.
    pub async fn try_new() -> Result<Self> {
        let executor = BuildahExecutor::new_async().await?;
        if !executor.is_available().await {
            return Err(crate::error::BuildError::BuildahNotFound {
                message: "buildah is installed but not responding".into(),
            });
        }
        Ok(Self { executor })
    }

    /// Create a new `BuildahBackend`, returning an error if buildah is not available.
    ///
    /// # Errors
    ///
    /// Returns an error if buildah is not installed or cannot be initialized.
    pub async fn new() -> Result<Self> {
        let executor = BuildahExecutor::new_async().await?;
        Ok(Self { executor })
    }

    /// Create a `BuildahBackend` from an existing executor.
    #[must_use]
    pub fn with_executor(executor: BuildahExecutor) -> Self {
        Self { executor }
    }

    /// Borrow the inner executor (useful for low-level operations).
    #[must_use]
    pub fn executor(&self) -> &BuildahExecutor {
        &self.executor
    }
}

#[async_trait::async_trait]
impl BuildBackend for BuildahBackend {
    async fn build_image(
        &self,
        context: &Path,
        _dockerfile: &Dockerfile,
        options: &BuildOptions,
        event_tx: Option<std::sync::mpsc::Sender<BuildEvent>>,
    ) -> Result<BuiltImage> {
        // Bootstrap: delegate to the existing ImageBuilder which contains all
        // the multi-stage build orchestration logic. In a later step (Step 3)
        // the core loop will be extracted into this method directly and the
        // `_dockerfile` parameter will be used instead of re-parsing.
        let mut builder = ImageBuilder::with_executor(context, self.executor.clone())?;

        // Transfer individual options via the existing builder API
        for tag in &options.tags {
            builder = builder.tag(tag);
        }
        for (k, v) in &options.build_args {
            builder = builder.build_arg(k, v);
        }
        if let Some(ref target) = options.target {
            builder = builder.target(target);
        }
        if let Some(ref dockerfile_path) = options.dockerfile {
            builder = builder.dockerfile(dockerfile_path);
        }
        if let Some(ref zimagefile_path) = options.zimagefile {
            builder = builder.zimagefile(zimagefile_path);
        }
        if let Some(ref rt) = options.runtime {
            builder = builder.runtime(*rt);
        }
        if options.no_cache {
            builder = builder.no_cache();
        }
        if options.squash {
            builder = builder.squash();
        }
        if let Some(ref fmt) = options.format {
            builder = builder.format(fmt);
        }
        if let Some(ref auth) = options.registry_auth {
            builder = builder.push(auth.clone());
        } else if options.push {
            builder = builder.push_without_auth();
        }

        // Attach event channel if provided
        if let Some(tx) = event_tx {
            builder = builder.with_events(tx);
        }

        // Run the build (ImageBuilder will re-resolve the Dockerfile from options)
        builder.build().await
    }

    async fn push_image(&self, tag: &str, auth: Option<&RegistryAuth>) -> Result<()> {
        let mut cmd = BuildahCommand::push(tag);
        if let Some(auth) = auth {
            cmd = cmd
                .arg("--creds")
                .arg(format!("{}:{}", auth.username, auth.password));
        }
        self.executor.execute_checked(&cmd).await?;
        Ok(())
    }

    async fn tag_image(&self, image: &str, new_tag: &str) -> Result<()> {
        let cmd = BuildahCommand::tag(image, new_tag);
        self.executor.execute_checked(&cmd).await?;
        Ok(())
    }

    async fn manifest_create(&self, name: &str) -> Result<()> {
        let cmd = BuildahCommand::manifest_create(name);
        self.executor.execute_checked(&cmd).await?;
        Ok(())
    }

    async fn manifest_add(&self, manifest: &str, image: &str) -> Result<()> {
        let cmd = BuildahCommand::manifest_add(manifest, image);
        self.executor.execute_checked(&cmd).await?;
        Ok(())
    }

    async fn manifest_push(&self, name: &str, destination: &str) -> Result<()> {
        let cmd = BuildahCommand::manifest_push(name, destination);
        self.executor.execute_checked(&cmd).await?;
        Ok(())
    }

    async fn is_available(&self) -> bool {
        self.executor.is_available().await
    }

    fn name(&self) -> &'static str {
        "buildah"
    }
}
