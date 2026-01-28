# PostgreSQL Database

A PostgreSQL deployment with persistent storage and proper configuration.

## Quick Start

```bash
# Set the database password
export POSTGRES_PASSWORD="your-secure-password-here"

# Deploy
zlayer deploy -f deployment.yaml

# Check status
zlayer status postgres-db
```

## Connection Details

| Setting  | Value                          |
|----------|--------------------------------|
| Host     | `database.postgres-db.service` |
| Port     | `5432`                         |
| Database | `appdb`                        |
| User     | `appuser`                      |
| Password | (from POSTGRES_PASSWORD env)   |

### Connection String

```
postgresql://appuser:${POSTGRES_PASSWORD}@database.postgres-db.service:5432/appdb
```

## Initialization Scripts

To run SQL scripts on first startup:

1. Create an `init/` directory:
   ```bash
   mkdir init
   cp init.sql init/01-schema.sql
   ```

2. Uncomment the init-scripts volume in `deployment.yaml`

3. Scripts run in alphabetical order (use numeric prefixes)

## Persistent Storage

Data is stored in a persistent volume at `/var/lib/postgresql/data`. This survives container restarts and redeployments.

To change storage size, modify in `deployment.yaml`:

```yaml
volumes:
  - name: pgdata
    path: /var/lib/postgresql/data
    size: 50Gi  # Increase as needed
    persist: true
```

## Configuration

### Performance Tuning

Add PostgreSQL configuration via environment variables:

```yaml
env:
  # Memory settings (adjust based on resources.memory)
  POSTGRES_SHARED_BUFFERS: 128MB
  POSTGRES_EFFECTIVE_CACHE_SIZE: 384MB
  POSTGRES_WORK_MEM: 4MB
  POSTGRES_MAINTENANCE_WORK_MEM: 64MB
```

Or mount a custom `postgresql.conf`:

```yaml
volumes:
  - name: pg-config
    path: /etc/postgresql/postgresql.conf
    source: ./postgresql.conf
    read_only: true
```

### High Availability

For production HA, consider:

1. **Streaming Replication**: Run primary + read replicas
2. **Patroni**: Automated failover with consensus
3. **Managed Service**: Use a managed PostgreSQL service

Basic replication example:

```yaml
services:
  primary:
    # ... primary config
    env:
      POSTGRES_REPLICATION_USER: replicator
      POSTGRES_REPLICATION_PASSWORD: $S:REPLICATION_PASSWORD

  replica:
    # ... replica config
    env:
      POSTGRES_PRIMARY_HOST: primary.service
```

## Backup

### Manual Backup

```bash
# Get shell access
zlayer exec postgres-db database -- bash

# Run pg_dump
pg_dump -U appuser appdb > /tmp/backup.sql

# Copy backup out
zlayer cp postgres-db:database:/tmp/backup.sql ./backup.sql
```

### Automated Backup (Cron Job)

Add to `deployment.yaml`:

```yaml
backup:
  rtype: cron
  image:
    name: postgres:16-alpine
  schedule: "0 2 * * *"  # Daily at 2 AM
  env:
    PGHOST: database.service
    PGUSER: appuser
    PGPASSWORD: $S:POSTGRES_PASSWORD
    PGDATABASE: appdb
  command:
    - sh
    - -c
    - "pg_dump | gzip > /backups/$(date +%Y%m%d).sql.gz"
  volumes:
    - name: backups
      path: /backups
      size: 50Gi
      persist: true
```

## Monitoring

### Check Health

```bash
zlayer logs postgres-db database
zlayer exec postgres-db database -- pg_isready -U appuser -d appdb
```

### Connection Stats

```bash
zlayer exec postgres-db database -- psql -U appuser -d appdb -c "SELECT * FROM pg_stat_activity;"
```

## Security Notes

1. **Passwords**: Use ZLayer secrets (`$S:SECRET_NAME`) instead of environment variables in production

2. **Network**: The endpoint is `internal` only - not accessible from outside the deployment

3. **Encryption**: Enable SSL for connections in production:
   ```yaml
   env:
     POSTGRES_SSL: "on"
   ```

4. **Access Control**: Customize `pg_hba.conf` for fine-grained access control
