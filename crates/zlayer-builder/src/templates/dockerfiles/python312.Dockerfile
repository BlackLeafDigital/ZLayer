# Multi-stage build for Python 3.12
# Optimized for production with minimal image size
# Uses cache mounts for faster rebuilds

FROM python:3.12-slim AS builder
WORKDIR /app

# Install build dependencies if needed
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    && rm -rf /var/lib/apt/lists/*

# Install Python dependencies with pip cache mount
COPY requirements.txt ./
RUN --mount=type=cache,target=/root/.cache/pip \
    pip install --user -r requirements.txt

FROM python:3.12-slim
WORKDIR /app

# Copy installed packages from builder
COPY --from=builder /root/.local /root/.local

# Ensure scripts in .local are usable
ENV PATH=/root/.local/bin:$PATH

# Copy application source
COPY . .

# Default command - override with your entry point
CMD ["python", "main.py"]
