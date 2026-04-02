//! Integration test: parse examples/docker-compose.yaml, convert to `DeploymentSpec`, verify.

use std::path::PathBuf;
use std::time::Duration;

use zlayer_docker::compose::{compose_to_deployment, parse_compose};
use zlayer_spec::{
    DependencyCondition, DeploymentSpec, HealthCheck, Protocol, ScaleSpec, StorageSpec,
};

/// Find the workspace root by walking up from `CARGO_MANIFEST_DIR`.
fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // zlayer-docker lives at <root>/crates/zlayer-docker
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("could not find workspace root")
        .to_path_buf()
}

fn load_and_convert() -> DeploymentSpec {
    let root = workspace_root();
    let compose_path = root.join("examples/docker-compose.yaml");
    let yaml = std::fs::read_to_string(&compose_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", compose_path.display()));

    let compose = parse_compose(&yaml).expect("failed to parse compose YAML");
    assert_eq!(compose.services.len(), 4, "expected 4 services");
    assert!(compose.services.contains_key("nginx"));
    assert!(compose.services.contains_key("api"));
    assert!(compose.services.contains_key("db"));
    assert!(compose.services.contains_key("redis"));
    assert_eq!(compose.volumes.len(), 2, "expected 2 top-level volumes");

    compose_to_deployment(&compose, "example-webapp")
        .expect("failed to convert compose to deployment spec")
}

#[test]
fn roundtrip_top_level() {
    let spec = load_and_convert();
    assert_eq!(spec.version, "v1");
    assert_eq!(spec.deployment, "example-webapp");
    assert_eq!(spec.services.len(), 4);
}

#[test]
fn roundtrip_nginx() {
    let spec = load_and_convert();
    let nginx = &spec.services["nginx"];
    assert_eq!(nginx.image.name, "nginx:alpine");
    assert_eq!(nginx.endpoints.len(), 2, "nginx should have 2 ports");

    let port80 = nginx.endpoints.iter().find(|e| e.port == 80).unwrap();
    assert_eq!(port80.target_port, Some(80));
    assert_eq!(port80.protocol, Protocol::Http);

    let port443 = nginx.endpoints.iter().find(|e| e.port == 443).unwrap();
    assert_eq!(port443.target_port, Some(443));

    assert_eq!(nginx.storage.len(), 1);
    match &nginx.storage[0] {
        StorageSpec::Bind {
            source,
            target,
            readonly,
        } => {
            assert_eq!(source, "./nginx.conf");
            assert_eq!(target, "/etc/nginx/nginx.conf");
            assert!(readonly, "nginx config should be readonly");
        }
        other => panic!("expected Bind for nginx volume, got: {other:?}"),
    }
    assert_eq!(nginx.depends.len(), 1);
    assert_eq!(nginx.depends[0].service, "api");
    assert_eq!(nginx.depends[0].condition, DependencyCondition::Healthy);
}

#[test]
fn roundtrip_api() {
    let spec = load_and_convert();
    let api = &spec.services["api"];
    assert_eq!(api.image.name, "api:latest");
    assert_eq!(api.endpoints.len(), 1);
    assert_eq!(api.endpoints[0].port, 3000);
    assert_eq!(api.endpoints[0].target_port, Some(3000));
    assert_eq!(api.endpoints[0].protocol, Protocol::Http);
    assert_eq!(
        api.env.get("DATABASE_URL").unwrap(),
        "postgres://user:pass@db:5432/myapp"
    );
    assert_eq!(api.env.get("REDIS_URL").unwrap(), "redis://redis:6379");

    assert_eq!(api.depends.len(), 2);
    let db_dep = api.depends.iter().find(|d| d.service == "db").unwrap();
    assert_eq!(db_dep.condition, DependencyCondition::Healthy);
    let redis_dep = api.depends.iter().find(|d| d.service == "redis").unwrap();
    assert_eq!(redis_dep.condition, DependencyCondition::Started);

    match &api.health.check {
        HealthCheck::Command { command } => {
            assert_eq!(command, "curl -f http://localhost:3000/health");
        }
        other => panic!("expected Command health check for api, got: {other:?}"),
    }
    assert_eq!(api.health.retries, 3);
    assert_eq!(api.health.interval, Some(Duration::from_secs(30)));
    assert_eq!(api.health.timeout, Some(Duration::from_secs(10)));
    assert_eq!(api.health.start_grace, Some(Duration::from_secs(10)));

    assert_eq!(api.scale, ScaleSpec::Fixed { replicas: 2 });
    assert_eq!(api.resources.cpu, Some(1.0));
    assert_eq!(api.resources.memory, Some("512M".to_string()));
}

#[test]
fn roundtrip_db() {
    let spec = load_and_convert();
    let db = &spec.services["db"];
    assert_eq!(db.image.name, "postgres:16-alpine");
    assert_eq!(db.env.get("POSTGRES_USER").unwrap(), "user");
    assert_eq!(db.env.get("POSTGRES_PASSWORD").unwrap(), "pass");
    assert_eq!(db.env.get("POSTGRES_DB").unwrap(), "myapp");
    assert_eq!(db.endpoints.len(), 1);
    assert_eq!(db.endpoints[0].port, 5432);
    assert_eq!(db.endpoints[0].protocol, Protocol::Tcp);
    assert_eq!(db.storage.len(), 1);
    match &db.storage[0] {
        StorageSpec::Named { name, target, .. } => {
            assert_eq!(name, "pgdata");
            assert_eq!(target, "/var/lib/postgresql/data");
        }
        other => panic!("expected Named volume for db, got: {other:?}"),
    }
    match &db.health.check {
        HealthCheck::Command { command } => {
            assert_eq!(command, "pg_isready -U user");
        }
        other => panic!("expected Command health check for db, got: {other:?}"),
    }
    assert_eq!(db.health.retries, 5);
    assert_eq!(db.health.interval, Some(Duration::from_secs(10)));
    assert_eq!(db.health.timeout, Some(Duration::from_secs(5)));
}

#[test]
fn roundtrip_redis() {
    let spec = load_and_convert();
    let redis = &spec.services["redis"];
    assert_eq!(redis.image.name, "redis:7-alpine");
    assert_eq!(redis.endpoints.len(), 1);
    assert_eq!(redis.endpoints[0].port, 6379);
    assert_eq!(redis.endpoints[0].protocol, Protocol::Tcp);
    assert_eq!(redis.storage.len(), 1);
    match &redis.storage[0] {
        StorageSpec::Named { name, target, .. } => {
            assert_eq!(name, "redis-data");
            assert_eq!(target, "/data");
        }
        other => panic!("expected Named volume for redis, got: {other:?}"),
    }
}
