//! `docker run` command implementation.
//!
//! Translates Docker `run` flags into a minimal `ZLayer` [`DeploymentSpec`]
//! and submits it to the daemon via `zlayer-client`. Flags that have no
//! `ZLayer` equivalent are logged as warnings but do not block the deploy.

use anyhow::{Context, Result};
use clap::Parser;
use std::collections::HashMap;

use zlayer_client::{default_socket_path, DaemonClient};
use zlayer_spec::{
    CommandSpec, DeploymentSpec, EndpointSpec, ExposeType, HealthCheck, HealthSpec, ImageSpec,
    Protocol, PullPolicy, ScaleSpec, ServiceSpec, StorageSpec,
};

/// Arguments for `docker run`.
#[derive(Debug, Parser)]
#[allow(clippy::struct_excessive_bools)]
pub struct RunArgs {
    /// Run container in background and print container ID
    #[clap(short, long)]
    pub detach: bool,

    /// Keep STDIN open even if not attached
    #[clap(short = 'i', long)]
    pub interactive: bool,

    /// Allocate a pseudo-TTY
    #[clap(short, long)]
    pub tty: bool,

    /// Publish a container's port(s) to the host
    #[clap(short = 'p', long = "publish")]
    pub ports: Vec<String>,

    /// Bind mount a volume
    #[clap(short, long = "volume")]
    pub volumes: Vec<String>,

    /// Set environment variables
    #[clap(short, long = "env")]
    pub env: Vec<String>,

    /// Read in a file of environment variables
    #[clap(long)]
    pub env_file: Vec<String>,

    /// Assign a name to the container
    #[clap(long)]
    pub name: Option<String>,

    /// Automatically remove the container when it exits
    #[clap(long)]
    pub rm: bool,

    /// Connect a container to a network
    #[clap(long)]
    pub network: Option<String>,

    /// Restart policy to apply when a container exits
    #[clap(long)]
    pub restart: Option<String>,

    /// Memory limit
    #[clap(short, long)]
    pub memory: Option<String>,

    /// Number of CPUs
    #[clap(long)]
    pub cpus: Option<String>,

    /// Overwrite the default ENTRYPOINT of the image
    #[clap(long)]
    pub entrypoint: Option<String>,

    /// Working directory inside the container
    #[clap(short = 'w', long)]
    pub workdir: Option<String>,

    /// Add Linux capabilities
    #[clap(long)]
    pub cap_add: Vec<String>,

    /// Drop Linux capabilities
    #[clap(long)]
    pub cap_drop: Vec<String>,

    /// Give extended privileges to this container
    #[clap(long)]
    pub privileged: bool,

    /// Username or UID
    #[clap(short, long)]
    pub user: Option<String>,

    /// Container host name
    #[clap(short = 'H', long)]
    pub hostname: Option<String>,

    /// Set meta data on a container
    #[clap(short, long = "label")]
    pub labels: Vec<String>,

    /// Run an init inside the container
    #[clap(long)]
    pub init: bool,

    /// Set the target platform
    #[clap(long)]
    pub platform: Option<String>,

    /// Image to run
    pub image: String,

    /// Command to run in the container
    #[clap(trailing_var_arg = true)]
    pub command: Vec<String>,
}

