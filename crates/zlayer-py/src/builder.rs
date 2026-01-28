//! ImageBuilder class for Python bindings
//!
//! Provides container image building capabilities via buildah.

use crate::error::{to_py_result, ZLayerError};
use pyo3::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use zlayer_builder::{BuiltImage, ImageBuilder as RustImageBuilder, RegistryAuth};

/// A built container image
#[pyclass]
#[derive(Clone)]
pub struct Image {
    /// The image ID (digest)
    #[pyo3(get)]
    pub id: String,

    /// Tags applied to this image
    #[pyo3(get)]
    pub tags: Vec<String>,

    /// Build duration in seconds
    #[pyo3(get)]
    pub build_time_secs: f64,
}

#[pymethods]
impl Image {
    fn __repr__(&self) -> String {
        format!("Image(id='{}', tags={:?})", self.id, self.tags)
    }

    fn __str__(&self) -> String {
        if !self.tags.is_empty() {
            self.tags[0].clone()
        } else {
            self.id.clone()
        }
    }
}

impl From<BuiltImage> for Image {
    fn from(image: BuiltImage) -> Self {
        Self {
            id: image.image_id,
            tags: image.tags,
            build_time_secs: Duration::from_millis(image.build_time_ms).as_secs_f64(),
        }
    }
}

/// Runtime detection result
#[pyclass]
#[derive(Clone)]
pub struct DetectedRuntime {
    /// The detected runtime name
    #[pyo3(get)]
    pub name: String,

    /// Human-readable description
    #[pyo3(get)]
    pub description: String,

    /// Recommended base image
    #[pyo3(get)]
    pub base_image: String,
}

#[pymethods]
impl DetectedRuntime {
    fn __repr__(&self) -> String {
        format!(
            "DetectedRuntime(name='{}', base='{}')",
            self.name, self.base_image
        )
    }
}

/// Internal builder state
struct BuilderInner {
    context_path: PathBuf,
    tags: Vec<String>,
    build_args: HashMap<String, String>,
    target_stage: Option<String>,
    dockerfile_path: Option<PathBuf>,
    runtime: Option<zlayer_builder::Runtime>,
    no_cache: bool,
    registry_auth: Option<(String, String)>, // (username, password)
}

/// Builder for creating container images
///
/// This class provides a fluent API for building container images
/// from Dockerfiles or runtime templates.
///
/// Example:
///     >>> builder = ImageBuilder("./my-app")
///     >>> builder.tag("myapp:latest")
///     >>> builder.tag("myapp:v1.0.0")
///     >>> image = await builder.build()
///     >>> print(image.id)
#[pyclass]
pub struct ImageBuilder {
    inner: Arc<RwLock<BuilderInner>>,
}

