# zlayer-py

Python bindings for the ZLayer container runtime.

## Installation

### From PyPI (when published)

```bash
pip install zlayer
```

### From Source

```bash
# Requires Rust toolchain and maturin
pip install maturin
cd crates/zlayer-py
maturin develop
```

## Quick Start

```python
import asyncio
from zlayer import Runtime, Container, ImageBuilder

async def main():
    # Deploy from a spec file
    runtime = Runtime()
    await runtime.deploy_spec("deployment.yaml")

    # Scale a service
    await runtime.scale("my-deployment", "api", 3)

    # Check status
    status = await runtime.status()
    print(status)

asyncio.run(main())
```

## Quick Container Run

The easiest way to run a container is using the `run()` function:

```python
import asyncio
import zlayer

async def main():
    # Run nginx with port mapping
    container = await zlayer.run("nginx:latest", ports={80: 8080})
    print(f"Started: {container.id}")

    # Run redis with a custom name
    container = await zlayer.run("redis:alpine", name="my-redis")

    # Run a one-shot command (waits for completion)
    container = await zlayer.run(
        "python:3.12",
        command=["python", "-c", "print('hello')"],
        detach=False
    )

    # Run with environment variables
    container = await zlayer.run(
        "postgres:16",
        ports={5432: 5432},
        env={"POSTGRES_PASSWORD": "secret"}
    )

    # Stop and clean up
    await container.stop()
    await container.remove()

asyncio.run(main())
```

## Working with Containers

```python
import asyncio
from zlayer import Container

async def main():
    # Create a container from an image
    container = Container.create(
        "nginx:latest",
        ports={"http": 80},
        env={"NGINX_HOST": "localhost"}
    )

    # Start the container
    await container.start()

    # Wait for it to be healthy
    await container.wait_healthy(timeout=30)

    # Get logs
    logs = await container.logs(tail=50)
    print(logs)

    # Stop and remove
    await container.stop()
    await container.remove()

asyncio.run(main())
```

## Building Images

### Quick Build (Convenience Function)

The simplest way to build an image is using the `build()` function:

```python
import asyncio
import zlayer

async def main():
    # Simple build
    image = await zlayer.build("./app", tag="myapp:latest")
    print(f"Built: {image}")

    # Build with custom Dockerfile
    image = await zlayer.build(".", tag="api:v1", dockerfile="Dockerfile.prod")

    # Build with arguments
    image = await zlayer.build(
        "./app",
        tag="myapp:latest",
        build_args={"VERSION": "1.0.0", "DEBUG": "false"}
    )

    # Build without cache
    image = await zlayer.build("./app", tag="myapp:latest", no_cache=True)

asyncio.run(main())
```

### ImageBuilder (Full Control)

For more control over the build process, use the `ImageBuilder` class:

```python
import asyncio
from zlayer import ImageBuilder, detect_runtime

async def main():
    # Auto-detect runtime from project files
    detected = detect_runtime("./my-app")
    if detected:
        print(f"Detected: {detected.name}")

    # Build an image
    builder = ImageBuilder("./my-app")
    builder.tag("myapp:latest")
    builder.tag("myapp:v1.0.0")

    # Optionally use a runtime template
    builder.runtime("node20")

    # Build
    image = await builder.build()
    print(f"Built: {image.id}")

asyncio.run(main())
```

## Working with Specs

```python
from zlayer import Spec, validate_spec

# Validate a spec
yaml_content = """
version: v1
deployment: my-app
services:
  api:
    rtype: service
    image:
      name: myapp:latest
    endpoints:
      - name: http
        protocol: http
        port: 8080
"""

# Validate
is_valid = validate_spec(yaml_content)
print(f"Valid: {is_valid}")

# Parse
spec = Spec.from_yaml(yaml_content)
print(f"Deployment: {spec.deployment}")
print(f"Services: {spec.services}")

# Access service details
api = spec.get_service("api")
if api:
    print(f"Image: {api.image}")
    print(f"Endpoints: {api.endpoints}")
```

## API Reference

### Container

- `Container.create(image, ports=None, env=None, name=None)` - Create a new container
- `container.start()` - Start the container
- `container.stop(timeout=30)` - Stop the container
- `container.remove()` - Remove the container
- `container.logs(tail=100)` - Get container logs
- `container.wait_healthy(timeout=60)` - Wait for healthy status
- `container.exec(command)` - Execute a command in the container
- `container.id` - Container ID
- `container.status` - Current status string

### Runtime

- `Runtime(options=None)` - Create a new runtime
- `Runtime.create(options=None)` - Create a runtime asynchronously
- `runtime.deploy_spec(path)` - Deploy from a YAML file
- `runtime.deploy_yaml(yaml)` - Deploy from a YAML string
- `runtime.scale(deployment, service, replicas)` - Scale a service
- `runtime.status(deployment=None)` - Get status
- `runtime.list_services()` - List all services
- `runtime.get_container(service, replica=1)` - Get a container
- `runtime.remove_service(service)` - Remove a service
- `runtime.shutdown()` - Shutdown the runtime

### ImageBuilder

- `ImageBuilder(context, dockerfile=None)` - Create a new builder
- `builder.tag(tag)` - Add a tag
- `builder.tags(tags)` - Add multiple tags
- `builder.arg(name, value)` - Set a build argument
- `builder.args(args)` - Set multiple build arguments
- `builder.target(stage)` - Set target stage
- `builder.runtime(name)` - Use a runtime template
- `builder.no_cache()` - Disable caching
- `builder.auth(registry, username, password)` - Set registry auth
- `builder.build()` - Build the image
- `builder.push(tag=None)` - Push to registry

### Spec

- `Spec.from_yaml(yaml)` - Parse from YAML string
- `Spec.from_file(path)` - Parse from file
- `spec.version` - Spec version
- `spec.deployment` - Deployment name
- `spec.services` - List of service names
- `spec.get_service(name)` - Get a service spec
- `spec.to_yaml()` - Convert to YAML
- `spec.to_dict()` - Convert to dict

### Helper Functions

- `run(image, name=None, ports=None, env=None, command=None, detach=True)` - Run a container quickly
- `build(path, tag, dockerfile=None, build_args=None, no_cache=False, progress=True)` - Build an image (convenience function)
- `parse_spec(yaml)` - Parse a YAML spec to dict
- `validate_spec(yaml)` - Validate a YAML spec
- `detect_runtime(path)` - Detect runtime from project
- `list_runtimes()` - List available runtimes
- `is_buildah_available()` - Check if buildah is installed
- `buildah_install_instructions()` - Get install instructions
- `create_service_spec(name, image, port=None)` - Create a service spec

## Requirements

- Python 3.8+
- For image building: buildah installed on the system
- For container runtime: youki or similar OCI runtime

## License

Apache-2.0
