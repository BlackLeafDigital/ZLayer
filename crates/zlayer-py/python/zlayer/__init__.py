"""
ZLayer - Python bindings for the ZLayer container runtime.

This package provides comprehensive tools for container orchestration using
ZLayer's specification-driven approach.

Quick Start:
    >>> import asyncio
    >>> import zlayer
    >>>
    >>> async def main():
    ...     # Quick container run (simplest API)
    ...     container = await zlayer.run("nginx:latest", ports={"http": 80})
    ...     print(f"Container running: {container.id}")
    ...
    ...     # Quick image build (simplest API)
    ...     image = await zlayer.build("./my-app", tag="myapp:latest")
    ...     print(f"Built image: {image.id}")
    ...
    ...     # Or use the full Runtime API for orchestration
    ...     runtime = zlayer.Runtime()
    ...     await runtime.deploy_spec("deployment.yaml")
    ...
    ...     # Or work with individual containers directly
    ...     container = zlayer.Container.create("nginx:latest", ports={"http": 80})
    ...     await container.start()
    ...     await container.wait_healthy()
    ...
    ...     # Full ImageBuilder API for complex builds
    ...     builder = zlayer.ImageBuilder("./my-app")
    ...     builder.tag("myapp:latest")
    ...     image = await builder.build()
    >>>
    >>> asyncio.run(main())

Classes:
    Container: Individual container lifecycle management
    Runtime: Deployment-level orchestration
    RuntimeOptions: Configuration for Runtime creation
    ImageBuilder: Container image building via buildah
    Image: A built container image
    Spec: Deployment specification parsing
    ServiceSpec: Service configuration
    EndpointSpec: Endpoint configuration
    ScaleSpec: Scaling configuration
    DependsSpec: Dependency configuration
    ResourcesSpec: Resource limits
    HealthSpec: Health check configuration
    ContainerState: Container state enumeration

Convenience Functions:
    run: Quickly run a container with simple parameters (async)
    build: Quickly build a container image from a directory (async)

Utility Functions:
    parse_spec: Parse a YAML spec string
    validate_spec: Validate a YAML spec string
    detect_runtime: Detect runtime from project files
    list_runtimes: List available runtime templates
    is_buildah_available: Check if buildah is installed
    buildah_install_instructions: Get buildah installation instructions
    create_service_spec: Create a service spec programmatically
"""

from zlayer._zlayer import (
    # Container
    Container,
    PyContainerState as ContainerState,
    # Runtime
    Runtime,
    RuntimeOptions,
    # Builder
    ImageBuilder,
    Image,
    DetectedRuntime,
    # Spec
    Spec,
    ServiceSpec,
    EndpointSpec,
    ScaleSpec,
    DependsSpec,
    ResourcesSpec,
    HealthSpec,
    # Convenience functions (async)
    run,
    build,
    # Utility functions
    parse_spec,
    validate_spec,
    detect_runtime,
    list_runtimes,
    is_buildah_available,
    buildah_install_instructions,
    create_service_spec,
    # Metadata
    __version__,
    __author__,
)

__all__ = [
    # Container
    "Container",
    "ContainerState",
    # Runtime
    "Runtime",
    "RuntimeOptions",
    # Builder
    "ImageBuilder",
    "Image",
    "DetectedRuntime",
    # Spec
    "Spec",
    "ServiceSpec",
    "EndpointSpec",
    "ScaleSpec",
    "DependsSpec",
    "ResourcesSpec",
    "HealthSpec",
    # Convenience functions (async)
    "run",
    "build",
    # Utility functions
    "parse_spec",
    "validate_spec",
    "detect_runtime",
    "list_runtimes",
    "is_buildah_available",
    "buildah_install_instructions",
    "create_service_spec",
    # Metadata
    "__version__",
    "__author__",
]