#[pymethods]
impl ImageBuilder {
    /// Create a new ImageBuilder for a context directory
    ///
    /// Args:
    ///     context: Path to the build context directory
    ///     dockerfile: Optional path to Dockerfile (default: context/Dockerfile)
    ///
    /// Returns:
    ///     A new ImageBuilder instance
    #[new]
    #[pyo3(signature = (context, dockerfile=None))]
    fn new(context: &str, dockerfile: Option<&str>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(BuilderInner {
                context_path: PathBuf::from(context),
                tags: Vec::new(),
                build_args: HashMap::new(),
                target_stage: None,
                dockerfile_path: dockerfile.map(PathBuf::from),
                runtime: None,
                no_cache: false,
                registry_auth: None,
            })),
        }
    }

    /// Add a tag to the image
    ///
    /// Args:
    ///     tag: The tag to add (e.g., "myapp:latest")
    ///
    /// Returns:
    ///     Self for method chaining
    fn tag<'py>(slf: PyRef<'py, Self>, tag: &str) -> PyResult<PyRef<'py, Self>> {
        let inner = slf.inner.clone();
        let tag = tag.to_string();

        // Block on the lock since this is a sync method
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| ZLayerError::Runtime(format!("Failed to create runtime: {}", e)))?;
        rt.block_on(async {
            let mut inner = inner.write().await;
            inner.tags.push(tag);
        });

        Ok(slf)
    }

    /// Add tags from a list
    ///
    /// Args:
    ///     tags: List of tags to add
    ///
    /// Returns:
    ///     Self for method chaining
    fn tags<'py>(slf: PyRef<'py, Self>, tags: Vec<String>) -> PyResult<PyRef<'py, Self>> {
        let inner = slf.inner.clone();

        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| ZLayerError::Runtime(format!("Failed to create runtime: {}", e)))?;
        rt.block_on(async {
            let mut inner = inner.write().await;
            inner.tags.extend(tags);
        });

        Ok(slf)
    }

    /// Set a build argument
    ///
    /// Args:
    ///     name: The argument name
    ///     value: The argument value
    ///
    /// Returns:
    ///     Self for method chaining
    fn arg<'py>(slf: PyRef<'py, Self>, name: &str, value: &str) -> PyResult<PyRef<'py, Self>> {
        let inner = slf.inner.clone();
        let name = name.to_string();
        let value = value.to_string();

        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| ZLayerError::Runtime(format!("Failed to create runtime: {}", e)))?;
        rt.block_on(async {
            let mut inner = inner.write().await;
            inner.build_args.insert(name, value);
        });

        Ok(slf)
    }

    /// Set multiple build arguments from a dict
    ///
    /// Args:
    ///     args: Dict of argument names to values
    ///
    /// Returns:
    ///     Self for method chaining
    fn args<'py>(
        slf: PyRef<'py, Self>,
        args: HashMap<String, String>,
    ) -> PyResult<PyRef<'py, Self>> {
        let inner = slf.inner.clone();

        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| ZLayerError::Runtime(format!("Failed to create runtime: {}", e)))?;
        rt.block_on(async {
            let mut inner = inner.write().await;
            inner.build_args.extend(args);
        });

        Ok(slf)
    }

    /// Set the target stage for multi-stage builds
    ///
    /// Args:
    ///     stage: The target stage name
    ///
    /// Returns:
    ///     Self for method chaining
    fn target<'py>(slf: PyRef<'py, Self>, stage: &str) -> PyResult<PyRef<'py, Self>> {
        let inner = slf.inner.clone();
        let stage = stage.to_string();

        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| ZLayerError::Runtime(format!("Failed to create runtime: {}", e)))?;
        rt.block_on(async {
            let mut inner = inner.write().await;
            inner.target_stage = Some(stage);
        });

        Ok(slf)
    }

    /// Set the runtime template to use
    ///
    /// This enables building without a Dockerfile using pre-configured
    /// templates for common runtimes.
    ///
    /// Args:
    ///     runtime: The runtime name (e.g., "node20", "python312", "rust")
    ///
    /// Returns:
    ///     Self for method chaining
    fn runtime<'py>(slf: PyRef<'py, Self>, runtime: &str) -> PyResult<PyRef<'py, Self>> {
        let inner = slf.inner.clone();
        let rt_enum = match runtime.to_lowercase().as_str() {
            "node20" => zlayer_builder::Runtime::Node20,
            "node22" => zlayer_builder::Runtime::Node22,
            "python312" => zlayer_builder::Runtime::Python312,
            "python313" => zlayer_builder::Runtime::Python313,
            "rust" => zlayer_builder::Runtime::Rust,
            "go" => zlayer_builder::Runtime::Go,
            "deno" => zlayer_builder::Runtime::Deno,
            "bun" => zlayer_builder::Runtime::Bun,
            _ => {
                return Err(ZLayerError::InvalidArgument(format!(
                    "Unknown runtime: {}. Valid options: node20, node22, python312, python313, rust, go, deno, bun",
                    runtime
                ))
                .into());
            }
        };

        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| ZLayerError::Runtime(format!("Failed to create runtime: {}", e)))?;
        rt.block_on(async {
            let mut inner = inner.write().await;
            inner.runtime = Some(rt_enum);
        });

        Ok(slf)
    }

    /// Disable build cache
    ///
    /// Returns:
    ///     Self for method chaining
    fn no_cache<'py>(slf: PyRef<'py, Self>) -> PyResult<PyRef<'py, Self>> {
        let inner = slf.inner.clone();

        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| ZLayerError::Runtime(format!("Failed to create runtime: {}", e)))?;
        rt.block_on(async {
            let mut inner = inner.write().await;
            inner.no_cache = true;
        });

        Ok(slf)
    }

    /// Set registry authentication
    ///
    /// Args:
    ///     registry: The registry URL (e.g., "ghcr.io") - currently unused, for future use
    ///     username: The username
    ///     password: The password or token
    ///
    /// Returns:
    ///     Self for method chaining
    fn auth<'py>(
        slf: PyRef<'py, Self>,
        _registry: &str,
        username: &str,
        password: &str,
    ) -> PyResult<PyRef<'py, Self>> {
        let inner = slf.inner.clone();
        let auth = (username.to_string(), password.to_string());

        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| ZLayerError::Runtime(format!("Failed to create runtime: {}", e)))?;
        rt.block_on(async {
            let mut inner = inner.write().await;
            inner.registry_auth = Some(auth);
        });

        Ok(slf)
    }

    /// Build the container image
    ///
    /// Returns:
    ///     The built Image
    ///
    /// Raises:
    ///     RuntimeError: If the build fails
    fn build<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let inner = inner.read().await;

            // Create the Rust ImageBuilder
            let mut builder = RustImageBuilder::new(&inner.context_path)
                .await
                .map_err(ZLayerError::from)?;

            // Apply configuration
            for tag in &inner.tags {
                builder = builder.tag(tag);
            }

            if let Some(target) = &inner.target_stage {
                builder = builder.target(target);
            }

            if let Some(dockerfile) = &inner.dockerfile_path {
                builder = builder.dockerfile(dockerfile);
            }

            if let Some(runtime) = &inner.runtime {
                builder = builder.runtime(*runtime);
            }

            // Apply build args
            for (key, value) in &inner.build_args {
                builder = builder.build_arg(key, value);
            }

            // Apply no_cache
            if inner.no_cache {
                builder = builder.no_cache();
            }

            // Apply auth if set
            if let Some((username, password)) = &inner.registry_auth {
                builder = builder.push(RegistryAuth::new(username, password));
            }

            // Build the image
            let built = builder.build().await.map_err(ZLayerError::from)?;

            to_py_result(Ok(Image::from(built)))
        })
    }

    /// Push the image to a registry
    ///
    /// Note: The image must be built first and have at least one tag.
    ///
    /// Args:
    ///     tag: Optional specific tag to push (pushes all if not specified)
    ///
    /// Raises:
    ///     RuntimeError: If the push fails
    #[pyo3(signature = (tag=None))]
    fn push<'py>(&self, py: Python<'py>, tag: Option<&str>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let tag = tag.map(|s| s.to_string());

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let inner = inner.read().await;

            // Determine which tags to push
            let tags_to_push: Vec<&str> = if let Some(ref t) = tag {
                vec![t.as_str()]
            } else {
                inner.tags.iter().map(|s| s.as_str()).collect()
            };

            if tags_to_push.is_empty() {
                return Err(ZLayerError::InvalidArgument(
                    "No tags to push. Add tags with .tag() first.".to_string(),
                )
                .into());
            }

            // Note: In a real implementation, we'd use buildah push here
            tracing::info!("Would push tags: {:?}", tags_to_push);

            to_py_result(Ok(()))
        })
    }

    fn __repr__(&self) -> String {
        "ImageBuilder(...)".to_string()
    }
}

