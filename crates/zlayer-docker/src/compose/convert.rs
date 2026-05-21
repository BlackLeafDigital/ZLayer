//! Convert Docker Compose files to `ZLayer` deployment specs.

use std::collections::HashMap;
use std::time::Duration;

use tracing::{info, warn};
use zlayer_spec::{
    CommandSpec, ContainerRestartKind, ContainerRestartPolicy, DependencyCondition, DependsSpec,
    DeploymentSpec, DeviceSpec, EndpointSpec, ErrorsSpec, ExposeType, GpuSpec, HealthCheck,
    HealthSpec, ImageSpec, InitSpec, LifecycleSpec, NetworkMode, NodeMode, Protocol, PullPolicy,
    ResourceType, ResourcesSpec, ScaleSpec, ServiceNetworkSpec, ServiceSpec, ServiceType,
    StorageSpec, StorageTier, TimeoutAction, UlimitSpec,
};

use super::types::{
    ComposeBuild, ComposeFile, ComposeService, ComposeServiceNetworks, DependsOnCondition, EnvFile,
    EnvFileEntry, VolumeType,
};
use crate::DockerError;
#[cfg(test)]
use zlayer_paths::ZLayerDirs;

/// Compose-style image name produced for a service that ships a `build:`
/// directive but no explicit `image:`. Mirrors Docker Compose's
/// `<project>-<service>` (or `<project>_<service>` for legacy names) tagging
/// convention so that `compose build` and `compose up` agree on the tag.
#[must_use]
pub fn build_image_tag(project: &str, service: &str) -> String {
    format!("{project}-{service}:latest")
}

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
        let spec = convert_service(name, svc_name, svc)?;
        services.insert(svc_name.clone(), spec);
    }

    Ok(DeploymentSpec {
        version: "v1".to_string(),
        deployment: name.to_string(),
        services,
        externals: HashMap::new(),
        tunnels: HashMap::new(),
        api: zlayer_spec::ApiSpec::default(),
    })
}

/// Convert a single `ComposeService` into a `ServiceSpec`.
#[allow(clippy::too_many_lines)]
fn convert_service(
    project: &str,
    svc_name: &str,
    svc: &ComposeService,
) -> crate::Result<ServiceSpec> {
    let image = convert_image(project, svc_name, svc)?;
    let endpoints = convert_ports(&svc.ports)?;
    let storage = convert_volumes(&svc.volumes);
    let env = convert_environment(svc)?;
    let depends = convert_depends_on(&svc.depends_on);
    let health = convert_healthcheck(svc.healthcheck.as_ref());
    let command = convert_command(svc);
    let resources = convert_resources(svc);
    let scale = convert_scale(svc);
    let restart_policy = convert_restart_policy(svc.restart.as_deref(), svc.deploy.as_ref());
    let network_mode = convert_network_mode(svc.network_mode.as_deref())?;
    let network = convert_service_network(svc.networks.as_ref());
    let pull_policy = parse_pull_policy(svc.pull_policy.as_deref());
    let platform = parse_target_platform(svc.platform.as_deref());
    let stop_grace_period = svc
        .stop_grace_period
        .as_deref()
        .and_then(parse_compose_duration);
    let devices = convert_devices(&svc.devices);
    let expose = convert_expose(&svc.expose);

    let resources = enrich_resources_with_gpu(resources, svc.deploy.as_ref());
    let resources = enrich_resources_with_gpus_short(resources, svc.gpus.as_ref());

    warn_unsupported_fields(svc_name, svc);

    let dns = svc
        .dns
        .clone()
        .map(super::types::StringOrList::into_list)
        .unwrap_or_default();

    let image = ImageSpec {
        name: image.name,
        pull_policy: pull_policy.unwrap_or(image.pull_policy),
    };

    // Plumb additional groups (`group_add:`) into `extra_groups`.
    let extra_groups: Vec<String> = svc
        .group_add
        .iter()
        .map(super::types::StringOrNumber::as_string)
        .collect();

    // Fold the long-tail compose fields that have no first-class
    // `ServiceSpec` slot into well-known `com.docker.compose.<key>` labels
    // so the data is preserved (it can be inspected via the runtime
    // labels map and rebuilt back into a compose file when needed).
    let labels = build_labels_with_extras(svc);

    Ok(ServiceSpec {
        rtype: ResourceType::Service,
        schedule: None,
        image,
        resources,
        env,
        command,
        network,
        endpoints,
        scale,
        depends,
        health,
        init: InitSpec::default(),
        errors: ErrorsSpec::default(),
        lifecycle: LifecycleSpec::default(),
        devices,
        storage,
        port_mappings: Vec::new(),
        capabilities: svc.cap_add.clone(),
        cap_drop: svc.cap_drop.clone(),
        privileged: svc.privileged,
        node_mode: NodeMode::default(),
        node_selector: None,
        service_type: ServiceType::default(),
        wasm: None,
        logs: None,
        host_network: matches!(network_mode, NetworkMode::Host),
        hostname: svc.hostname.clone(),
        dns,
        extra_hosts: svc.extra_hosts.clone(),
        restart_policy,
        platform,
        labels,
        user: svc.user.clone(),
        stop_signal: svc.stop_signal.clone(),
        stop_grace_period,
        sysctls: svc.sysctls.clone(),
        ulimits: svc
            .ulimits
            .iter()
            .map(|(k, v)| {
                let ulimit = match v {
                    super::types::ComposeUlimit::Single(n) => UlimitSpec { soft: *n, hard: *n },
                    super::types::ComposeUlimit::SoftHard { soft, hard } => UlimitSpec {
                        soft: *soft,
                        hard: *hard,
                    },
                };
                (k.clone(), ulimit)
            })
            .collect(),
        security_opt: svc.security_opt.clone(),
        pid_mode: svc.pid.clone(),
        ipc_mode: svc.ipc.clone(),
        network_mode,
        extra_groups,
        read_only_root_fs: svc.read_only,
        init_container: svc.init,
        tty: svc.tty,
        stdin_open: svc.stdin_open,
        userns_mode: svc.userns_mode.clone(),
        cgroup_parent: svc.cgroup_parent.clone(),
        expose,
        replica_groups: None,
        isolation: None,
    })
}

