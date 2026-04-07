//! Convert Docker Compose files to `ZLayer` deployment specs.

use std::collections::HashMap;
use std::time::Duration;

use tracing::{info, warn};
use zlayer_spec::{
    CommandSpec, DependencyCondition, DependsSpec, DeploymentSpec, EndpointSpec, ErrorsSpec,
    ExposeType, HealthCheck, HealthSpec, ImageSpec, InitSpec, NodeMode, Protocol, PullPolicy,
    ResourceType, ResourcesSpec, ScaleSpec, ServiceNetworkSpec, ServiceSpec, ServiceType,
    StorageSpec, StorageTier, TimeoutAction,
};

use super::types::{
    ComposeFile, ComposeService, DependsOnCondition, EnvFile, EnvFileEntry, VolumeType,
};
use crate::DockerError;

/// Convert a `ComposeFile` into a `ZLayer` deployment spec.
///
/// # Errors
///
/// Returns a conversion error if the compose file contains features
/// that cannot be mapped to a `ZLayer` deployment spec (e.g., port parsing
/// failures, invalid resource strings).
pub fn compose_to_deployment(compose: &ComposeFile, name: &str) -> crate::Result<DeploymentSpec> {
    let mut services = HashMap::new();

    // Warn about unsupported top-level features
    if !compose.secrets.is_empty() {
        warn!(
            "Docker Compose secrets are not directly supported; skipping {} top-level secret(s)",
            compose.secrets.len()
        );
    }
    if !compose.configs.is_empty() {
        warn!(
            "Docker Compose configs are not directly supported; skipping {} top-level config(s)",
            compose.configs.len()
        );
    }
    if !compose.networks.is_empty() {
        info!(
            "ZLayer handles networking automatically; {} top-level network(s) will be ignored",
            compose.networks.len()
        );
    }

    for (svc_name, svc) in &compose.services {
        let spec = convert_service(svc_name, svc)?;
        services.insert(svc_name.clone(), spec);
    }

    Ok(DeploymentSpec {
        version: "v1".to_string(),
        deployment: name.to_string(),
        services,
        tunnels: HashMap::new(),
        api: zlayer_spec::ApiSpec::default(),
    })
}

/// Convert a single `ComposeService` into a `ServiceSpec`.
#[allow(clippy::too_many_lines)]
fn convert_service(svc_name: &str, svc: &ComposeService) -> crate::Result<ServiceSpec> {
    let image = convert_image(svc_name, svc)?;
    let endpoints = convert_ports(&svc.ports)?;
    let storage = convert_volumes(&svc.volumes);
    let env = convert_environment(svc)?;
    let depends = convert_depends_on(&svc.depends_on);
    let health = convert_healthcheck(svc.healthcheck.as_ref());
    let command = convert_command(svc);
    let resources = convert_resources(svc);
    let scale = convert_scale(svc);

    warn_unsupported_fields(svc_name, svc);

    Ok(ServiceSpec {
        rtype: ResourceType::Service,
        schedule: None,
        image,
        resources,
        env,
        command,
        network: ServiceNetworkSpec::default(),
        endpoints,
        scale,
        depends,
        health,
        init: InitSpec::default(),
        errors: ErrorsSpec::default(),
        devices: Vec::new(),
        storage,
        capabilities: svc.cap_add.clone(),
        privileged: svc.privileged,
        node_mode: NodeMode::default(),
        node_selector: None,
        service_type: ServiceType::default(),
        wasm: None,
        logs: None,
        host_network: false,
    })
}

