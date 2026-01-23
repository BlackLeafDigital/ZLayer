# ZLayer V1 Final TODO

**Last Updated**: 2026-01-23
**Current State**: Phases 1-5 implemented, integration and production hardening needed

---

## Summary

ZLayer has made significant progress with most core components implemented. The main gaps are:
1. **Container command handling** - OCI spec building doesn't extract CMD/ENTRYPOINT from images
2. **FIFO handling** - Current workaround spawns orphaned tasks
3. **Integration gaps** - Components exist but aren't fully wired together in deployment flow
4. **Production hardening** - Join flow, graceful shutdown, error recovery

---

## Critical Path (Must Fix for V1)

### 1. Containerd Runtime Fixes

#### 1.1 Image CMD/ENTRYPOINT Extraction
- [ ] **Extract image config CMD/ENTRYPOINT** in `crates/agent/src/containerd_runtime.rs`
  - Current: `build_oci_spec()` creates process without args (line 201-210)
  - Issue: We already read the image config in `get_image_chain_id()` to get diff_ids
  - Fix: Parse `config.Config.Cmd` and `config.Config.Entrypoint` from image config JSON
  - Store extracted command when pulling/inspecting image
  - Use in `build_oci_spec()` when building ProcessBuilder

```rust
// In get_image_chain_id(), also extract:
// config["config"]["Entrypoint"] -> Vec<String>
// config["config"]["Cmd"] -> Vec<String>
// Return (chain_id, entrypoint, cmd)
```

#### 1.2 Add Command Override to ServiceSpec
- [ ] **Add optional `command` field** to `ServiceSpec` in `crates/spec/src/types.rs`
```yaml
services:
  api:
    image:
      name: alpine:latest
    command: ["sleep", "infinity"]  # Optional override
```
- [ ] Update `build_oci_spec()` to prefer ServiceSpec.command, fallback to image CMD/ENTRYPOINT

#### 1.3 Fix FIFO Handling in start_container
- [ ] **Clean up FIFO reader tasks** in `crates/agent/src/containerd_runtime.rs`
  - Current: Lines 792-821 spawn infinite loops that are never cleaned up
  - Issue: Spawned tasks run forever with `loop { sleep(60s) }`
  - Fix: Store JoinHandles in ContainerInfo, cancel on stop_container
  - Alternative: Use proper async FIFO handling with select! for cancellation

```rust
// Store handles in ContainerInfo
struct ContainerInfo {
    // ... existing fields
    stdout_task: Option<JoinHandle<()>>,
    stderr_task: Option<JoinHandle<()>>,
}

// In stop_container, abort the handles
if let Some(handle) = info.stdout_task {
    handle.abort();
}
```

#### 1.4 Fix E2E Tests
- [ ] **test_container_lifecycle** - Needs proper command or image with CMD
  - Alpine image has no default CMD, exits immediately
  - Fix: Use `nginx:alpine` or add command override to spec
- [ ] **test_health_checks_tcp** - Timing/state issues
  - Container may exit before health check runs
  - Fix: Use long-running container, proper state waiting

---

## Spec Implementation Gaps

### 2. ServiceSpec Extensions

#### 2.1 Missing Fields
- [ ] Add `command: Vec<String>` - Optional command override
- [ ] Add `args: Vec<String>` - Optional args override (separate from command)
- [ ] Add `working_dir: Option<String>` - Working directory
- [ ] Add `user: Option<String>` - Run as user (e.g., "1000:1000")

#### 2.2 Cron Schedule Field (for rtype: cron)
- [ ] Add `schedule: String` field to ServiceSpec (cron expression)
- [ ] Only required when `rtype: cron`
- [ ] Validate cron syntax

```yaml
services:
  cleanup:
    rtype: cron
    schedule: "0 0 * * *"  # Daily at midnight
    image:
      name: cleanup:latest
```

---

## Integration Work

### 3. Deployment Flow Integration

#### 3.1 CLI Deploy Command (bin/runtime)
- [ ] **Wire up ServiceManager** in deploy command (line 278-320)
  - Currently prints deployment plan but doesn't start containers
  - Create ServiceManager with runtime
  - Scale each service to initial replica count
  - Wait for health checks

