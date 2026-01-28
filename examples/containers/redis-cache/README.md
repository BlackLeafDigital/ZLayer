# Redis Cache

A Redis deployment optimized for caching with optional persistence.

## Quick Start

```bash
# Deploy cache-only Redis
zlayer deploy -f deployment.yaml

# Check status
zlayer status redis-cache
```

## Connection Details

| Setting | Value                        |
|---------|------------------------------|
| Host    | `cache.redis-cache.service`  |
| Port    | `6379`                       |

### Connection String

```
redis://cache.redis-cache.service:6379
```

## Modes

### Cache-Only (Default)

The default configuration runs Redis as a pure in-memory cache:
- No persistence (data lost on restart)
- LRU eviction when memory limit reached
- Fastest performance

Best for: Session storage, API caching, rate limiting

### Persistent Mode

For data durability, uncomment the persistent configuration in `deployment.yaml`:
- AOF (Append-Only File) for durability
- RDB snapshots as backup
- Data survives restarts

Best for: Queues, leaderboards, persistent counters

## Memory Management

The default configuration uses:
- **Max memory**: 200MB
- **Eviction policy**: `allkeys-lru` (Least Recently Used)

Other eviction policies:
| Policy          | Description                          |
|-----------------|--------------------------------------|
| `noeviction`    | Return errors when memory limit hit  |
| `allkeys-lru`   | Evict any key using LRU              |
| `volatile-lru`  | Evict keys with TTL using LRU        |
| `allkeys-lfu`   | Evict any key using LFU              |
| `volatile-ttl`  | Evict keys with shortest TTL         |
| `allkeys-random`| Evict random keys                    |

Change policy in `deployment.yaml`:

```yaml
command:
  - redis-server
  - --maxmemory-policy
  - volatile-lfu  # Your preferred policy
```

## Security

### Password Protection

1. Set password via ZLayer secrets:
   ```bash
   zlayer secret create REDIS_PASSWORD "your-secure-password"
   ```

2. Update `deployment.yaml`:
   ```yaml
   env:
     REDIS_PASSWORD: $S:REDIS_PASSWORD
   command:
     - redis-server
     - --requirepass
     - $(REDIS_PASSWORD)
   ```

3. Connect with password:
   ```
   redis://default:${REDIS_PASSWORD}@cache.redis-cache.service:6379
   ```

### Disable Dangerous Commands

In `redis.conf`:

```
rename-command FLUSHALL ""
rename-command FLUSHDB ""
rename-command CONFIG ""
rename-command DEBUG ""
```

## Scaling

### Vertical Scaling

Increase memory in `deployment.yaml`:

```yaml
resources:
  memory: 1Gi

command:
  - redis-server
  - --maxmemory
  - 900mb  # Leave headroom for Redis overhead
```

### Horizontal Scaling (Read Replicas)

For read-heavy workloads:

```yaml
services:
  primary:
    # ... primary config

  replica:
    rtype: service
    image:
      name: redis:7-alpine
    command:
      - redis-server
      - --replicaof
      - primary.service
      - "6379"
    scale:
      replicas: 2
```

### Redis Cluster

For large-scale deployments, use Redis Cluster mode with multiple shards.

## Monitoring

### Check Health

```bash
zlayer logs redis-cache cache
zlayer exec redis-cache cache -- redis-cli ping
```

### Memory Stats

```bash
zlayer exec redis-cache cache -- redis-cli info memory
```

### Key Statistics

```bash
zlayer exec redis-cache cache -- redis-cli info keyspace
```

### Slow Log

```bash
zlayer exec redis-cache cache -- redis-cli slowlog get 10
```

## Integration Examples

### Python (redis-py)

```python
import redis

r = redis.Redis(
    host='cache.redis-cache.service',
    port=6379,
    decode_responses=True
)

# Set with expiration
r.setex('key', 3600, 'value')

# Get
value = r.get('key')
```

### Node.js (ioredis)

```javascript
const Redis = require('ioredis');

const redis = new Redis({
  host: 'cache.redis-cache.service',
  port: 6379
});

await redis.set('key', 'value', 'EX', 3600);
const value = await redis.get('key');
```

### Rust (redis-rs)

```rust
use redis::AsyncCommands;

let client = redis::Client::open("redis://cache.redis-cache.service:6379")?;
let mut conn = client.get_async_connection().await?;

conn.set_ex("key", "value", 3600).await?;
let value: String = conn.get("key").await?;
```

## Best Practices

1. **Set TTL on keys**: Prevent unbounded memory growth
2. **Use appropriate data structures**: Hashes for objects, Sorted Sets for rankings
3. **Monitor memory**: Set up alerts for memory usage
4. **Plan for failure**: Cache should be recoverable, not critical path
5. **Use connection pooling**: Avoid connection overhead