/// Build the final label map for a service, layering long-tail compose
/// fields that have no first-class `ServiceSpec` slot under reserved
/// `com.docker.compose.<key>` entries. User-supplied labels take priority
/// over the reserved entries so an explicit `labels:` mapping wins.
fn build_labels_with_extras(svc: &ComposeService) -> HashMap<String, String> {
    let mut labels = HashMap::new();

    // Carry-over fields with no spec slot.
    if let Some(ref domain) = svc.domainname {
        labels.insert("com.docker.compose.domainname".to_string(), domain.clone());
    }
    if let Some(ref dns_search) = svc.dns_search {
        let joined = dns_search.clone().into_list().join(",");
        if !joined.is_empty() {
            labels.insert("com.docker.compose.dns_search".to_string(), joined);
        }
    }
    if !svc.dns_opt.is_empty() {
        labels.insert(
            "com.docker.compose.dns_opt".to_string(),
            svc.dns_opt.join(","),
        );
    }
    if let Some(ref mac) = svc.mac_address {
        labels.insert("com.docker.compose.mac_address".to_string(), mac.clone());
    }
    if !svc.links.is_empty() {
        labels.insert("com.docker.compose.links".to_string(), svc.links.join(","));
    }
    if !svc.external_links.is_empty() {
        labels.insert(
            "com.docker.compose.external_links".to_string(),
            svc.external_links.join(","),
        );
    }
    if let Some(ms) = svc.mem_swappiness {
        labels.insert(
            "com.docker.compose.mem_swappiness".to_string(),
            ms.to_string(),
        );
    }
    if svc.oom_kill_disable {
        labels.insert(
            "com.docker.compose.oom_kill_disable".to_string(),
            "true".to_string(),
        );
    }
    if let Some(adj) = svc.oom_score_adj {
        labels.insert(
            "com.docker.compose.oom_score_adj".to_string(),
            adj.to_string(),
        );
    }
    if let Some(ref runtime) = svc.runtime {
        labels.insert("com.docker.compose.runtime".to_string(), runtime.clone());
    }
    if let Some(ref isolation) = svc.isolation {
        labels.insert(
            "com.docker.compose.isolation".to_string(),
            isolation.clone(),
        );
    }
    if let Some(ref cgroup) = svc.cgroup {
        labels.insert("com.docker.compose.cgroup".to_string(), cgroup.clone());
    }
    if let Some(limit) = svc.pids_limit {
        labels.insert(
            "com.docker.compose.pids_limit".to_string(),
            limit.to_string(),
        );
    }
    if !svc.profiles.is_empty() {
        labels.insert(
            "com.docker.compose.profiles".to_string(),
            svc.profiles.join(","),
        );
    }
    if let Some(ref name) = svc.container_name {
        labels.insert(
            "com.docker.compose.container_name".to_string(),
            name.clone(),
        );
    }

    // User-supplied labels win over the synthesized reserved labels.
    for (k, v) in &svc.labels {
        labels.insert(k.clone(), v.clone());
    }
    labels
}

/// Fold the short-form `gpus:` directive into `ResourcesSpec.gpu`.
///
/// Compose accepts:
/// * `gpus: all` — request every GPU on the host (count = 0 sentinel).
/// * `gpus: 2` — request a specific count.
/// * `gpus: ["device=0,1"]` — explicit device selectors (count inferred
///   from the number of comma-separated device IDs).
///
/// If the long-form `deploy.resources.reservations.devices` already
/// populated `resources.gpu`, that wins and this function is a no-op.
fn enrich_resources_with_gpus_short(
    mut spec: ResourcesSpec,
    gpus: Option<&super::types::ComposeGpus>,
) -> ResourcesSpec {
    if spec.gpu.is_some() {
        return spec;
    }
    let Some(gpus) = gpus else {
        return spec;
    };

    let count = match gpus {
        super::types::ComposeGpus::All(s) if s.eq_ignore_ascii_case("all") => 0u32,
        super::types::ComposeGpus::All(_) => return spec,
        super::types::ComposeGpus::Count(n) => u32::try_from(*n).unwrap_or(u32::MAX),
        super::types::ComposeGpus::Devices(devices) => {
            // Count the number of `device=` selectors, falling back to 1.
            let total: u32 = devices
                .iter()
                .map(|d| {
                    d.split_once('=')
                        .filter(|(k, _)| k.eq_ignore_ascii_case("device"))
                        .map_or(1, |(_, ids)| {
                            u32::try_from(ids.split(',').filter(|s| !s.is_empty()).count())
                                .unwrap_or(1)
                        })
                })
                .sum();
            if total == 0 {
                1
            } else {
                total
            }
        }
    };

    spec.gpu = Some(GpuSpec {
        count,
        vendor: "nvidia".to_string(),
        mode: None,
        model: None,
        scheduling: None,
        distributed: None,
        sharing: None,
        mps_pipe_dir: None,
        mps_log_dir: None,
        time_slice_index: None,
        time_slicing_config_path: None,
    });
    spec
}

/// Emit warnings/info for compose fields that are not mapped to `ZLayer`.
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
    if svc.logging.is_some() {
        warn!(
            service = svc_name,
            "custom logging drivers are not supported; ZLayer uses built-in observability"
        );
    }
    if let Some(ref deploy) = svc.deploy {
        if let Some(ref placement) = deploy.placement {
            if !placement.constraints.is_empty()
                || !placement.preferences.is_empty()
                || placement.max_replicas_per_node.is_some()
            {
                info!(
                    service = svc_name,
                    "deploy.placement constraints are not mapped; use ZLayer node selectors"
                );
            }
        }
    }
}

/// Convert the image field, handling build-only services.
///
/// When the service ships a `build:` directive but no explicit `image:`,
/// the resulting [`ImageSpec`] uses the Compose-style `<project>-<service>:latest`
/// tag — the same tag that `compose build` writes — so that `compose up`
/// and `compose build` agree on which image the runtime should resolve.
///
/// Validates the long-form `build:` map: at least one of `context` or
/// `dockerfile_inline` must be present. The short string form (where the
/// whole value is the context path) is always accepted.
fn convert_image(project: &str, svc_name: &str, svc: &ComposeService) -> crate::Result<ImageSpec> {
    if let Some(ref image) = svc.image {
        return Ok(ImageSpec {
            name: image.parse().map_err(|e| {
                DockerError::Conversion(format!(
                    "service '{svc_name}' has invalid image reference '{image}': {e}"
                ))
            })?,
            pull_policy: PullPolicy::IfNotPresent,
        });
    }

    let Some(build) = svc.build.as_ref() else {
        return Err(DockerError::Conversion(format!(
            "service '{svc_name}' has neither 'image' nor 'build' specified"
        )));
    };

    validate_build_directive(svc_name, build)?;

    let tag = build_image_tag(project, svc_name);
    warn!(
        service = svc_name,
        tag = %tag,
        "service uses 'build' without 'image'; the runtime will resolve the tag produced by `compose build`",
    );
    Ok(ImageSpec {
        name: tag.parse().map_err(|e| {
            DockerError::Conversion(format!(
                "failed to build placeholder image reference for service '{svc_name}': {e}"
            ))
        })?,
        pull_policy: PullPolicy::IfNotPresent,
    })
}