#### 3.2 CLI Join Command
- [ ] **Complete join flow** (line 417-489)
  - Fetch spec from API endpoint (currently placeholder)
  - Start service with ServiceManager
  - Register with overlay network
  - Register with load balancer

#### 3.3 CLI Stop Command
- [ ] **Implement stop logic** (line 650-676)
  - Get running services from ServiceManager
  - Scale to 0 (graceful) or force kill
  - Clean up overlay registrations

#### 3.4 CLI Logs Command
- [ ] **Implement log streaming** (line 614-647)
  - Connect to containerd
  - Stream logs from container FIFOs
  - Merge multiple replica logs

---

## Networking Integration

### 4. Overlay Network Wiring

#### 4.1 Service-to-Overlay Integration
- [ ] **Connect agent to overlay** on service start
  - Create WireGuard interface for deployment
  - Register container IP in service overlay
  - Register in global overlay if enabled
  - Update DNS records

#### 4.2 Network Namespace Integration
- [ ] **Configure container network namespace**
  - Currently: Network namespace not set in OCI spec (line 220-238)
  - Add network namespace to container
  - Connect to WireGuard interface
  - Set up routing

### 5. Proxy Integration

#### 5.1 Proxy-Agent Wiring
- [ ] **ProxyManager integration** with ServiceManager
  - Auto-register routes when service starts
  - Add backends when containers start
  - Update health status from HealthChecker
  - Remove backends when containers stop

#### 5.2 Endpoint Routing
- [ ] **Route registration** from ServiceSpec endpoints
  - Map `endpoints` to proxy routes
  - Handle `expose: public` vs `internal`
  - Support path-based routing

---

## Scheduler Integration

### 6. Autoscaling Wiring

#### 6.1 Metrics Collection
- [ ] **Connect ContainerdMetricsSource** to real containerd
  - Currently: MockMetricsSource used in tests
  - Wire up actual cgroup metrics from containerd
  - Collect CPU, memory, and optionally RPS

#### 6.2 Scaling Actions
- [ ] **Execute scaling decisions**
  - Connect Scheduler to ServiceManager
  - When ScaleUp: call scale_to(current + delta)
  - When ScaleDown: call scale_to(current - delta)

#### 6.3 Multi-Node Coordination
- [ ] **Raft cluster setup** for distributed scheduling
  - Bootstrap cluster on first node
  - Join flow for additional nodes
  - Leader election for scaling decisions

---

## Missing V1 Spec Features

### 7. Job Semantics (rtype: job)

#### 7.1 Job Execution Model
- [ ] **Implement job trigger semantics**
  - Run-to-completion containers
  - Endpoint triggers job execution
  - Return logs/exit code to caller
  - No auto-restart on completion

#### 7.2 Job API Endpoint
- [ ] **Add job trigger API**
  - POST /deployments/{d}/services/{s}/trigger
  - Start container, wait for completion
  - Return result

### 8. Cron Semantics (rtype: cron)

#### 8.1 Cron Scheduler
- [ ] **Implement cron scheduling**
  - Parse cron expression from spec
  - Use `cron` or `tokio-cron-scheduler` crate
  - Trigger container execution on schedule

### 9. Dependency Resolution

#### 9.1 Startup Ordering
- [ ] **Implement depends resolution**
  - Build dependency graph
  - Topological sort for startup order
  - Wait for dependency conditions

#### 9.2 Condition Checks
- [ ] **Implement condition types**
  - `started`: Container process exists
  - `healthy`: Health check passes
  - `ready`: Proxy considers it ready

### 10. Environment Variable Resolution

#### 10.1 $E: Prefix Handling
- [ ] **Resolve $E: prefixed values**
  - Parse env vars with `$E:` prefix
  - Resolve from runtime environment at start time
  - Error if required env var missing

```rust
// In service.rs before container create
for (key, value) in &spec.env {
    if let Some(env_name) = value.strip_prefix("$E:") {
        let resolved = std::env::var(env_name)?;
        resolved_env.insert(key.clone(), resolved);
    }
}
```