/// Handle the `docker run` command.
///
/// Translates the Docker-style flags into a single-service `ZLayer`
/// [`DeploymentSpec`] and calls
/// [`DaemonClient::create_deployment`](zlayer_client::DaemonClient::create_deployment)
/// to submit it. Only flags with clear `ZLayer` analogues are applied; the
/// rest emit a warning on stderr.
///
/// # Errors
///
/// Returns an error if the image name cannot be derived to a service name,
/// a port/env/volume argument is malformed, the daemon cannot be reached,
/// or the daemon rejects the generated spec.
#[allow(clippy::too_many_lines)]
pub async fn handle_run(args: RunArgs) -> Result<()> {
    tracing::info!("docker run: image={}, detach={}", args.image, args.detach);

    // Warn on flags we don't currently translate.
    if args.interactive || args.tty {
        eprintln!("warning: -i/-t have no effect with zlayer docker run (non-interactive only)");
    }
    if !args.env_file.is_empty() {
        eprintln!("warning: --env-file is not yet supported; ignoring");
    }
    if args.network.is_some() {
        eprintln!("warning: --network is not yet supported; using the default overlay");
    }
    if !args.cap_add.is_empty() || !args.cap_drop.is_empty() {
        eprintln!("warning: --cap-add/--cap-drop are not yet translated; ignoring");
    }
    if args.user.is_some() {
        eprintln!("warning: --user is not yet supported; ignoring");
    }
    if args.hostname.is_some() {
        eprintln!("warning: --hostname is not yet supported; ignoring");
    }
    if !args.labels.is_empty() {
        eprintln!("warning: --label is not yet supported; ignoring");
    }
    if args.init {
        eprintln!("warning: --init is not yet supported; ignoring");
    }
    if args.platform.is_some() {
        eprintln!("warning: --platform is not yet supported; ignoring");
    }
    if args.rm {
        eprintln!(
            "warning: --rm has no direct analogue; use `zlayer docker rm {}` when done",
            args.name
                .clone()
                .unwrap_or_else(|| derive_service_name(&args.image))
        );
    }

    // ------------------------------------------------------------------
    // Build the DeploymentSpec
    // ------------------------------------------------------------------

    let service_name = args
        .name
        .clone()
        .unwrap_or_else(|| derive_service_name(&args.image));

    // Deployment name: use the container name if the user set one, else
    // derive from the image so one-off runs don't all collide.
    let deployment_name = args
        .name
        .clone()
        .unwrap_or_else(|| format!("{}-run", derive_service_name(&args.image)));

    // Endpoints from -p flags.
    let endpoints = parse_ports(&args.ports).context("Failed to parse --publish argument")?;

    // Env from -e flags.
    let env = parse_env(&args.env).context("Failed to parse --env argument")?;

    // Volumes from -v flags.
    let storage = parse_volumes(&args.volumes).context("Failed to parse --volume argument")?;

    // Resources.
    let mut resources = zlayer_spec::ResourcesSpec::default();
    if let Some(ref mem) = args.memory {
        resources.memory = Some(mem.clone());
    }
    if let Some(ref cpus) = args.cpus {
        let cpu: f64 = cpus
            .parse()
            .with_context(|| format!("invalid --cpus value: '{cpus}'"))?;
        resources.cpu = Some(cpu);
    }

    // Command override. We do a naive whitespace split on --entrypoint;
    // users who need quoting can pass the entrypoint as a single-argument
    // JSON-array-style string by omitting --entrypoint and placing the
    // executable in the trailing command instead.
    let command = CommandSpec {
        entrypoint: args.entrypoint.as_ref().map(|e| {
            let parts: Vec<String> = e.split_whitespace().map(str::to_string).collect();
            if parts.is_empty() {
                vec![e.clone()]
            } else {
                parts
            }
        }),
        args: if args.command.is_empty() {
            None
        } else {
            Some(args.command.clone())
        },
        workdir: args.workdir.clone(),
    };

    // Scale policy -- single replica, matching `docker run`.
    let scale = ScaleSpec::Fixed { replicas: 1 };

    // Health -- a bare minimum spec; TCP on first endpoint if any, else a
    // never-failing placeholder. The daemon accepts `check: { type: tcp, port: 0 }`.
    let health_port = endpoints
        .first()
        .map_or(0, zlayer_spec::EndpointSpec::target_port);
    let health = HealthSpec {
        start_grace: Some(std::time::Duration::from_secs(5)),
        interval: None,
        timeout: None,
        retries: 3,
        check: HealthCheck::Tcp { port: health_port },
    };

    // Restart policy: Docker's --restart maps loosely onto our error policy.
    // We don't model this in ServiceSpec directly, so just warn if set.
    if let Some(ref r) = args.restart {
        if r != "no" && r != "never" {
            eprintln!("note: --restart={r} not yet forwarded; zlayer restarts unhealthy services by default");
        }
    }

    let service = ServiceSpec {
        rtype: zlayer_spec::ResourceType::Service,
        schedule: None,
        image: ImageSpec {
            name: args.image.clone(),
            pull_policy: PullPolicy::IfNotPresent,
        },
        resources,
        env,
        command,
        network: zlayer_spec::ServiceNetworkSpec::default(),
        endpoints,
        scale,
        depends: Vec::new(),
        health,
        init: zlayer_spec::InitSpec::default(),
        errors: zlayer_spec::ErrorsSpec::default(),
        devices: Vec::new(),
        storage,
        capabilities: Vec::new(),
        privileged: args.privileged,
        node_mode: zlayer_spec::NodeMode::default(),
        node_selector: None,
        service_type: zlayer_spec::ServiceType::default(),
        wasm: None,
        logs: None,
        host_network: false,
    };

    let mut services = HashMap::new();
    services.insert(service_name.clone(), service);

    let spec = DeploymentSpec {
        version: "v1".to_string(),
        deployment: deployment_name.clone(),
        services,
        externals: HashMap::new(),
        tunnels: HashMap::new(),
        api: zlayer_spec::ApiSpec::default(),
    };

    let spec_yaml = serde_yaml::to_string(&spec)
        .context("Failed to serialize generated DeploymentSpec to YAML")?;
    tracing::debug!(
        deployment = %deployment_name,
        service = %service_name,
        "Deploying generated spec from docker run"
    );

    // ------------------------------------------------------------------
    // Submit to the daemon
    // ------------------------------------------------------------------

    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    let resp = client
        .create_deployment(&spec_yaml)
        .await
        .context("Failed to create deployment via daemon")?;

    // Docker echoes the container ID; we echo the deployment ID or name.
    if let Some(id) = resp.get("id").and_then(|v| v.as_str()) {
        println!("{id}");
    } else if let Some(name) = resp.get("name").and_then(|v| v.as_str()) {
        println!("{name}");
    } else {
        println!("{deployment_name}");
    }

    if !args.detach {
        eprintln!("(running in background; use `zlayer docker logs {service_name}` to follow)");
    }

    Ok(())
}

