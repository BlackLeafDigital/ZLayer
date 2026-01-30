# Nginx Static Content Server

A simple nginx deployment for serving static files.

## Quick Start

```bash
# Deploy with default nginx welcome page
zlayer deploy -f deployment.yaml

# Check status
zlayer status nginx-static
```

## Customization

### Serving Your Own Content

1. Create an `html/` directory with your static files:
   ```bash
   mkdir html
   echo "<h1>Hello ZLayer!</h1>" > html/index.html
   ```

2. Uncomment the volumes section in `deployment.yaml`:
   ```yaml
   volumes:
     - name: static-content
       path: /usr/share/nginx/html
       source: ./html
   ```

3. Deploy:
   ```bash
   zlayer deploy -f deployment.yaml
   ```

### Custom Nginx Configuration

1. Mount the provided `nginx.conf`:
   ```yaml
   volumes:
     - name: nginx-config
       path: /etc/nginx/nginx.conf
       source: ./nginx.conf
       read_only: true
   ```

2. Modify `nginx.conf` for your needs.

## Features

- **Alpine-based**: Minimal image size (~40MB)
- **Health checks**: HTTP health endpoint at `/health`
- **Gzip compression**: Enabled for text assets
- **Security headers**: XSS, frame, and content-type protection
- **Static asset caching**: 30-day cache for images, CSS, JS

## Scaling

Adjust replicas in `deployment.yaml`:

```yaml
scale:
  mode: fixed
  replicas: 4  # Increase for more capacity
```

Or use adaptive scaling:

```yaml
scale:
  mode: adaptive
  min: 2
  max: 10
  targets:
    cpu: 70
    rps: 1000
```

## Endpoints

| Path    | Description              |
|---------|--------------------------|
| `/`     | Static content root      |
| `/health` | Health check endpoint  |

## Resource Requirements

- CPU: 0.25 cores (minimal for static serving)
- Memory: 64Mi (alpine nginx is very lightweight)

Adjust in `deployment.yaml` if serving large files or high traffic.