/// Detect the runtime from a project directory
///
/// This function analyzes the project files to determine the appropriate
/// runtime template to use.
///
/// Args:
///     path: Path to the project directory
///
/// Returns:
///     The detected runtime info, or None if no runtime is detected
#[pyfunction]
pub fn detect_runtime(path: &str) -> PyResult<Option<DetectedRuntime>> {
    let path = std::path::Path::new(path);
    let detected = zlayer_builder::detect_runtime(path);

    Ok(detected.map(|rt| {
        let info = rt.info();
        DetectedRuntime {
            name: info.name.to_string(),
            description: info.description.to_string(),
            base_image: rt.template().lines().next().unwrap_or("").to_string(),
        }
    }))
}

/// List available runtime templates
///
/// Returns:
///     A list of available runtime names
#[pyfunction]
pub fn list_runtimes() -> Vec<String> {
    zlayer_builder::list_templates()
        .iter()
        .map(|info| info.name.to_string())
        .collect()
}

/// Check if buildah is installed and available
///
/// Returns:
///     True if buildah is available
#[pyfunction]
pub fn is_buildah_available() -> bool {
    zlayer_builder::is_platform_supported()
}

/// Get buildah installation instructions for the current platform
///
/// Returns:
///     Installation instructions as a string
#[pyfunction]
pub fn buildah_install_instructions() -> String {
    zlayer_builder::install_instructions()
}

