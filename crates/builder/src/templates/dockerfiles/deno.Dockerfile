# Deno runtime image
# Uses official Deno image with sensible defaults

FROM denoland/deno:latest
WORKDIR /app

# Cache dependencies (if using import map or deps.ts)
COPY deno.json* deno.lock* deps.ts* ./
RUN deno cache deps.ts 2>/dev/null || true

# Copy application source
COPY . .

# Cache the main module
RUN deno cache main.ts

# Run as non-root deno user
USER deno

# Default command with common permissions
# Adjust permissions as needed for your application
CMD ["run", "--allow-net", "--allow-read", "--allow-env", "main.ts"]