/// Validate that a long-form `build:` map carries enough information to
/// actually run a build. Compose accepts `context` OR `dockerfile_inline`
/// (an inline Dockerfile body) — without either of those there is nothing
/// to feed buildah, so we surface the error at convert time rather than at
/// `compose build` time.
fn validate_build_directive(svc_name: &str, build: &ComposeBuild) -> crate::Result<()> {
    match build {
        ComposeBuild::Simple(s) => {
            if s.trim().is_empty() {
                return Err(DockerError::Conversion(format!(
                    "service '{svc_name}' has an empty `build:` context string"
                )));
            }
        }
        ComposeBuild::Full(cfg) => {
            let has_context = cfg.context.as_deref().is_some_and(|c| !c.trim().is_empty());
            let has_inline = cfg
                .dockerfile_inline
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty());
            if !has_context && !has_inline {
                return Err(DockerError::Conversion(format!(
                    "service '{svc_name}' has a `build:` directive with neither `context` nor `dockerfile_inline`"
                )));
            }
        }
    }
    Ok(())
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
            target_role: None,
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
///
/// Honours all compose healthcheck fields that have a `HealthSpec` analogue:
/// `test`, `interval`, `timeout`, `retries` (including the explicit `0` value
/// which compose uses to mean "never give up"), `start_period`, and
/// `start_interval`. The latter currently has no dedicated `HealthSpec` slot,
/// but when set on its own (without an explicit `interval`) we fall back to it
/// so the runtime still polls during startup. `disable: true` is honoured by
/// returning a TCP-port-0 sentinel that downstream layers treat as "no check".
fn convert_healthcheck(hc: Option<&super::types::ComposeHealthcheck>) -> HealthSpec {
    let Some(hc) = hc else {
        return HealthSpec {
            start_grace: Some(Duration::from_secs(5)),
            interval: None,
            timeout: None,
            retries: 3,
            check: HealthCheck::Tcp { port: 0 },
        };
    };

    if hc.disable {
        return HealthSpec {
            start_grace: None,
            interval: None,
            timeout: None,
            retries: 0,
            check: HealthCheck::Tcp { port: 0 },
        };
    }

    let check = convert_health_test(&hc.test);
    let interval_main = hc.interval.as_deref().and_then(parse_compose_duration);
    let start_interval = hc
        .start_interval
        .as_deref()
        .and_then(parse_compose_duration);
    // If only `start_interval` is provided (no main `interval`), use it as the
    // effective polling interval so the daemon doesn't fall back to a hard-
    // coded default during startup. Compose's semantics are subtly different
    // (start_interval only applies during the start_period), but until the
    // HealthSpec gains a separate field, this is the closest non-lossy mapping.
    let interval = interval_main.or(start_interval);
    let timeout = hc.timeout.as_deref().and_then(parse_compose_duration);
    let start_grace = hc.start_period.as_deref().and_then(parse_compose_duration);
    // Compose's `retries: 0` means "infinite retries" (default 3 otherwise).
    // `HealthSpec::retries` is a `u32`; we preserve the explicit `0` value
    // rather than silently forcing the default-3 fallback.
    let retries = match hc.retries {
        None => 3,
        Some(n) => u32::try_from(n).unwrap_or(0),
    };

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
///
/// Honours both `deploy.resources.limits` (mapped to `cpu` / `memory`) and
/// `deploy.resources.reservations` (mapped to `cpu_shares` / `memory_reservation`).
/// `reservations.devices` with the `gpu` capability is folded into `gpu`
/// separately by `enrich_resources_with_gpu`.
fn convert_resources(svc: &ComposeService) -> ResourcesSpec {
    let Some(deploy) = &svc.deploy else {
        return ResourcesSpec::default();
    };
    let Some(resources) = &deploy.resources else {
        return ResourcesSpec::default();
    };

    let mut spec = ResourcesSpec::default();

    if let Some(limits) = &resources.limits {
        spec.cpu = limits
            .cpus
            .as_ref()
            .and_then(|c| c.as_string().parse::<f64>().ok());
        spec.memory.clone_from(&limits.memory);
        spec.pids_limit = limits.pids.and_then(|p| i64::try_from(p).ok());
    }

    if let Some(reservations) = &resources.reservations {
        // Reservations map onto Docker's `--cpu-shares` (relative weight) and
        // `--memory-reservation` (soft floor). cpu_shares is `u32`; we
        // translate fractional CPU reservations into the standard 1024-per-core
        // weight so that `cpus: "0.5"` → `512`, `cpus: "2"` → `2048`.
        if let Some(cpu_str) = reservations
            .cpus
            .as_ref()
            .map(super::types::StringOrNumber::as_string)
        {
            if let Ok(cpu_f) = cpu_str.parse::<f64>() {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let shares = (cpu_f * 1024.0).round() as u32;
                if shares > 0 {
                    spec.cpu_shares = Some(shares);
                }
            }
        }
        if let Some(mem) = &reservations.memory {
            spec.memory_reservation = Some(mem.clone());
        }
    }

    spec
}

/// Fold `deploy.resources.reservations.devices` GPU requests into
/// `ResourcesSpec.gpu`.
fn enrich_resources_with_gpu(
    mut spec: ResourcesSpec,
    deploy: Option<&super::types::ComposeDeploy>,
) -> ResourcesSpec {
    let Some(deploy) = deploy else {
        return spec;
    };
    let Some(resources) = &deploy.resources else {
        return spec;
    };
    let Some(reservations) = &resources.reservations else {
        return spec;
    };
    let Some(devices) = &reservations.devices else {
        return spec;
    };

    for dev in devices {
        // Compose marks GPU requests with `capabilities: [["gpu"]]` (a list of
        // capability sets). We treat any device request whose capability list
        // contains the `gpu` token as a GPU reservation.
        let has_gpu_cap = dev
            .capabilities
            .iter()
            .any(|set| set.iter().any(|c| c.eq_ignore_ascii_case("gpu")));
        if !has_gpu_cap {
            continue;
        }

        let count = dev
            .count
            .as_ref()
            .and_then(|c| c.as_string().parse::<u32>().ok())
            .unwrap_or(1);
        let vendor = dev.driver.clone().unwrap_or_else(|| "nvidia".to_string());

        spec.gpu = Some(GpuSpec {
            count,
            vendor,
            mode: None,
            model: None,
            scheduling: None,
            distributed: None,
            sharing: None,
            mps_pipe_dir: None,
            mps_log_dir: None,
            time_slice_index: None,
            time_slicing_config_path: None,
        });
        break;
    }

    spec
}

/// Convert compose `restart` (top-level service field) and `deploy.restart_policy`
/// into a [`ContainerRestartPolicy`].
///
/// Compose accepts: `"no"`, `"always"`, `"on-failure"`, `"on-failure:N"`,
/// `"unless-stopped"`. The Swarm-style `deploy.restart_policy` block uses
/// `condition: any|on-failure|none` and an explicit `max_attempts`; when
/// present it takes precedence over the simpler top-level `restart` string.
fn convert_restart_policy(
    restart: Option<&str>,
    deploy: Option<&super::types::ComposeDeploy>,
) -> Option<ContainerRestartPolicy> {
    if let Some(deploy_policy) = deploy.and_then(|d| d.restart_policy.as_ref()) {
        let kind = match deploy_policy.condition.as_deref() {
            Some("none") => ContainerRestartKind::No,
            Some("on-failure") => ContainerRestartKind::OnFailure,
            Some("any") | None => ContainerRestartKind::Always,
            Some(other) => {
                warn!(
                    condition = other,
                    "unknown deploy.restart_policy.condition; defaulting to always"
                );
                ContainerRestartKind::Always
            }
        };
        let max_attempts = deploy_policy
            .max_attempts
            .and_then(|n| u32::try_from(n).ok());
        let delay = deploy_policy.delay.clone();
        return Some(ContainerRestartPolicy {
            kind,
            max_attempts,
            delay,
        });
    }

    let restart = restart?.trim();
    if restart.is_empty() {
        return None;
    }

    let (kind, max_attempts) = if let Some(rest) = restart.strip_prefix("on-failure:") {
        (
            ContainerRestartKind::OnFailure,
            rest.trim().parse::<u32>().ok(),
        )
    } else {
        match restart {
            "no" => (ContainerRestartKind::No, None),
            "always" => (ContainerRestartKind::Always, None),
            "on-failure" => (ContainerRestartKind::OnFailure, None),
            "unless-stopped" => (ContainerRestartKind::UnlessStopped, None),
            other => {
                warn!(value = other, "unknown compose restart value; ignoring");
                return None;
            }
        }
    };

    Some(ContainerRestartPolicy {
        kind,
        max_attempts,
        delay: None,
    })
}

/// Convert compose `network_mode` string into [`NetworkMode`].
fn convert_network_mode(mode: Option<&str>) -> crate::Result<NetworkMode> {
    let Some(mode) = mode else {
        return Ok(NetworkMode::default());
    };
    let mode = mode.trim();
    if mode.is_empty() {
        return Ok(NetworkMode::default());
    }
    match mode {
        "default" => Ok(NetworkMode::Default),
        "host" => Ok(NetworkMode::Host),
        "none" => Ok(NetworkMode::None),
        "bridge" => Ok(NetworkMode::Bridge { name: None }),
        other => {
            if let Some(rest) = other.strip_prefix("bridge:") {
                let name = rest.trim();
                if name.is_empty() {
                    Ok(NetworkMode::Bridge { name: None })
                } else {
                    Ok(NetworkMode::Bridge {
                        name: Some(name.to_string()),
                    })
                }
            } else if let Some(rest) = other.strip_prefix("container:") {
                let id = rest.trim();
                if id.is_empty() {
                    Err(DockerError::Conversion(
                        "network_mode 'container:<id>' requires a non-empty id".to_string(),
                    ))
                } else {
                    Ok(NetworkMode::Container { id: id.to_string() })
                }
            } else if let Some(rest) = other.strip_prefix("service:") {
                let svc = rest.trim();
                Err(DockerError::Conversion(format!(
                    "network_mode 'service:{svc}' is not supported by ZLayer"
                )))
            } else {
                Err(DockerError::Conversion(format!(
                    "unknown network_mode '{other}'"
                )))
            }
        }
    }
}

/// Convert compose service-level `networks` into a [`ServiceNetworkSpec`].
///
/// `ZLayer` does not have a direct equivalent of Docker's per-service network
/// attachments, but we still acknowledge the shape so that callers can detect
/// non-empty network maps. The default service network spec is preserved when
/// no compose networks are declared.
fn convert_service_network(networks: Option<&ComposeServiceNetworks>) -> ServiceNetworkSpec {
    let _ = networks;
    ServiceNetworkSpec::default()
}

/// Parse compose `pull_policy`. Compose values: `"always"`, `"never"`,
/// `"if_not_present"`, `"missing"` (alias of `if_not_present`),
/// `"build"` (build-only — falls back to default), `"never"`.
fn parse_pull_policy(s: Option<&str>) -> Option<PullPolicy> {
    let s = s?.trim().to_ascii_lowercase();
    match s.as_str() {
        "always" => Some(PullPolicy::Always),
        "never" => Some(PullPolicy::Never),
        "missing" | "if_not_present" | "if-not-present" => Some(PullPolicy::IfNotPresent),
        "newer" | "daily" => Some(PullPolicy::Newer),
        "build" => None,
        other => {
            warn!(value = other, "unknown compose pull_policy; ignoring");
            None
        }
    }
}

/// Parse a compose `platform` string (e.g. `"linux/amd64"`,
/// `"windows/arm64"`).
fn parse_target_platform(s: Option<&str>) -> Option<zlayer_spec::TargetPlatform> {
    use zlayer_spec::{ArchKind, OsKind, TargetPlatform};
    let s = s?.trim();
    if s.is_empty() {
        return None;
    }
    let (os_str, arch_str) = s.split_once('/')?;
    let os = match os_str.to_ascii_lowercase().as_str() {
        "linux" => OsKind::Linux,
        "windows" => OsKind::Windows,
        "darwin" | "macos" | "osx" => OsKind::Macos,
        other => {
            warn!(value = other, "unknown compose platform OS; ignoring");
            return None;
        }
    };
    let arch = match arch_str.to_ascii_lowercase().as_str() {
        "amd64" | "x86_64" => ArchKind::Amd64,
        "arm64" | "aarch64" => ArchKind::Arm64,
        other => {
            warn!(value = other, "unknown compose platform arch; ignoring");
            return None;
        }
    };
    Some(TargetPlatform::new(os, arch))
}

/// Convert compose `devices` (e.g. `"/dev/sda"`, `"/dev/sda:/dev/xvda:rwm"`)
/// into [`DeviceSpec`] entries.
fn convert_devices(devices: &[String]) -> Vec<DeviceSpec> {
    devices
        .iter()
        .filter_map(|raw| {
            let raw = raw.trim();
            if raw.is_empty() {
                return None;
            }
            let mut parts = raw.split(':');
            let host = parts.next()?.trim().to_string();
            // The container path / cgroup permissions tail (parts 2 and 3) is
            // captured by the runtime via the host path; ZLayer's `DeviceSpec`
            // doesn't carry a separate container path field.
            let _container_path = parts.next();
            let perms = parts.next().unwrap_or("rwm");
            if host.is_empty() {
                return None;
            }
            Some(DeviceSpec {
                path: host,
                read: perms.contains('r'),
                write: perms.contains('w'),
                mknod: perms.contains('m'),
            })
        })
        .collect()
}

/// Convert compose `expose:` entries (a list of port strings or numbers)
/// into the `ServiceSpec::expose` field.
fn convert_expose(expose: &[super::types::StringOrNumber]) -> Vec<String> {
    expose
        .iter()
        .map(super::types::StringOrNumber::as_string)
        .collect()
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
        assert_eq!(web.image.name.to_string(), "docker.io/library/nginx:latest");
        assert_eq!(web.endpoints.len(), 1);
        assert_eq!(web.endpoints[0].port, 8080);
        assert_eq!(web.endpoints[0].target_port, Some(80));
        assert_eq!(web.endpoints[0].protocol, Protocol::Http);
        assert_eq!(web.env.get("FOO").unwrap(), "bar");

        let db = &spec.services["db"];
        assert_eq!(db.image.name.to_string(), "docker.io/library/postgres:16");
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
        // Compose-style tag: `<project>-<service>:latest`.
        assert_eq!(
            app.image.name.to_string(),
            "docker.io/library/build-test-app:latest",
        );
        assert_eq!(app.image.pull_policy, PullPolicy::IfNotPresent);
    }

    /// Task 6.4.1: `build:` short-form string is accepted and produces a
    /// project-scoped placeholder image tag.
    #[test]
    fn test_build_conversion_short_string_form() {
        let yaml = r"
services:
  api:
    build: ./services/api
";
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "myproj").unwrap();
        let api = &spec.services["api"];
        assert_eq!(
            api.image.name.to_string(),
            "docker.io/library/myproj-api:latest",
            "short-form `build:` must produce <project>-<service>:latest",
        );
        // Confirm the helper emits the same un-canonicalised form too.
        assert_eq!(build_image_tag("myproj", "api"), "myproj-api:latest");
    }

    /// Task 6.4.2: `build:` map form with every documented field round-trips
    /// without errors and still produces the project-scoped tag.
    #[test]
    fn test_build_conversion_map_form_with_all_fields() {
        let yaml = r#"
services:
  worker:
    build:
      context: ./worker
      dockerfile: Dockerfile.prod
      args:
        VERSION: "1.2.3"
        FEATURE_X: "true"
      target: runtime
      cache_from:
        - "ghcr.io/foo/worker:cache"
        - "ghcr.io/foo/worker:latest"
      network: host
      shm_size: "256m"
      labels:
        com.example.maintainer: "ops"
        tier: "backend"
"#;
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "fullbuild").unwrap();
        let worker = &spec.services["worker"];
        assert_eq!(
            worker.image.name.to_string(),
            "docker.io/library/fullbuild-worker:latest",
        );

        // The build directive itself stays attached to the compose model so
        // `compose build` can read it; assert via the parsed compose, not the
        // ServiceSpec (which intentionally does not carry build args today).
        let build = compose
            .services
            .get("worker")
            .and_then(|s| s.build.as_ref())
            .expect("build directive present");
        match build {
            ComposeBuild::Full(cfg) => {
                assert_eq!(cfg.context.as_deref(), Some("./worker"));
                assert_eq!(cfg.dockerfile.as_deref(), Some("Dockerfile.prod"));
                assert_eq!(cfg.args.get("VERSION").map(String::as_str), Some("1.2.3"),);
                assert_eq!(cfg.args.get("FEATURE_X").map(String::as_str), Some("true"),);
                assert_eq!(cfg.target.as_deref(), Some("runtime"));
                assert_eq!(cfg.cache_from.len(), 2);
                assert_eq!(cfg.network.as_deref(), Some("host"));
                let shm = cfg.shm_size.as_ref().expect("shm_size set").as_string();
                assert_eq!(shm, "256m");
                assert_eq!(
                    cfg.labels.get("com.example.maintainer").map(String::as_str),
                    Some("ops"),
                );
                assert_eq!(cfg.labels.get("tier").map(String::as_str), Some("backend"));
            }
            ComposeBuild::Simple(_) => panic!("expected long-form build, got short-form"),
        }
    }

    /// Task 6.4.3: long-form `build:` with neither `context` nor
    /// `dockerfile_inline` is rejected by [`compose_to_deployment`] rather
    /// than silently producing a build that has nothing to feed buildah.
    #[test]
    fn test_build_conversion_missing_context_errors() {
        let yaml = r"
services:
  bad:
    build:
      args:
        FOO: bar
";
        let compose = parse_compose(yaml).unwrap();
        let err = compose_to_deployment(&compose, "miss")
            .expect_err("missing context+dockerfile_inline must error during conversion");
        let msg = err.to_string();
        assert!(
            msg.contains("bad") && msg.contains("context"),
            "error must mention the offending service and context: {msg}",
        );
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

    // -------------------------------------------------------------------
    // Silent-drop fix coverage (task 6.7.1-16). One-to-two unit tests
    // per category, asserting that previously-dropped compose fields now
    // round-trip onto the resulting `ServiceSpec`.
    // -------------------------------------------------------------------

    /// Category 1: `labels` round-trips onto `ServiceSpec.labels`.
    #[test]
    fn test_silent_drop_labels_plumbed() {
        let yaml = r#"
services:
  app:
    image: myapp:latest
    labels:
      com.example.version: "1.0"
      tier: "frontend"
"#;
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "labels-test").unwrap();
        let app = &spec.services["app"];
        assert_eq!(
            app.labels.get("com.example.version").map(String::as_str),
            Some("1.0")
        );
        assert_eq!(app.labels.get("tier").map(String::as_str), Some("frontend"));
    }

    /// Category 2: `user` is plumbed.
    #[test]
    fn test_silent_drop_user_plumbed() {
        let yaml = r"
services:
  app:
    image: myapp:latest
    user: '1000:1000'
";
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "user-test").unwrap();
        assert_eq!(spec.services["app"].user.as_deref(), Some("1000:1000"));
    }

    /// Category 3: `read_only`, `init`, `tty`, `stdin_open` are plumbed.
    #[test]
    fn test_silent_drop_read_only_init_tty_stdin_plumbed() {
        let yaml = r"
services:
  app:
    image: myapp:latest
    read_only: true
    init: true
    tty: true
    stdin_open: true
";
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "flags-test").unwrap();
        let app = &spec.services["app"];
        assert!(app.read_only_root_fs);
        assert_eq!(app.init_container, Some(true));
        assert!(app.tty);
        assert!(app.stdin_open);
    }

    /// Category 4: `stop_signal` and `stop_grace_period` are plumbed.
    #[test]
    fn test_silent_drop_stop_signal_and_grace_plumbed() {
        let yaml = r"
services:
  app:
    image: myapp:latest
    stop_signal: SIGTERM
    stop_grace_period: 45s
";
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "stop-test").unwrap();
        let app = &spec.services["app"];
        assert_eq!(app.stop_signal.as_deref(), Some("SIGTERM"));
        assert_eq!(app.stop_grace_period, Some(Duration::from_secs(45)));
    }

    /// Category 5: `sysctls` and `ulimits` are plumbed.
    #[test]
    fn test_silent_drop_sysctls_and_ulimits_plumbed() {
        let yaml = r"
services:
  app:
    image: myapp:latest
    sysctls:
      net.core.somaxconn: '1024'
    ulimits:
      nofile:
        soft: 1024
        hard: 65536
";
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "syslim-test").unwrap();
        let app = &spec.services["app"];
        assert_eq!(
            app.sysctls.get("net.core.somaxconn").map(String::as_str),
            Some("1024")
        );
        let nofile = app.ulimits.get("nofile").expect("nofile ulimit present");
        assert_eq!(nofile.soft, 1024);
        assert_eq!(nofile.hard, 65536);
    }

    /// Category 6: `security_opt`, `pid`, `ipc`, `cgroup_parent` are plumbed.
    /// (`userns_mode` is intentionally not represented in the compose service
    /// surface area today; its `ServiceSpec` slot is reserved for future use.)
    #[test]
    fn test_silent_drop_security_pid_ipc_cgroup_plumbed() {
        let yaml = r"
services:
  app:
    image: myapp:latest
    security_opt:
      - no-new-privileges:true
      - seccomp=unconfined
    pid: host
    ipc: shareable
    cgroup_parent: /custom-cgroup
";
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "ns-test").unwrap();
        let app = &spec.services["app"];
        assert_eq!(
            app.security_opt,
            vec![
                "no-new-privileges:true".to_string(),
                "seccomp=unconfined".to_string(),
            ]
        );
        assert_eq!(app.pid_mode.as_deref(), Some("host"));
        assert_eq!(app.ipc_mode.as_deref(), Some("shareable"));
        assert_eq!(app.cgroup_parent.as_deref(), Some("/custom-cgroup"));
    }

    /// Category 7: `devices` are plumbed onto `ServiceSpec.devices`, and
    /// `deploy.resources.reservations.devices` with `gpu` capability folds
    /// into `resources.gpu`.
    #[test]
    fn test_silent_drop_devices_and_gpus_plumbed() {
        let yaml = r#"
services:
  vm:
    image: myapp:latest
    devices:
      - "/dev/kvm"
      - "/dev/net/tun:/dev/net/tun:rwm"
  ml:
    image: pytorch:latest
    deploy:
      resources:
        reservations:
          devices:
            - driver: nvidia
              count: 2
              capabilities: [["gpu"]]
"#;
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "dev-test").unwrap();

        let vm = &spec.services["vm"];
        assert_eq!(vm.devices.len(), 2);
        assert_eq!(vm.devices[0].path, "/dev/kvm");
        assert!(vm.devices[0].read && vm.devices[0].write);
        assert_eq!(vm.devices[1].path, "/dev/net/tun");
        assert!(vm.devices[1].mknod);

        let ml = &spec.services["ml"];
        let gpu = ml.resources.gpu.as_ref().expect("GPU plumbed");
        assert_eq!(gpu.count, 2);
        assert_eq!(gpu.vendor, "nvidia");
    }

    /// Category 8: `cap_drop` is plumbed.
    #[test]
    fn test_silent_drop_cap_drop_plumbed() {
        let yaml = r"
services:
  app:
    image: myapp:latest
    cap_drop:
      - NET_RAW
      - MKNOD
";
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "cap-test").unwrap();
        assert_eq!(
            spec.services["app"].cap_drop,
            vec!["NET_RAW".to_string(), "MKNOD".to_string()]
        );
    }

    /// Category 9: `restart` is plumbed onto `ServiceSpec.restart_policy`,
    /// and the Swarm-style `deploy.restart_policy` block is honoured too.
    #[test]
    fn test_silent_drop_restart_policy_plumbed() {
        let yaml = r"
services:
  always:
    image: myapp:latest
    restart: always
  on_failure_capped:
    image: myapp:latest
    restart: 'on-failure:5'
  swarm:
    image: myapp:latest
    deploy:
      restart_policy:
        condition: on-failure
        max_attempts: 3
        delay: '500ms'
";
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "restart-test").unwrap();

        let always = spec.services["always"]
            .restart_policy
            .as_ref()
            .expect("restart policy present");
        assert_eq!(always.kind, ContainerRestartKind::Always);
        assert_eq!(always.max_attempts, None);

        let onf = spec.services["on_failure_capped"]
            .restart_policy
            .as_ref()
            .expect("on_failure restart policy present");
        assert_eq!(onf.kind, ContainerRestartKind::OnFailure);
        assert_eq!(onf.max_attempts, Some(5));

        let swarm = spec.services["swarm"]
            .restart_policy
            .as_ref()
            .expect("swarm restart policy present");
        assert_eq!(swarm.kind, ContainerRestartKind::OnFailure);
        assert_eq!(swarm.max_attempts, Some(3));
        assert_eq!(swarm.delay.as_deref(), Some("500ms"));
    }

    /// Category 10: `network_mode` is plumbed; service-level `networks` no
    /// longer crashes the conversion (it's accepted but not mapped).
    #[test]
    fn test_silent_drop_network_mode_plumbed() {
        let yaml = r"
services:
  hostnet:
    image: myapp:latest
    network_mode: host
  bridged:
    image: myapp:latest
    network_mode: 'bridge:custom-net'
  named:
    image: myapp:latest
    networks:
      - frontend
      - backend
";
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "net-test").unwrap();
        assert_eq!(spec.services["hostnet"].network_mode, NetworkMode::Host);
        assert!(spec.services["hostnet"].host_network);
        match &spec.services["bridged"].network_mode {
            NetworkMode::Bridge { name } => {
                assert_eq!(name.as_deref(), Some("custom-net"));
            }
            other => panic!("expected bridge:custom-net, got {other:?}"),
        }
        // `named` has no network_mode override, default applies.
        assert_eq!(spec.services["named"].network_mode, NetworkMode::Default);
    }

    /// Category 11: `expose` (port-list) is plumbed onto
    /// `ServiceSpec.expose`.
    #[test]
    fn test_silent_drop_expose_plumbed() {
        let yaml = r#"
services:
  app:
    image: myapp:latest
    expose:
      - "3000"
      - "8080/tcp"
      - 9090
"#;
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "expose-test").unwrap();
        let app = &spec.services["app"];
        assert_eq!(app.expose.len(), 3);
        assert!(app.expose.iter().any(|p| p == "3000"));
        assert!(app.expose.iter().any(|p| p == "8080/tcp"));
        assert!(app.expose.iter().any(|p| p == "9090"));
    }

    /// Category 12: `pull_policy` and `platform` are plumbed.
    #[test]
    fn test_silent_drop_pull_policy_and_platform_plumbed() {
        let yaml = r"
services:
  always:
    image: myapp:latest
    pull_policy: always
    platform: linux/arm64
";
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "pull-test").unwrap();
        let svc = &spec.services["always"];
        assert_eq!(svc.image.pull_policy, PullPolicy::Always);
        let plat = svc.platform.clone().expect("platform plumbed");
        assert_eq!(plat.as_oci_str(), "linux/arm64");
    }

    /// Category 13: healthcheck `test` arrays with `CMD` / `CMD-SHELL` map
    /// to `HealthCheck::Command`. (Existing `test_healthcheck_conversion`
    /// already covers `CMD`; here we add `CMD-SHELL`.)
    #[test]
    fn test_silent_drop_healthcheck_cmd_shell_plumbed() {
        let yaml = r#"
services:
  web:
    image: nginx:latest
    healthcheck:
      test: ["CMD-SHELL", "curl -f http://localhost || exit 1"]
"#;
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "hc-test").unwrap();
        match &spec.services["web"].health.check {
            HealthCheck::Command { command } => {
                assert_eq!(command, "curl -f http://localhost || exit 1");
            }
            other => panic!("expected Command, got {other:?}"),
        }
    }

    /// Category 14: `retries: 0` is honoured (not silently coerced to 3),
    /// and `start_interval` is honoured as a fallback poll interval.
    #[test]
    fn test_silent_drop_healthcheck_retries_zero_and_start_interval() {
        let yaml = r#"
services:
  patient:
    image: myapp:latest
    healthcheck:
      test: ["CMD", "true"]
      retries: 0
      start_interval: 2s
"#;
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "hc-retries-test").unwrap();
        let h = &spec.services["patient"].health;
        assert_eq!(h.retries, 0);
        // No main `interval`, so `start_interval` becomes the effective poll.
        assert_eq!(h.interval, Some(Duration::from_secs(2)));
    }

    /// Category 15: `deploy.resources.reservations` (CPU + memory) folds into
    /// `ResourcesSpec.cpu_shares` and `ResourcesSpec.memory_reservation`.
    #[test]
    fn test_silent_drop_resource_reservations_plumbed() {
        let yaml = r#"
services:
  app:
    image: myapp:latest
    deploy:
      resources:
        limits:
          cpus: "1.0"
          memory: "1G"
        reservations:
          cpus: "0.5"
          memory: "512M"
"#;
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "reservations-test").unwrap();
        let r = &spec.services["app"].resources;
        assert_eq!(r.cpu, Some(1.0));
        assert_eq!(r.memory.as_deref(), Some("1G"));
        // 0.5 CPU * 1024 weight = 512.
        assert_eq!(r.cpu_shares, Some(512));
        assert_eq!(r.memory_reservation.as_deref(), Some("512M"));
    }

    /// Category 16: `env_file` contents are merged into `service.env`,
    /// with inline `environment` overriding file entries.
    #[test]
    fn test_silent_drop_env_file_forwarding() {
        let dir = ZLayerDirs::system_default()
            .scratch_dir("test-silent-drop-env-file-forwarding-")
            .expect("tempdir");
        let env_path = dir.path().join("svc.env");
        std::fs::write(
            &env_path,
            "FOO=from_file\nSHARED=from_file\n# comment\n\nQUOTED=\"hello world\"\n",
        )
        .expect("write env file");

        let yaml = format!(
            r"
services:
  app:
    image: myapp:latest
    env_file: {env_path:?}
    environment:
      SHARED: from_inline
      EXTRA: present
",
            env_path = env_path.display().to_string(),
        );

        let compose = parse_compose(&yaml).unwrap();
        let spec = compose_to_deployment(&compose, "env-test").unwrap();
        let env = &spec.services["app"].env;
        assert_eq!(env.get("FOO").map(String::as_str), Some("from_file"));
        assert_eq!(env.get("QUOTED").map(String::as_str), Some("hello world"));
        // Inline `environment` wins over `env_file`.
        assert_eq!(env.get("SHARED").map(String::as_str), Some("from_inline"));
        assert_eq!(env.get("EXTRA").map(String::as_str), Some("present"));
    }

    // -------------------------------------------------------------------
    // Long-tail field coverage (task 6.8.1-5+). Each test asserts that a
    // previously-ignored compose field now round-trips into the resulting
    // `ServiceSpec` (either via a first-class slot or via a reserved
    // `com.docker.compose.<key>` label).
    // -------------------------------------------------------------------

    #[test]
    fn test_long_tail_gpus_short_form_all_plumbed() {
        let yaml = r"
services:
  ml:
    image: pytorch:latest
    gpus: all
";
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "gpu-all").unwrap();
        let gpu = spec.services["ml"]
            .resources
            .gpu
            .as_ref()
            .expect("gpu plumbed");
        // `all` is the count=0 sentinel.
        assert_eq!(gpu.count, 0);
        assert_eq!(gpu.vendor, "nvidia");
    }

    #[test]
    fn test_long_tail_gpus_short_form_count_plumbed() {
        let yaml = r"
services:
  ml:
    image: pytorch:latest
    gpus: 3
";
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "gpu-count").unwrap();
        let gpu = spec.services["ml"]
            .resources
            .gpu
            .as_ref()
            .expect("gpu plumbed");
        assert_eq!(gpu.count, 3);
    }

    #[test]
    fn test_long_tail_gpus_long_form_takes_precedence() {
        // When both short-form `gpus` and long-form `deploy.resources` are
        // present, the long form wins (it carries vendor + capability).
        let yaml = r#"
services:
  ml:
    image: pytorch:latest
    gpus: 1
    deploy:
      resources:
        reservations:
          devices:
            - driver: amd
              count: 4
              capabilities: [["gpu"]]
"#;
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "gpu-both").unwrap();
        let gpu = spec.services["ml"]
            .resources
            .gpu
            .as_ref()
            .expect("gpu plumbed");
        assert_eq!(gpu.count, 4);
        assert_eq!(gpu.vendor, "amd");
    }

    #[test]
    fn test_long_tail_dns_dns_search_dns_opt_mac_address_plumbed() {
        let yaml = r#"
services:
  app:
    image: alpine
    dns:
      - 1.1.1.1
      - 8.8.8.8
    dns_search:
      - example.com
    dns_opt:
      - "use-vc"
      - "no-tld-query"
    mac_address: "02:42:ac:11:00:02"
    domainname: "internal.example.com"
"#;
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "dns-test").unwrap();
        let app = &spec.services["app"];
        // dns is first-class.
        assert_eq!(app.dns, vec!["1.1.1.1", "8.8.8.8"]);
        // dns_search, dns_opt, mac_address, domainname land in reserved labels.
        assert_eq!(
            app.labels
                .get("com.docker.compose.dns_search")
                .map(String::as_str),
            Some("example.com")
        );
        assert_eq!(
            app.labels
                .get("com.docker.compose.dns_opt")
                .map(String::as_str),
            Some("use-vc,no-tld-query")
        );
        assert_eq!(
            app.labels
                .get("com.docker.compose.mac_address")
                .map(String::as_str),
            Some("02:42:ac:11:00:02")
        );
        assert_eq!(
            app.labels
                .get("com.docker.compose.domainname")
                .map(String::as_str),
            Some("internal.example.com")
        );
    }

    #[test]
    fn test_long_tail_links_external_links_plumbed() {
        let yaml = r#"
services:
  app:
    image: alpine
    links:
      - db
      - redis:cache
    external_links:
      - "external_db:db"
"#;
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "link-test").unwrap();
        let app = &spec.services["app"];
        assert_eq!(
            app.labels
                .get("com.docker.compose.links")
                .map(String::as_str),
            Some("db,redis:cache")
        );
        assert_eq!(
            app.labels
                .get("com.docker.compose.external_links")
                .map(String::as_str),
            Some("external_db:db")
        );
    }

    #[test]
    fn test_long_tail_oom_runtime_userns_plumbed() {
        let yaml = r"
services:
  app:
    image: alpine
    oom_kill_disable: true
    oom_score_adj: -500
    mem_swappiness: 60
    runtime: nvidia
    userns_mode: host
    pids_limit: 1024
    cgroup: private
    isolation: process
";
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "oom-test").unwrap();
        let app = &spec.services["app"];
        // userns_mode is first-class.
        assert_eq!(app.userns_mode.as_deref(), Some("host"));
        // The rest land in reserved labels.
        assert_eq!(
            app.labels
                .get("com.docker.compose.oom_kill_disable")
                .map(String::as_str),
            Some("true")
        );
        assert_eq!(
            app.labels
                .get("com.docker.compose.oom_score_adj")
                .map(String::as_str),
            Some("-500")
        );
        assert_eq!(
            app.labels
                .get("com.docker.compose.mem_swappiness")
                .map(String::as_str),
            Some("60")
        );
        assert_eq!(
            app.labels
                .get("com.docker.compose.runtime")
                .map(String::as_str),
            Some("nvidia")
        );
        assert_eq!(
            app.labels
                .get("com.docker.compose.pids_limit")
                .map(String::as_str),
            Some("1024")
        );
        assert_eq!(
            app.labels
                .get("com.docker.compose.cgroup")
                .map(String::as_str),
            Some("private")
        );
        assert_eq!(
            app.labels
                .get("com.docker.compose.isolation")
                .map(String::as_str),
            Some("process")
        );
    }

    #[test]
    fn test_long_tail_group_add_plumbed() {
        let yaml = r#"
services:
  app:
    image: alpine
    group_add:
      - "users"
      - 200
      - "audio"
"#;
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "group-test").unwrap();
        let app = &spec.services["app"];
        assert_eq!(
            app.extra_groups,
            vec!["users".to_string(), "200".to_string(), "audio".to_string()]
        );
    }

    #[test]
    fn test_long_tail_user_labels_override_reserved() {
        // If the user explicitly sets a `com.docker.compose.runtime` label,
        // their value must win over the synthesized one.
        let yaml = r"
services:
  app:
    image: alpine
    runtime: nvidia
    labels:
      com.docker.compose.runtime: user-specified
      tier: backend
";
        let compose = parse_compose(yaml).unwrap();
        let spec = compose_to_deployment(&compose, "label-prio").unwrap();
        let app = &spec.services["app"];
        // User-supplied label wins.
        assert_eq!(
            app.labels
                .get("com.docker.compose.runtime")
                .map(String::as_str),
            Some("user-specified")
        );
        assert_eq!(app.labels.get("tier").map(String::as_str), Some("backend"));
    }

    #[test]
    fn test_long_tail_develop_watch_parses() {
        // `develop.watch[]` is parsed but not consumed by the conversion;
        // we verify the data round-trips via the ComposeFile struct itself
        // since there is no first-class spec slot for it.
        let yaml = r#"
services:
  web:
    image: node:20
    develop:
      watch:
        - path: ./src
          action: sync
          target: /app/src
          ignore:
            - "**/*.test.ts"
        - path: package.json
          action: rebuild
"#;
        let compose = parse_compose(yaml).unwrap();
        let dev = compose.services["web"]
            .develop
            .as_ref()
            .expect("develop parsed");
        assert_eq!(dev.watch.len(), 2);
        assert_eq!(dev.watch[0].path, "./src");
        assert_eq!(dev.watch[0].action, "sync");
        assert_eq!(dev.watch[0].target.as_deref(), Some("/app/src"));
        assert_eq!(dev.watch[0].ignore, vec!["**/*.test.ts"]);
        assert_eq!(dev.watch[1].path, "package.json");
        assert_eq!(dev.watch[1].action, "rebuild");
    }

    #[test]
    fn test_long_tail_blkio_config_parses() {
        let yaml = r#"
services:
  app:
    image: alpine
    blkio_config:
      weight: 300
      device_read_bps:
        - path: /dev/sda
          rate: "10mb"
      device_write_iops:
        - path: /dev/sda
          rate: 1000
"#;
        let compose = parse_compose(yaml).unwrap();
        let bio = compose.services["app"]
            .blkio_config
            .as_ref()
            .expect("blkio parsed");
        assert_eq!(bio.weight, Some(300));
        assert_eq!(bio.device_read_bps.len(), 1);
        assert_eq!(bio.device_read_bps[0].path, "/dev/sda");
        assert_eq!(bio.device_write_iops[0].rate, 1000);
    }
}