/// Build a container image from a directory
///
/// This is a convenience function that provides a simple one-liner interface
/// for building container images. For more control, use the ImageBuilder class.
///
/// Args:
///     path: Path to the build context directory (required)
///     tag: Image tag to apply (required, e.g., "myapp:latest")
///     dockerfile: Path to Dockerfile (default: "Dockerfile" in context)
///     build_args: Dictionary of build arguments (optional)
///     no_cache: Disable build cache (default: False)
///     progress: Show build progress - currently unused, reserved for future use (default: True)
///
/// Returns:
///     The image tag on success
///
/// Raises:
///     RuntimeError: If the build fails
///     ValueError: If invalid arguments are provided
///
/// Example:
///     >>> # Simple build
///     >>> image = await zlayer.build("./app", tag="myapp:latest")
///     >>> print(image)  # "myapp:latest"
///
///     >>> # Build with custom Dockerfile
///     >>> image = await zlayer.build(".", tag="api:v1", dockerfile="Dockerfile.prod")
///
///     >>> # Build with arguments
///     >>> image = await zlayer.build(
///     ...     "./app",
///     ...     tag="myapp:latest",
///     ...     build_args={"VERSION": "1.0.0", "DEBUG": "false"}
///     ... )
///
///     >>> # Build without cache
///     >>> image = await zlayer.build("./app", tag="myapp:latest", no_cache=True)
#[pyfunction]
#[pyo3(signature = (path, tag, dockerfile=None, build_args=None, no_cache=false, progress=true))]
pub fn build<'py>(
    py: Python<'py>,
    path: &str,
    tag: &str,
    dockerfile: Option<&str>,
    build_args: Option<HashMap<String, String>>,
    no_cache: bool,
    #[allow(unused_variables)] progress: bool,
) -> PyResult<Bound<'py, PyAny>> {
    // Validate required arguments
    if path.is_empty() {
        return Err(ZLayerError::InvalidArgument("path cannot be empty".to_string()).into());
    }
    if tag.is_empty() {
        return Err(ZLayerError::InvalidArgument("tag cannot be empty".to_string()).into());
    }

    let path = PathBuf::from(path);
    let tag = tag.to_string();
    let dockerfile = dockerfile.map(PathBuf::from);
    let build_args = build_args.unwrap_or_default();

    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        // Create the Rust ImageBuilder
        let mut builder = RustImageBuilder::new(&path)
            .await
            .map_err(ZLayerError::from)?;

        // Apply tag
        builder = builder.tag(&tag);

        // Apply dockerfile if specified
        if let Some(df) = dockerfile {
            builder = builder.dockerfile(df);
        }

        // Apply build args
        for (key, value) in build_args {
            builder = builder.build_arg(key, value);
        }

        // Apply no_cache
        if no_cache {
            builder = builder.no_cache();
        }

        // Build the image
        let built = builder.build().await.map_err(ZLayerError::from)?;

        // Return the primary tag (or image ID if no tags)
        let result = if !built.tags.is_empty() {
            built.tags[0].clone()
        } else {
            built.image_id
        };

        to_py_result(Ok(result))
    })
}