/// Emit warnings/info for compose fields that are not mapped to `ZLayer`.
#[allow(clippy::too_many_lines)]
fn warn_unsupported_fields(svc_name: &str, svc: &ComposeService) {
    if !svc.secrets.is_empty() {
        warn!(
            service = svc_name,
            "Docker Compose service secrets are not directly supported; skipping"
        );
    }
    if !svc.configs.is_empty() {
        warn!(
            service = svc_name,
            "Docker Compose service configs are not directly supported; skipping"
        );
    }
    if let Some(ref restart) = svc.restart {
        info!(
            service = svc_name,
            restart = %restart,
            "ZLayer handles restart policies differently; compose restart policy ignored"
        );
    }
    if svc.networks.is_some() {
        info!(
            service = svc_name,
            "ZLayer handles networking automatically; service-level networks ignored"
        );
    }
    if svc.network_mode.is_some() {
        info!(
            service = svc_name,
            "ZLayer handles networking automatically; network_mode ignored"
        );
    }
    if !svc.extra_hosts.is_empty() {
        warn!(
            service = svc_name,
            "extra_hosts is not supported in ZLayer; skipping"
        );
    }
    if svc.dns.is_some() {
        warn!(
            service = svc_name,
            "custom DNS is not supported in ZLayer; skipping"
        );
    }
    if !svc.sysctls.is_empty() {
        warn!(
            service = svc_name,
            "sysctls are not supported in ZLayer; skipping"
        );
    }
    if !svc.ulimits.is_empty() {
        warn!(
            service = svc_name,
            "ulimits are not supported in ZLayer; skipping"
        );
    }
    if svc.logging.is_some() {
        warn!(
            service = svc_name,
            "custom logging drivers are not supported; ZLayer uses built-in observability"
        );
    }
    if !svc.devices.is_empty() {
        warn!(
            service = svc_name,
            "device passthrough from compose is not mapped; use ZLayer device specs instead"
        );
    }
    if !svc.security_opt.is_empty() {
        warn!(
            service = svc_name,
            "security_opt is not supported in ZLayer; skipping"
        );
    }
    if svc.pid.is_some() {
        warn!(
            service = svc_name,
            "pid namespace sharing is not supported in ZLayer; skipping"
        );
    }
    if svc.ipc.is_some() {
        warn!(
            service = svc_name,
            "ipc namespace sharing is not supported in ZLayer; skipping"
        );
    }
    if !svc.cap_drop.is_empty() {
        warn!(
            service = svc_name,
            count = svc.cap_drop.len(),
            "cap_drop is not mapped; ZLayer uses explicit capability additions only"
        );
    }
    if svc.user.is_some() {
        warn!(
            service = svc_name,
            "user field is not mapped to ZLayer spec; skipping"
        );
    }
    if svc.stop_signal.is_some() {
        warn!(
            service = svc_name,
            "stop_signal is not supported in ZLayer; skipping"
        );
    }
    if svc.stop_grace_period.is_some() {
        warn!(
            service = svc_name,
            "stop_grace_period is not supported in ZLayer; skipping"
        );
    }
}

/// Convert the image field, handling build-only services.
fn convert_image(svc_name: &str, svc: &ComposeService) -> crate::Result<ImageSpec> {
    if let Some(ref image) = svc.image {
        Ok(ImageSpec {
            name: image.clone(),
            pull_policy: PullPolicy::IfNotPresent,
        })
    } else if svc.build.is_some() {
        warn!(
            service = svc_name,
            "service uses 'build' without 'image'; build is handled separately by zlayer-build. \
             Using placeholder image name '{svc_name}:latest'"
        );
        Ok(ImageSpec {
            name: format!("{svc_name}:latest"),
            pull_policy: PullPolicy::IfNotPresent,
        })
    } else {
        Err(DockerError::Conversion(format!(
            "service '{svc_name}' has neither 'image' nor 'build' specified"
        )))
    }
}

/// Convert compose port mappings to `EndpointSpec` entries.
fn convert_ports(ports: &[super::types::ComposePort]) -> crate::Result<Vec<EndpointSpec>> {
    let mut endpoints = Vec::with_capacity(ports.len());

    for port in ports {
        let container_port: u16 = parse_port_number(&port.container_port).map_err(|e| {
            DockerError::Conversion(format!(
                "invalid container port '{}': {e}",
                port.container_port
            ))
        })?;

        let host_port: u16 = match &port.host_port {
            Some(hp) => parse_port_number(hp)
                .map_err(|e| DockerError::Conversion(format!("invalid host port '{hp}': {e}")))?,
            None => container_port,
        };

        let protocol = match port.protocol.as_deref() {
            Some("udp") => Protocol::Udp,
            _ => infer_protocol(container_port),
        };

        endpoints.push(EndpointSpec {
            name: format!("port-{container_port}"),
            protocol,
            port: host_port,
            target_port: Some(container_port),
            path: None,
            host: None,
            expose: ExposeType::Public,
            stream: None,
            tunnel: None,
        });
    }

    Ok(endpoints)
}

/// Infer protocol from well-known port numbers.
fn infer_protocol(port: u16) -> Protocol {
    match port {
        80 | 443 | 8080 | 8443 | 3000 | 5000 | 8000 | 8888 => Protocol::Http,
        _ => Protocol::Tcp,
    }
}

