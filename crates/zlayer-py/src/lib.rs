//! ZLayer Python Bindings
//!
//! This crate provides Python bindings for the ZLayer container runtime,
//! enabling Python developers to orchestrate containers using ZLayer's
//! specification-driven approach.
//!
//! # Overview
//!
//! The bindings expose the following main components:
//!
//! - [`Container`] - Individual container lifecycle management
//! - [`Runtime`] - Deployment-level orchestration
//! - [`ImageBuilder`] - Container image building via buildah
//! - [`Spec`] - ZLayer specification parsing and validation
//!
//! # Quick Start
//!
//! ```python
//! import asyncio
//! from zlayer import Runtime, Container, ImageBuilder
//!
//! async def main():
//!     # Deploy from a spec file
//!     runtime = Runtime()
//!     await runtime.deploy_spec("deployment.yaml")
//!
//!     # Or work with individual containers
//!     container = Container.create("nginx:latest", ports={"http": 80})
//!     await container.start()
//!     await container.wait_healthy()
//!
//!     # Build custom images
//!     builder = ImageBuilder("./my-app")
//!     builder.tag("myapp:latest")
//!     image = await builder.build()
//!
//! asyncio.run(main())
//! ```

mod builder;
mod container;
mod error;
mod run;
mod runtime;
mod spec;

use pyo3::prelude::*;

// Re-export types for internal use
pub use builder::{DetectedRuntime, Image, ImageBuilder};
pub use container::{Container, PyContainerState};
pub use error::{Result, ZLayerError};
pub use runtime::{Runtime, RuntimeOptions};
pub use spec::{
    DependsSpec, EndpointSpec, HealthSpec, ResourcesSpec, ScaleSpec, ServiceSpec, Spec,
};

/// ZLayer - Python bindings for the ZLayer container runtime
///
/// This module provides comprehensive tools for container orchestration:
///
/// Classes:
///     Container: Individual container lifecycle management
///     Runtime: Deployment-level orchestration
///     RuntimeOptions: Configuration for Runtime creation
///     ImageBuilder: Container image building
///     Image: A built container image
///     Spec: Deployment specification parsing
///     ServiceSpec: Service configuration
///     EndpointSpec: Endpoint configuration
///     ScaleSpec: Scaling configuration
///     DependsSpec: Dependency configuration
///     ResourcesSpec: Resource limits
///     HealthSpec: Health check configuration
///     ContainerState: Container state enumeration
///
/// Functions:
///     run: Run a container quickly with simple parameters
///     build: Build a container image from a directory
///     parse_spec: Parse a YAML spec string
///     validate_spec: Validate a YAML spec string
///     detect_runtime: Detect runtime from project files
///     list_runtimes: List available runtime templates
///     is_buildah_available: Check if buildah is installed
///     buildah_install_instructions: Get buildah installation instructions
///     create_service_spec: Create a service spec programmatically
#[pymodule]
fn _zlayer(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Container classes
    m.add_class::<Container>()?;
    m.add_class::<PyContainerState>()?;

    // Runtime classes
    m.add_class::<Runtime>()?;
    m.add_class::<RuntimeOptions>()?;

    // Builder classes
    m.add_class::<ImageBuilder>()?;
    m.add_class::<Image>()?;
    m.add_class::<DetectedRuntime>()?;

    // Spec classes
    m.add_class::<Spec>()?;
    m.add_class::<ServiceSpec>()?;
    m.add_class::<EndpointSpec>()?;
    m.add_class::<ScaleSpec>()?;
    m.add_class::<DependsSpec>()?;
    m.add_class::<ResourcesSpec>()?;
    m.add_class::<HealthSpec>()?;

    // Spec functions
    m.add_function(wrap_pyfunction!(runtime::parse_spec, m)?)?;
    m.add_function(wrap_pyfunction!(runtime::validate_spec, m)?)?;
    m.add_function(wrap_pyfunction!(spec::create_service_spec, m)?)?;

    // Builder functions
    m.add_function(wrap_pyfunction!(builder::build, m)?)?;
    m.add_function(wrap_pyfunction!(builder::detect_runtime, m)?)?;
    m.add_function(wrap_pyfunction!(builder::list_runtimes, m)?)?;
    m.add_function(wrap_pyfunction!(builder::is_buildah_available, m)?)?;
    m.add_function(wrap_pyfunction!(builder::buildah_install_instructions, m)?)?;

    // Convenience functions
    m.add_function(wrap_pyfunction!(run::run, m)?)?;

    // Version info
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("__author__", "Zach Handley <zachhandley@gmail.com>")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_module_compiles() {
        // Basic smoke test that the module compiles
        assert!(true);
    }
}