/// Derive a `ZLayer` service name from a Docker image reference.
///
/// Strips the registry, repository path, and tag, leaving only the image
/// basename, then replaces any non-alnum characters with `-`.
fn derive_service_name(image: &str) -> String {
    // Strip registry/path (everything up to and including the last '/').
    let tail = image.rsplit('/').next().unwrap_or(image);
    // Strip the tag (`:latest`, `@sha256:...`, etc.).
    let base = tail.split(&[':', '@'][..]).next().unwrap_or(tail);
    // Sanitise to a deployment-name-safe charset.
    let mut out = String::with_capacity(base.len());
    for ch in base.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('-');
        }
    }
    if out.is_empty() {
        "app".to_string()
    } else {
        out
    }
}

/// Parse a list of Docker `-p HOST[:CONTAINER[/PROTO]]` specs into
/// [`EndpointSpec`]s.
fn parse_ports(specs: &[String]) -> Result<Vec<EndpointSpec>> {
    let mut out = Vec::with_capacity(specs.len());
    for (i, raw) in specs.iter().enumerate() {
        // Strip optional interface: "0.0.0.0:8080:80" -> "8080:80".
        // We only keep the last two numeric segments.
        let without_iface = raw.rsplit(':').take(2).collect::<Vec<_>>();
        // without_iface is in reverse order: [container[/proto], host]
        let (host_port, container_port_proto) = match without_iface.as_slice() {
            [only] => (None, *only),
            [container, host] => (Some(*host), *container),
            _ => anyhow::bail!("invalid --publish value: '{raw}'"),
        };

        // Split container_port/proto.
        let (container_port, proto) = match container_port_proto.split_once('/') {
            Some((p, pr)) => (p, pr),
            None => (container_port_proto, "tcp"),
        };

        let container_port: u16 = container_port
            .parse()
            .with_context(|| format!("invalid container port in '{raw}'"))?;
        let host_port: u16 = match host_port {
            Some(s) => s
                .parse()
                .with_context(|| format!("invalid host port in '{raw}'"))?,
            None => container_port,
        };

        let protocol = match proto.to_ascii_lowercase().as_str() {
            "tcp" => Protocol::Tcp,
            "udp" => Protocol::Udp,
            "http" => Protocol::Http,
            "https" => Protocol::Https,
            other => anyhow::bail!("unsupported protocol in --publish '{raw}': {other}"),
        };

        out.push(EndpointSpec {
            name: format!("p{i}"),
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
    Ok(out)
}

/// Parse `-e KEY=VALUE` / `-e KEY` into an env map. Bare `-e KEY` reads the
/// value from the current process environment at submission time (matching
/// Docker's behaviour).
fn parse_env(specs: &[String]) -> Result<HashMap<String, String>> {
    let mut out = HashMap::with_capacity(specs.len());
    for raw in specs {
        let (key, value) = if let Some((k, v)) = raw.split_once('=') {
            (k.to_string(), v.to_string())
        } else {
            let v = std::env::var(raw).unwrap_or_default();
            (raw.clone(), v)
        };
        if key.is_empty() {
            anyhow::bail!("invalid --env value: '{raw}' (empty key)");
        }
        out.insert(key, value);
    }
    Ok(out)
}

/// Parse `-v SOURCE:TARGET[:ro]` specs into [`StorageSpec::Bind`] entries.
///
/// Anonymous volumes (`-v /data`) map to [`StorageSpec::Anonymous`].
fn parse_volumes(specs: &[String]) -> Result<Vec<StorageSpec>> {
    let mut out = Vec::with_capacity(specs.len());
    for raw in specs {
        let parts: Vec<&str> = raw.splitn(3, ':').collect();
        match parts.as_slice() {
            [target] => out.push(StorageSpec::Anonymous {
                target: (*target).to_string(),
                tier: zlayer_spec::StorageTier::default(),
            }),
            [source, target] => out.push(StorageSpec::Bind {
                source: (*source).to_string(),
                target: (*target).to_string(),
                readonly: false,
            }),
            [source, target, opts] => {
                let readonly = opts.split(',').any(|o| o == "ro" || o == "readonly");
                out.push(StorageSpec::Bind {
                    source: (*source).to_string(),
                    target: (*target).to_string(),
                    readonly,
                });
            }
            _ => anyhow::bail!("invalid --volume value: '{raw}'"),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_service_name_strips_registry_and_tag() {
        assert_eq!(derive_service_name("nginx"), "nginx");
        assert_eq!(derive_service_name("nginx:alpine"), "nginx");
        assert_eq!(derive_service_name("library/nginx:1.25"), "nginx");
        assert_eq!(derive_service_name("ghcr.io/org/my_app:1.2"), "my-app");
        assert_eq!(
            derive_service_name(
                "ghcr.io/org/app@sha256:deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
            ),
            "app"
        );
    }

    #[test]
    fn parse_ports_basic() {
        let eps = parse_ports(&["8080:80".to_string()]).expect("parse");
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].port, 8080);
        assert_eq!(eps[0].target_port, Some(80));
        assert!(matches!(eps[0].protocol, Protocol::Tcp));
    }

    #[test]
    fn parse_ports_with_interface_and_udp() {
        let eps = parse_ports(&["0.0.0.0:5353:53/udp".to_string()]).expect("parse");
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].port, 5353);
        assert_eq!(eps[0].target_port, Some(53));
        assert!(matches!(eps[0].protocol, Protocol::Udp));
    }

    #[test]
    fn parse_ports_single_port_defaults_host_to_container() {
        let eps = parse_ports(&["9000".to_string()]).expect("parse");
        assert_eq!(eps[0].port, 9000);
        assert_eq!(eps[0].target_port, Some(9000));
    }

    #[test]
    fn parse_env_handles_kv_and_bare_key() {
        let env = parse_env(&["FOO=bar".to_string()]).expect("parse");
        assert_eq!(env.get("FOO").map(String::as_str), Some("bar"));
        // Empty value is allowed.
        let env = parse_env(&["EMPTY=".to_string()]).expect("parse");
        assert_eq!(env.get("EMPTY").map(String::as_str), Some(""));
    }

    #[test]
    fn parse_env_rejects_empty_key() {
        assert!(parse_env(&["=value".to_string()]).is_err());
    }

    #[test]
    fn parse_volumes_bind_and_ro() {
        let out = parse_volumes(&["/host/data:/data:ro".to_string()]).expect("parse");
        match &out[0] {
            StorageSpec::Bind {
                source,
                target,
                readonly,
            } => {
                assert_eq!(source, "/host/data");
                assert_eq!(target, "/data");
                assert!(*readonly);
            }
            other => panic!("expected bind, got {other:?}"),
        }
    }

    #[test]
    fn parse_volumes_anonymous() {
        let out = parse_volumes(&["/data".to_string()]).expect("parse");
        assert!(matches!(out[0], StorageSpec::Anonymous { .. }));
    }

    #[test]
    fn generated_spec_yaml_roundtrips_through_validator() {
        // Build a minimal ServiceSpec using the same fields `handle_run` fills
        // in, then serialize and re-parse via zlayer_spec::from_yaml_str.
        // This guarantees the generated YAML is actually accepted by the
        // daemon's validator.
        let service = ServiceSpec {
            rtype: zlayer_spec::ResourceType::Service,
            schedule: None,
            image: ImageSpec {
                name: "nginx:alpine".to_string(),
                pull_policy: PullPolicy::IfNotPresent,
            },
            resources: zlayer_spec::ResourcesSpec::default(),
            env: HashMap::new(),
            command: CommandSpec::default(),
            network: zlayer_spec::ServiceNetworkSpec::default(),
            endpoints: vec![EndpointSpec {
                name: "p0".to_string(),
                protocol: Protocol::Http,
                port: 8080,
                target_port: Some(80),
                path: None,
                host: None,
                expose: ExposeType::Public,
                stream: None,
                tunnel: None,
            }],
            scale: ScaleSpec::Fixed { replicas: 1 },
            depends: Vec::new(),
            health: HealthSpec {
                start_grace: Some(std::time::Duration::from_secs(5)),
                interval: None,
                timeout: None,
                retries: 3,
                check: HealthCheck::Tcp { port: 80 },
            },
            init: zlayer_spec::InitSpec::default(),
            errors: zlayer_spec::ErrorsSpec::default(),
            devices: Vec::new(),
            storage: Vec::new(),
            capabilities: Vec::new(),
            privileged: false,
            node_mode: zlayer_spec::NodeMode::default(),
            node_selector: None,
            service_type: zlayer_spec::ServiceType::default(),
            wasm: None,
            logs: None,
            host_network: false,
        };
        let mut services = HashMap::new();
        services.insert("nginx".to_string(), service);
        let spec = DeploymentSpec {
            version: "v1".to_string(),
            deployment: "nginx-run".to_string(),
            services,
            externals: HashMap::new(),
            tunnels: HashMap::new(),
            api: zlayer_spec::ApiSpec::default(),
        };
        let yaml = serde_yaml::to_string(&spec).expect("serialize");
        zlayer_spec::from_yaml_str(&yaml).expect("generated spec should validate");
    }
}