/// Parse a port number string, handling range syntax by taking the first port.
fn parse_port_number(s: &str) -> Result<u16, String> {
    // Port ranges like "8080-8081" -- take the first port
    let port_str = s.split('-').next().unwrap_or(s);
    port_str
        .trim()
        .parse::<u16>()
        .map_err(|e| format!("'{port_str}' is not a valid port: {e}"))
}

/// Convert compose volume mounts to `StorageSpec` entries.
fn convert_volumes(volumes: &[super::types::ComposeVolume]) -> Vec<StorageSpec> {
    let mut storage = Vec::with_capacity(volumes.len());

    for vol in volumes {
        let Some(target) = vol.target.clone() else {
            warn!("volume has no target path; skipping");
            continue;
        };

        let vol_type = vol.volume_type.clone().unwrap_or_default();

        match vol_type {
            VolumeType::Bind => {
                let source = vol.source.clone().unwrap_or_default();
                storage.push(StorageSpec::Bind {
                    source,
                    target,
                    readonly: vol.read_only,
                });
            }
            VolumeType::Tmpfs => {
                let size = vol
                    .tmpfs
                    .as_ref()
                    .and_then(|t| t.size.as_ref())
                    .map(super::types::StringOrNumber::as_string);
                let mode = vol.tmpfs.as_ref().and_then(|t| t.mode);
                storage.push(StorageSpec::Tmpfs { target, size, mode });
            }
            VolumeType::Volume | VolumeType::Npipe | VolumeType::Cluster => {
                // Named or anonymous volumes
                match &vol.source {
                    Some(name) if !name.is_empty() => {
                        storage.push(StorageSpec::Named {
                            name: name.clone(),
                            target,
                            readonly: vol.read_only,
                            tier: StorageTier::default(),
                            size: None,
                        });
                    }
                    _ => {
                        // Anonymous volume
                        storage.push(StorageSpec::Anonymous {
                            target,
                            tier: StorageTier::default(),
                        });
                    }
                }
            }
        }
    }

    storage
}

/// Merge inline environment variables with `env_file` contents.
fn convert_environment(svc: &ComposeService) -> crate::Result<HashMap<String, String>> {
    let mut env = HashMap::new();

    // Load env_file entries first (lower priority, overridden by inline env)
    if let Some(ref env_file) = svc.env_file {
        let paths = collect_env_file_paths(env_file);
        for (path, required) in paths {
            match load_env_file(&path) {
                Ok(vars) => {
                    for (k, v) in vars {
                        env.insert(k, v);
                    }
                }
                Err(e) => {
                    if required {
                        return Err(DockerError::Conversion(format!(
                            "failed to read required env_file '{path}': {e}"
                        )));
                    }
                    warn!(
                        path = %path,
                        error = %e,
                        "env_file not found or unreadable; skipping"
                    );
                }
            }
        }
    }

    // Inline environment overrides env_file
    for (k, v) in &svc.environment {
        env.insert(k.clone(), v.clone());
    }

    Ok(env)
}

/// Collect all env file paths with their `required` flag.
fn collect_env_file_paths(env_file: &EnvFile) -> Vec<(String, bool)> {
    match env_file {
        EnvFile::Single(path) => vec![(path.clone(), true)],
        EnvFile::List(entries) => entries
            .iter()
            .map(|entry| match entry {
                EnvFileEntry::Path(path) => (path.clone(), true),
                EnvFileEntry::Config(cfg) => (cfg.path.clone(), cfg.required),
            })
            .collect(),
    }
}

/// Load a `.env`-style file and return key-value pairs.
fn load_env_file(path: &str) -> Result<Vec<(String, String)>, std::io::Error> {
    let content = std::fs::read_to_string(path)?;
    let mut vars = Vec::new();

    for line in content.lines() {
        let line = line.trim();

        // Skip empty lines and comments
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim().to_string();
            let value = value.trim().to_string();
            // Strip surrounding quotes if present
            let value = strip_quotes(&value);
            vars.push((key, value));
        }
    }

    Ok(vars)
}

/// Strip surrounding single or double quotes from a value.
fn strip_quotes(s: &str) -> String {
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        return s[1..s.len() - 1].to_string();
    }
    s.to_string()
}

