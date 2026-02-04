# ZImagefile vs Dockerfile: Node.js

This example shows the same multi-stage Node.js build expressed in both
ZImagefile (YAML) and Dockerfile syntax.

## Files

| File        | Format     | Description                            |
|-------------|------------|----------------------------------------|
| ZImagefile  | YAML       | ZLayer's native image build format     |
| Dockerfile  | Dockerfile | Traditional Docker build format        |

## What It Demonstrates

- **Multi-stage builds** -- a `builder` stage compiles TypeScript, and a
  `runtime` stage contains only the production output.
- **Cache mounts** -- the npm cache directory is shared across builds for
  faster dependency installation.
- **Cross-stage COPY** -- `from: builder` in ZImagefile maps to
  `COPY --from=builder` in Dockerfile.
- **Healthcheck** -- both formats configure a health check on `/health`.

## Build

```bash
# Using ZImagefile (auto-detected when present)
zlayer build -t my-node-app:latest .

# Using Dockerfile explicitly
zlayer build -f Dockerfile -t my-node-app:latest .

# Using Docker directly
docker build -t my-node-app:latest .
```