---

## Production Hardening

### 11. Graceful Shutdown

#### 11.1 Signal Handling
- [ ] **SIGTERM/SIGINT handling** in runtime
  - Currently: Basic handler exists
  - Need: Drain connections, stop containers gracefully

#### 11.2 Container Cleanup
- [ ] **Stop all containers on shutdown**
  - Track running containers globally
  - Stop with timeout, then SIGKILL
  - Clean up FIFOs and state directories

### 12. Error Recovery

#### 12.1 Container Restart Policies
- [ ] **Implement errors.on_panic policies**
  - `restart`: Auto-restart on failure
  - `shutdown`: Stop the service
  - `isolate`: Remove from LB, keep running
  - Implement exponential backoff for `backoff`

#### 12.2 Init Failure Handling
- [ ] **Implement errors.on_init_failure policies**
  - Currently: Init orchestrator exists
  - Wire up failure actions (fail/restart/backoff)

### 13. Security

#### 13.1 JWT Secret
- [ ] **Require JWT_SECRET in production**
  - Warn on default secret
  - Validate minimum entropy
  - Support key rotation

#### 13.2 Input Validation
- [ ] **Audit all API inputs**
  - Validate deployment/service names
  - Validate image references
  - Sanitize user-provided data

### 14. Observability

#### 14.1 Metrics Exposition
- [ ] **Add Prometheus endpoint** to runtime
  - Container metrics
  - Proxy metrics
  - Scheduler metrics

#### 14.2 Structured Logging
- [ ] **Add context to all log messages**
  - deployment_name
  - service_name
  - container_id
  - replica_id

---

## Test Coverage

### 15. Integration Tests

#### 15.1 E2E Tests
- [ ] **Fix test_container_lifecycle**
- [ ] **Fix test_health_checks_tcp**
- [ ] All 11 E2E tests passing

#### 15.2 New Tests Needed
- [ ] Deployment flow end-to-end
- [ ] Join flow end-to-end
- [ ] Scaling up and down
- [ ] Health check failure recovery
- [ ] Dependency startup ordering

### 16. Registry Tests

#### 16.1 Fix Hanging Test
- [ ] **Fix test_cache_put_get** in `crates/registry/src/cache.rs`
  - Hangs indefinitely
  - Add timeout wrapper
  - Debug blocking I/O issue

---

## Documentation

### 17. User Documentation

- [ ] Quick start guide
- [ ] Spec file reference
- [ ] CLI command reference
- [ ] Architecture overview
- [ ] Deployment guide

### 18. API Documentation

- [ ] OpenAPI spec complete
- [ ] Auth flow documented
- [ ] Rate limits documented

---

## Technical Debt

### 19. Code Cleanup

- [ ] Fix clippy warnings (unused variables in init_actions, agent)
- [ ] Remove TODO comments with actual implementations
- [ ] Add doc comments to public APIs
- [ ] Consolidate error types

### 20. Build Optimization

- [ ] Feature flag audit
- [ ] Dependency audit (cargo-deny)
- [ ] Binary size optimization
- [ ] Build cache optimization for CI

---

## Prioritized Task Order

### Phase 1: Make Basic Deployment Work (Critical)
1. Extract image CMD/ENTRYPOINT
2. Fix FIFO handling
3. Wire up deploy command
4. Fix E2E tests

### Phase 2: Complete Core Features
5. Add command override to ServiceSpec
6. Implement $E: resolution
7. Implement dependency resolution
8. Wire up proxy integration

### Phase 3: Network & Scaling
9. Overlay network integration
10. Connect scheduler to ServiceManager
11. Implement container restart policies

### Phase 4: Advanced Features
12. Job/Cron semantics
13. Join flow completion
14. Multi-node coordination

### Phase 5: Production Ready
15. Graceful shutdown
16. Security audit
17. Documentation
18. Load testing

---

## Notes

- **Test with real containerd**: `sudo cargo test --package agent --test containerd_e2e -- --nocapture`
- **Skip registry tests**: `cargo test --workspace --exclude registry`
- **Build release**: `cargo build --release --package runtime`