/// Convert compose `depends_on` to `DependsSpec` entries.
fn convert_depends_on(
    depends: &HashMap<String, super::types::ComposeDependsOn>,
) -> Vec<DependsSpec> {
    depends
        .iter()
        .map(|(service_name, dep)| {
            let condition = match dep.condition {
                DependsOnCondition::ServiceStarted => DependencyCondition::Started,
                DependsOnCondition::ServiceHealthy => DependencyCondition::Healthy,
                DependsOnCondition::ServiceCompletedSuccessfully => DependencyCondition::Ready,
            };

            DependsSpec {
                service: service_name.clone(),
                condition,
                timeout: Some(Duration::from_secs(300)),
                on_timeout: TimeoutAction::Fail,
            }
        })
        .collect()
}

/// Convert compose healthcheck to `HealthSpec`.
fn convert_healthcheck(hc: Option<&super::types::ComposeHealthcheck>) -> HealthSpec {
    let Some(hc) = hc.filter(|h| !h.disable) else {
        // No healthcheck or disabled -- return default TCP check
        return HealthSpec {
            start_grace: Some(Duration::from_secs(5)),
            interval: None,
            timeout: None,
            retries: 3,
            check: HealthCheck::Tcp { port: 0 },
        };
    };

    let check = convert_health_test(&hc.test);
    let interval = hc.interval.as_deref().and_then(parse_compose_duration);
    let timeout = hc.timeout.as_deref().and_then(parse_compose_duration);
    let start_grace = hc.start_period.as_deref().and_then(parse_compose_duration);
    let retries = hc.retries.and_then(|r| u32::try_from(r).ok()).unwrap_or(3);

    HealthSpec {
        start_grace,
        interval,
        timeout,
        retries,
        check,
    }
}

/// Convert compose healthcheck test command to a `HealthCheck`.
fn convert_health_test(test: &[String]) -> HealthCheck {
    if test.is_empty() {
        return HealthCheck::Tcp { port: 0 };
    }

    // Docker compose test can be:
    // ["CMD", "curl", "-f", "http://localhost"]
    // ["CMD-SHELL", "curl -f http://localhost || exit 1"]
    // ["NONE"] -- disable
    let first = test[0].as_str();
    match first {
        "NONE" => HealthCheck::Tcp { port: 0 },
        "CMD" => {
            let command = test[1..].join(" ");
            HealthCheck::Command { command }
        }
        "CMD-SHELL" => {
            let command = if test.len() > 1 {
                test[1..].join(" ")
            } else {
                String::new()
            };
            HealthCheck::Command { command }
        }
        _ => {
            // Treat the entire test as a shell command
            let command = test.join(" ");
            HealthCheck::Command { command }
        }
    }
}

/// Convert compose command, entrypoint, and `working_dir` to `CommandSpec`.
fn convert_command(svc: &ComposeService) -> CommandSpec {
    let args = if svc.command.is_empty() {
        None
    } else {
        Some(svc.command.clone())
    };

    let entrypoint = if svc.entrypoint.is_empty() {
        None
    } else {
        Some(svc.entrypoint.clone())
    };

    CommandSpec {
        args,
        entrypoint,
        workdir: svc.working_dir.clone(),
    }
}

/// Convert compose deploy resources to `ResourcesSpec`.
fn convert_resources(svc: &ComposeService) -> ResourcesSpec {
    let Some(deploy) = &svc.deploy else {
        return ResourcesSpec::default();
    };
    let Some(resources) = &deploy.resources else {
        return ResourcesSpec::default();
    };
    let Some(limits) = &resources.limits else {
        return ResourcesSpec::default();
    };

    let cpu = limits
        .cpus
        .as_ref()
        .and_then(|c| c.as_string().parse::<f64>().ok());

    let memory = limits.memory.clone();

    ResourcesSpec {
        cpu,
        memory,
        gpu: None,
    }
}

/// Convert compose deploy replicas to `ScaleSpec`.
fn convert_scale(svc: &ComposeService) -> ScaleSpec {
    // Check deploy.replicas first
    if let Some(ref deploy) = svc.deploy {
        if let Some(replicas) = deploy.replicas {
            if let Ok(r) = u32::try_from(replicas) {
                return ScaleSpec::Fixed { replicas: r };
            }
        }
    }

    // Check top-level scale field
    if let Some(scale) = svc.scale {
        if let Ok(r) = u32::try_from(scale) {
            return ScaleSpec::Fixed { replicas: r };
        }
    }

    // Default: adaptive scaling
    ScaleSpec::default()
}

