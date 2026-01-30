# Multi-Service Application Stack

A complete application stack demonstrating a production-ready deployment with multiple interconnected services.

## Architecture

```
                    +-----------------+
                    |    Internet     |
                    +--------+--------+
                             |
                    +--------v--------+
                    |   web (nginx)   |
                    |   :443/:80      |
                    +--------+--------+
                             |
              +--------------+--------------+
              |                             |
     +--------v--------+           +--------v--------+
     |   api (app)     |           |  static files   |
     |   :8080         |           |  /usr/share/... |
     +--------+--------+           +-----------------+
              |
     +--------+--------+--------------+
     |                 |              |
+----v----+      +-----v-----+   +----v----+
| database|      |   cache   |   | worker  |
| :5432   |      |   :6379   |   | (jobs)  |
+---------+      +-----------+   +---------+
```

## Services

| Service  | Type    | Port | Purpose                    |
|----------|---------|------|----------------------------|
| web      | service | 443  | Frontend + reverse proxy   |
| api      | service | 8080 | Backend API                |
| database | service | 5432 | PostgreSQL database        |
| cache    | service | 6379 | Redis cache                |
| worker   | job     | -    | Background job processor   |
| cleanup  | cron    | -    | Scheduled maintenance      |

## Quick Start

### 1. Set Environment Variables

```bash
export POSTGRES_PASSWORD="your-secure-db-password"
export API_SECRET_KEY="your-api-secret-key"
```

### 2. Create Static Content

```bash
mkdir -p static
echo '<!DOCTYPE html><html><body><h1>Hello ZLayer!</h1></body></html>' > static/index.html
```

### 3. Create Init Scripts (Optional)

```bash
mkdir -p init
# Add SQL initialization scripts
```

### 4. Deploy

```bash
zlayer deploy -f deployment.yaml
```

### 5. Check Status

```bash
zlayer status multi-service-app
```

## Service Communication

### Internal DNS

Services communicate using internal DNS names:
- `database.service:5432` - PostgreSQL
- `cache.service:6379` - Redis
- `api.service:8080` - API backend

### Connection Strings

**Database:**
```
postgresql://appuser:${POSTGRES_PASSWORD}@database.service:5432/appdb
```

**Cache:**
```
redis://cache.service:6379
```

## Configuration

### Scaling

Adjust scaling parameters in `deployment.yaml`:

```yaml
scale:
  mode: adaptive
  min: 2      # Minimum instances
  max: 20     # Maximum instances
  cooldown: 30s
  targets:
    cpu: 70   # Scale up when CPU > 70%
    rps: 500  # Scale up when RPS > 500
```

### Resources

Modify resource limits per service:

```yaml
resources:
  cpu: 2.0      # CPU cores
  memory: 1Gi   # Memory limit
```

### Environment Variables

Add or modify environment variables:

```yaml
env:
  MY_VAR: "value"
  SECRET: $S:SECRET_NAME  # From ZLayer secrets
```

## Dependencies

The deployment defines service dependencies:

```
web -> api (healthy)
api -> database (healthy), cache (healthy)
worker -> database (healthy), cache (healthy)
cleanup -> (none, runs independently)
```

Services wait for dependencies before starting.

## Health Checks

Each service has health checks:

| Service  | Type | Endpoint/Command         |
|----------|------|--------------------------|
| web      | HTTP | localhost:80/health      |
| api      | HTTP | localhost:8080/health    |
| database | Exec | pg_isready -U appuser    |
| cache    | Exec | redis-cli ping           |

## Initialization

The API service runs initialization steps before accepting traffic:

1. **wait-db**: Wait for database to be reachable
2. **run-migrations**: Apply database migrations

## Volumes

Persistent storage:

| Service  | Volume   | Size | Purpose          |
|----------|----------|------|------------------|
| database | pgdata   | 20Gi | Database files   |

## Monitoring

### View Logs

```bash
# All services
zlayer logs multi-service-app

# Specific service
zlayer logs multi-service-app api
zlayer logs multi-service-app database
```

### Check Health

```bash
zlayer status multi-service-app
```

### Access Shell

```bash
zlayer exec multi-service-app api -- sh
zlayer exec multi-service-app database -- psql -U appuser -d appdb
```

## Backup

### Database Backup

```bash
zlayer exec multi-service-app database -- pg_dump -U appuser appdb > backup.sql
```

### Restore

```bash
zlayer exec multi-service-app database -- psql -U appuser appdb < backup.sql
```

## Updating

### Rolling Update

```bash
# Update API image
zlayer set-image multi-service-app api ghcr.io/myorg/api:v2.0.0

# Or redeploy with updated manifest
zlayer deploy -f deployment.yaml
```

### Rollback

```bash
zlayer rollback multi-service-app
```

## Development vs Production

This example is configured for production. For development:

1. Remove SSL redirect in nginx.conf
2. Set `APP_ENV: development`
3. Reduce replica counts
4. Use local volumes instead of persistent storage

## Troubleshooting

### Service Won't Start

Check dependencies:
```bash
zlayer status multi-service-app
zlayer logs multi-service-app api --since 5m
```

### Database Connection Failed

Verify credentials and connectivity:
```bash
zlayer exec multi-service-app api -- nc -zv database.service 5432
```

### Cache Connection Failed

Check Redis status:
```bash
zlayer exec multi-service-app cache -- redis-cli ping
```

### Migrations Failed

Check migration logs:
```bash
zlayer logs multi-service-app api --init
```