/// Parse Docker Compose duration strings like `"30s"`, `"1m30s"`, `"5m"`, `"1h"`.
///
/// Docker Compose supports Go-style durations: `ns`, `us`, `ms`, `s`, `m`, `h`.
/// Returns `None` if the string cannot be parsed.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn parse_compose_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    let mut total_nanos: u128 = 0;
    let mut current_num = String::new();

    for ch in s.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            current_num.push(ch);
        } else {
            let value: f64 = current_num.parse().ok()?;
            current_num.clear();

            let nanos = match ch {
                'h' => value * 3_600_000_000_000.0,
                'm' => value * 60_000_000_000.0,
                's' => value * 1_000_000_000.0,
                _ => return None,
            };

            total_nanos = total_nanos.checked_add(nanos as u128)?;
        }
    }

    // Handle bare number (no suffix) -- treat as seconds
    if !current_num.is_empty() {
        let value: f64 = current_num.parse().ok()?;
        total_nanos = total_nanos.checked_add((value * 1_000_000_000.0) as u128)?;
    }

    if total_nanos == 0 {
        return None;
    }

    Some(Duration::from_nanos(total_nanos as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::parse_compose;

    #[test]
    fn test_parse_compose_duration() {
        assert_eq!(parse_compose_duration("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_compose_duration("5m"), Some(Duration::from_secs(300)));
        assert_eq!(
            parse_compose_duration("1h"),
            Some(Duration::from_secs(3600))
        );
        assert_eq!(parse_compose_duration(""), None);
    }

    #[test]
    fn test_infer_protocol() {
        assert_eq!(infer_protocol(80), Protocol::Http);
        assert_eq!(infer_protocol(443), Protocol::Http);
        assert_eq!(infer_protocol(8080), Protocol::Http);
        assert_eq!(infer_protocol(5432), Protocol::Tcp);
        assert_eq!(infer_protocol(6379), Protocol::Tcp);
    }

    #[test]
    fn test_strip_quotes() {
        assert_eq!(strip_quotes("\"hello\""), "hello");
        assert_eq!(strip_quotes("'hello'"), "hello");
        assert_eq!(strip_quotes("hello"), "hello");
        assert_eq!(strip_quotes("\"\""), "");
    }

    #[test]
    fn test_parse_port_number() {
        assert_eq!(parse_port_number("8080"), Ok(8080));
        assert_eq!(parse_port_number("8080-8081"), Ok(8080));
        assert!(parse_port_number("not_a_port").is_err());
    }

    #[test]
    fn test_basic_compose_conversion() {
        let yaml = r"
services:
  web:
    image: nginx:latest
    ports:
      - '8080:80'
    environment:
      FOO: bar
  db:
    image: postgres:16
    ports:
      - '5432:5432'
    volumes:
      - pgdata:/var/lib/postgresql/data
    environment:
      POSTGRES_PASSWORD: secret

volumes:
  pgdata:
";
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "test-app").unwrap();

        assert_eq!(spec.version, "v1");
        assert_eq!(spec.deployment, "test-app");
        assert_eq!(spec.services.len(), 2);

        let web = &spec.services["web"];
        assert_eq!(web.image.name, "nginx:latest");
        assert_eq!(web.endpoints.len(), 1);
        assert_eq!(web.endpoints[0].port, 8080);
        assert_eq!(web.endpoints[0].target_port, Some(80));
        assert_eq!(web.endpoints[0].protocol, Protocol::Http);
        assert_eq!(web.env.get("FOO").unwrap(), "bar");

        let db = &spec.services["db"];
        assert_eq!(db.image.name, "postgres:16");
        assert_eq!(db.endpoints.len(), 1);
        assert_eq!(db.endpoints[0].port, 5432);
        assert_eq!(db.endpoints[0].protocol, Protocol::Tcp);
        assert_eq!(db.storage.len(), 1);
        match &db.storage[0] {
            StorageSpec::Named {
                name,
                target,
                readonly,
                ..
            } => {
                assert_eq!(name, "pgdata");
                assert_eq!(target, "/var/lib/postgresql/data");
                assert!(!readonly);
            }
            other => panic!("expected Named storage, got: {other:?}"),
        }
    }

    #[test]
    fn test_build_only_service() {
        let yaml = r"
services:
  app:
    build: .
";
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "build-test").unwrap();

        let app = &spec.services["app"];
        assert_eq!(app.image.name, "app:latest");
        assert_eq!(app.image.pull_policy, PullPolicy::IfNotPresent);
    }

    #[test]
    fn test_depends_on_conversion() {
        let yaml = r"
services:
  web:
    image: nginx:latest
    depends_on:
      db:
        condition: service_healthy
      cache:
        condition: service_started
  db:
    image: postgres:16
  cache:
    image: redis:7
";
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "deps-test").unwrap();

        let web = &spec.services["web"];
        assert_eq!(web.depends.len(), 2);

        let db_dep = web.depends.iter().find(|d| d.service == "db").unwrap();
        assert_eq!(db_dep.condition, DependencyCondition::Healthy);

        let cache_dep = web.depends.iter().find(|d| d.service == "cache").unwrap();
        assert_eq!(cache_dep.condition, DependencyCondition::Started);
    }

    #[test]
    fn test_deploy_resources_conversion() {
        let yaml = "
services:
  app:
    image: myapp:latest
    deploy:
      replicas: 3
      resources:
        limits:
          cpus: \"0.5\"
          memory: \"512M\"
";
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "resources-test").unwrap();

        let app = &spec.services["app"];
        assert_eq!(app.resources.cpu, Some(0.5));
        assert_eq!(app.resources.memory, Some("512M".to_string()));
        assert_eq!(app.scale, ScaleSpec::Fixed { replicas: 3 });
    }

    #[test]
    fn test_healthcheck_conversion() {
        let yaml = r"
services:
  web:
    image: nginx:latest
    healthcheck:
      test: ['CMD', 'curl', '-f', 'http://localhost']
      interval: 30s
      timeout: 10s
      retries: 5
      start_period: 5s
";
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "health-test").unwrap();

        let web = &spec.services["web"];
        assert_eq!(web.health.retries, 5);
        assert_eq!(web.health.interval, Some(Duration::from_secs(30)));
        assert_eq!(web.health.timeout, Some(Duration::from_secs(10)));
        assert_eq!(web.health.start_grace, Some(Duration::from_secs(5)));
        match &web.health.check {
            HealthCheck::Command { command } => {
                assert_eq!(command, "curl -f http://localhost");
            }
            other => panic!("expected Command health check, got: {other:?}"),
        }
    }

    #[test]
    fn test_volume_types() {
        let yaml = r"
services:
  app:
    image: myapp:latest
    volumes:
      - ./data:/app/data:ro
      - named_vol:/app/storage
      - type: tmpfs
        target: /app/tmp
";
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "vol-test").unwrap();

        let app = &spec.services["app"];
        assert_eq!(app.storage.len(), 3);

        match &app.storage[0] {
            StorageSpec::Bind {
                source,
                target,
                readonly,
            } => {
                assert_eq!(source, "./data");
                assert_eq!(target, "/app/data");
                assert!(readonly);
            }
            other => panic!("expected Bind, got: {other:?}"),
        }

        match &app.storage[1] {
            StorageSpec::Named {
                name,
                target,
                readonly,
                ..
            } => {
                assert_eq!(name, "named_vol");
                assert_eq!(target, "/app/storage");
                assert!(!readonly);
            }
            other => panic!("expected Named, got: {other:?}"),
        }

        match &app.storage[2] {
            StorageSpec::Tmpfs { target, .. } => {
                assert_eq!(target, "/app/tmp");
            }
            other => panic!("expected Tmpfs, got: {other:?}"),
        }
    }

    #[test]
    fn test_command_and_entrypoint() {
        let yaml = r"
services:
  app:
    image: myapp:latest
    entrypoint: ['/bin/sh', '-c']
    command: ['echo', 'hello']
    working_dir: /app
";
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "cmd-test").unwrap();

        let app = &spec.services["app"];
        assert_eq!(
            app.command.entrypoint,
            Some(vec!["/bin/sh".to_string(), "-c".to_string()])
        );
        assert_eq!(
            app.command.args,
            Some(vec!["echo".to_string(), "hello".to_string()])
        );
        assert_eq!(app.command.workdir, Some("/app".to_string()));
    }

    #[test]
    fn test_privileged_and_capabilities() {
        let yaml = r"
services:
  sys:
    image: myapp:latest
    privileged: true
    cap_add:
      - SYS_ADMIN
      - NET_ADMIN
";
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "priv-test").unwrap();

        let sys = &spec.services["sys"];
        assert!(sys.privileged);
        assert_eq!(
            sys.capabilities,
            vec!["SYS_ADMIN".to_string(), "NET_ADMIN".to_string()]
        );
    }

    #[test]
    fn test_no_image_no_build_fails() {
        let yaml = r"
services:
  broken:
    ports:
      - '8080:80'
";
        let compose = parse_compose(yaml).unwrap();
        let result = compose_to_deployment(&compose, "fail-test");
        assert!(result.is_err());
    }
}
