//! Docker `POST /containers/create` request body.
//!
//! Mirrors the wire shape exactly (`PascalCase`) so we can deserialize a body
//! produced by `docker run`, `docker compose up`, dockur/Winboat, Portainer,
//! testcontainers, lazydocker, the Docker Go SDK, the Python `docker`
//! library, and any other Docker-aware client.
//!
//! Reference: <https://docs.docker.com/engine/api/v1.43/#tag/Container/operation/ContainerCreate>
//!
//! These structs back `super::super::containers::create_container` (wired up
//! in sub-task 2.4.5). `build_create_request` reads the subset of fields the
//! daemon DTO understands; the remaining fields are kept on the wire shape so
//! deserialization stays lossless for clients that emit them
//! (`AttachStdin`, `Tty`, `Shell`, `OnBuild`, ...). The `dead_code` allow
//! below covers those intentionally-unconsumed fields, and the
//! `zero_sized_map_values` allow covers Docker's `{}` set-style maps
//! (`ExposedPorts`, `Volumes`).

#![allow(dead_code, clippy::zero_sized_map_values)]

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Top-level body of `POST /containers/create`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ContainerCreateBody {
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub domainname: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub attach_stdin: Option<bool>,
    #[serde(default)]
    pub attach_stdout: Option<bool>,
    #[serde(default)]
    pub attach_stderr: Option<bool>,
    #[serde(default)]
    pub exposed_ports: HashMap<String, EmptyObject>,
    #[serde(default)]
    pub tty: Option<bool>,
    #[serde(default)]
    pub open_stdin: Option<bool>,
    #[serde(default)]
    pub stdin_once: Option<bool>,
    /// Environment variables in `"KEY=VALUE"` form.
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub cmd: Option<Vec<String>>,
    #[serde(default)]
    pub healthcheck: Option<HealthcheckBody>,
    #[serde(default)]
    pub args_escaped: Option<bool>,
    /// Image reference to launch (required).
    pub image: String,
    #[serde(default)]
    pub volumes: HashMap<String, EmptyObject>,
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub entrypoint: Option<Vec<String>>,
    #[serde(default)]
    pub network_disabled: Option<bool>,
    #[serde(default)]
    pub mac_address: Option<String>,
    #[serde(default)]
    pub on_build: Option<Vec<String>>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
    #[serde(default)]
    pub stop_signal: Option<String>,
    #[serde(default)]
    pub stop_timeout: Option<i64>,
    #[serde(default)]
    pub shell: Option<Vec<String>>,
    #[serde(default)]
    pub host_config: Option<HostConfig>,
    #[serde(default)]
    pub networking_config: Option<NetworkingConfig>,
}

/// Empty object used by Docker for set-style maps (e.g. `ExposedPorts`,
/// `Volumes`).
///
/// Serde accepts any JSON object — including `{}` — for a struct with no
/// fields, so this round-trips Docker's `{ "80/tcp": {} }` shape correctly.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct EmptyObject {}

/// `Healthcheck` block on the create body.
///
/// All durations are nanoseconds (Docker's wire format); translation to
/// `std::time::Duration` happens in [`super::super::translate::healthcheck`].
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct HealthcheckBody {
    #[serde(default)]
    pub test: Vec<String>,
    /// Interval between checks, in nanoseconds.
    #[serde(default)]
    pub interval: Option<i64>,
    /// Per-check timeout, in nanoseconds.
    #[serde(default)]
    pub timeout: Option<i64>,
    #[serde(default)]
    pub retries: Option<i64>,
    /// Grace period before the first check, in nanoseconds.
    #[serde(default)]
    pub start_period: Option<i64>,
    /// Cadence during the start period, in nanoseconds.
    #[serde(default)]
    pub start_interval: Option<i64>,
}

/// `HostConfig` block of the create body.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct HostConfig {
    #[serde(default)]
    pub binds: Vec<String>,
    #[serde(default)]
    pub mounts: Vec<MountSpec>,
    #[serde(default)]
    pub network_mode: Option<String>,
    /// Map of `"<port>/<proto>"` keys to host bindings. Docker emits `null`
    /// (rather than an empty array) when a key has no host bindings — both
    /// forms are accepted here.
    #[serde(default)]
    pub port_bindings: HashMap<String, Option<Vec<PortBindingHost>>>,
    #[serde(default)]
    pub restart_policy: Option<RestartPolicy>,
    #[serde(default)]
    pub auto_remove: Option<bool>,
    #[serde(default)]
    pub volume_driver: Option<String>,
    #[serde(default)]
    pub volumes_from: Vec<String>,
    #[serde(default)]
    pub cap_add: Vec<String>,
    #[serde(default)]
    pub cap_drop: Vec<String>,
    #[serde(default)]
    pub privileged: Option<bool>,
    #[serde(default)]
    pub publish_all_ports: Option<bool>,
    #[serde(default)]
    pub readonly_rootfs: Option<bool>,
    #[serde(default)]
    pub dns: Vec<String>,
    #[serde(default)]
    pub dns_options: Vec<String>,
    #[serde(default)]
    pub dns_search: Vec<String>,
    /// `"hostname:ip"` entries appended to `/etc/hosts`.
    #[serde(default)]
    pub extra_hosts: Vec<String>,
    #[serde(default)]
    pub group_add: Vec<String>,
    #[serde(default)]
    pub ipc_mode: Option<String>,
    #[serde(default)]
    pub cgroup: Option<String>,
    #[serde(default)]
    pub cgroup_parent: Option<String>,
    #[serde(default)]
    pub links: Vec<String>,
    #[serde(default)]
    pub oom_score_adj: Option<i32>,
    #[serde(default)]
    pub pid_mode: Option<String>,
    #[serde(default)]
    pub security_opt: Vec<String>,
    #[serde(default)]
    pub storage_opt: HashMap<String, String>,
    #[serde(default)]
    pub tmpfs: HashMap<String, String>,
    #[serde(default)]
    pub uts_mode: Option<String>,
    #[serde(default)]
    pub userns_mode: Option<String>,
    #[serde(default)]
    pub shm_size: Option<i64>,
    #[serde(default)]
    pub sysctls: HashMap<String, String>,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub init: Option<bool>,
    #[serde(default)]
    pub devices: Vec<DeviceMapping>,
    #[serde(default)]
    pub ulimits: Vec<UlimitMapping>,

    // Resource fields ------------------------------------------------------
    #[serde(default)]
    pub memory: Option<i64>,
    #[serde(default)]
    pub memory_swap: Option<i64>,
    #[serde(default)]
    pub memory_reservation: Option<i64>,
    #[serde(default)]
    pub memory_swappiness: Option<i64>,
    #[serde(default)]
    pub kernel_memory_tcp: Option<i64>,
    /// Nano-CPUs (1e9 = 1 CPU).
    #[serde(default)]
    pub nano_cpus: Option<i64>,
    #[serde(default)]
    pub cpu_shares: Option<i64>,
    #[serde(default)]
    pub cpu_period: Option<i64>,
    #[serde(default)]
    pub cpu_quota: Option<i64>,
    #[serde(default)]
    pub cpu_realtime_period: Option<i64>,
    #[serde(default)]
    pub cpu_realtime_runtime: Option<i64>,
    #[serde(default)]
    pub cpuset_cpus: Option<String>,
    #[serde(default)]
    pub cpuset_mems: Option<String>,
    #[serde(default)]
    pub blkio_weight: Option<i64>,
    #[serde(default)]
    pub pids_limit: Option<i64>,
    #[serde(default)]
    pub oom_kill_disable: Option<bool>,
}

/// Object-form mount specification (`HostConfig.Mounts[]`).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct MountSpec {
    /// Container-side mount path.
    pub target: String,
    /// Source. Interpretation depends on [`Self::mount_type`]:
    /// - `"bind"`: absolute host path
    /// - `"volume"`: named-volume identifier
    /// - `"tmpfs"`: ignored
    /// - `"npipe"`: named-pipe path (Windows)
    #[serde(default)]
    pub source: String,
    /// One of `"bind"`, `"volume"`, `"tmpfs"`, `"npipe"`. Defaults to
    /// `"volume"` when missing, matching Docker.
    #[serde(rename = "Type", default = "default_mount_type")]
    pub mount_type: String,
    #[serde(default)]
    pub read_only: Option<bool>,
    #[serde(default)]
    pub consistency: Option<String>,
    #[serde(default)]
    pub bind_options: Option<BindOptions>,
    #[serde(default)]
    pub volume_options: Option<VolumeOptions>,
    #[serde(default)]
    pub tmpfs_options: Option<TmpfsOptions>,
}

fn default_mount_type() -> String {
    "volume".to_string()
}

/// Bind-mount-specific options on [`MountSpec`].
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct BindOptions {
    #[serde(default)]
    pub propagation: Option<String>,
    #[serde(default)]
    pub non_recursive: Option<bool>,
    #[serde(default)]
    pub create_mountpoint: Option<bool>,
}

/// Named-volume options on [`MountSpec`].
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct VolumeOptions {
    #[serde(default)]
    pub no_copy: Option<bool>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
    #[serde(default)]
    pub driver_config: Option<DriverConfig>,
}

/// Storage driver configuration nested under [`VolumeOptions`].
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct DriverConfig {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub options: HashMap<String, String>,
}

/// Tmpfs-specific options on [`MountSpec`].
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct TmpfsOptions {
    #[serde(default)]
    pub size_bytes: Option<i64>,
    #[serde(default)]
    pub mode: Option<u32>,
}

/// Single host binding inside [`HostConfig::port_bindings`].
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PortBindingHost {
    #[serde(default)]
    pub host_ip: Option<String>,
    /// Host port as a string. Docker uses string form on the wire (e.g.
    /// `"8080"` or `""` to request an ephemeral port).
    #[serde(default)]
    pub host_port: Option<String>,
}

/// `HostConfig.RestartPolicy`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct RestartPolicy {
    /// `"no"`, `"always"`, `"unless-stopped"`, or `"on-failure"`.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub maximum_retry_count: Option<i64>,
}

/// Single entry in `HostConfig.Devices`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct DeviceMapping {
    pub path_on_host: String,
    pub path_in_container: String,
    /// Cgroup permissions string, defaulting to `"rwm"` per Docker.
    #[serde(default)]
    pub cgroup_permissions: Option<String>,
}

/// Single entry in `HostConfig.Ulimits`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UlimitMapping {
    pub name: String,
    pub soft: i64,
    pub hard: i64,
}

/// `NetworkingConfig` block of the create body.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct NetworkingConfig {
    #[serde(default)]
    pub endpoints_config: HashMap<String, EndpointConfig>,
}

/// Per-network endpoint settings (one entry in `EndpointsConfig`).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct EndpointConfig {
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub links: Vec<String>,
    #[serde(default)]
    pub network_id: Option<String>,
    #[serde(default)]
    pub endpoint_id: Option<String>,
    #[serde(default)]
    pub gateway: Option<String>,
    #[serde(default)]
    pub ip_address: Option<String>,
    #[serde(default)]
    pub mac_address: Option<String>,
    #[serde(default)]
    pub ipam_config: Option<EndpointIpamConfig>,
}

/// IPAM overrides nested under [`EndpointConfig`].
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct EndpointIpamConfig {
    #[serde(default)]
    pub ipv4_address: Option<String>,
    #[serde(default)]
    pub ipv6_address: Option<String>,
    #[serde(default)]
    pub link_local_ips: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_body() {
        let json = r#"{"Image": "nginx"}"#;
        let body: ContainerCreateBody = serde_json::from_str(json).unwrap();
        assert_eq!(body.image, "nginx");
        assert!(body.host_config.is_none());
        assert!(body.networking_config.is_none());
    }

    #[test]
    fn parse_empty_object_in_exposed_ports_and_volumes() {
        let json = r#"{
            "Image": "nginx",
            "ExposedPorts": {"80/tcp": {}, "443/tcp": {}},
            "Volumes": {"/data": {}}
        }"#;
        let body: ContainerCreateBody = serde_json::from_str(json).unwrap();
        assert_eq!(body.exposed_ports.len(), 2);
        assert!(body.exposed_ports.contains_key("80/tcp"));
        assert!(body.exposed_ports.contains_key("443/tcp"));
        assert_eq!(body.volumes.len(), 1);
        assert!(body.volumes.contains_key("/data"));
    }

    #[test]
    fn parse_null_port_bindings() {
        // Docker emits `null` when a key has no host bindings — confirm we
        // accept it without erroring.
        let json = r#"{
            "Image": "nginx",
            "HostConfig": {"PortBindings": {"80/tcp": null}}
        }"#;
        let body: ContainerCreateBody = serde_json::from_str(json).unwrap();
        let hc = body.host_config.expect("host_config present");
        assert_eq!(hc.port_bindings.len(), 1);
        assert!(hc.port_bindings.get("80/tcp").unwrap().is_none());
    }

    #[test]
    fn parse_full_docker_run_body() {
        let json = r#"{
            "Image": "nginx",
            "Cmd": ["nginx", "-g", "daemon off;"],
            "Env": ["FOO=bar", "BAZ=qux"],
            "ExposedPorts": {"80/tcp": {}},
            "Labels": {"app": "test"},
            "HostConfig": {
                "Binds": ["/host:/container:ro"],
                "PortBindings": {"80/tcp": [{"HostIp":"","HostPort":"8080"}]},
                "RestartPolicy": {"Name":"on-failure","MaximumRetryCount":3},
                "Privileged": true,
                "CapAdd": ["NET_ADMIN"],
                "Devices": [{"PathOnHost":"/dev/kvm","PathInContainer":"/dev/kvm","CgroupPermissions":"rwm"}],
                "AutoRemove": true,
                "Memory": 536870912,
                "NanoCpus": 1000000000,
                "NetworkMode": "bridge"
            },
            "NetworkingConfig": {
                "EndpointsConfig": {"my-net": {"Aliases": ["app", "service"]}}
            }
        }"#;
        let body: ContainerCreateBody = serde_json::from_str(json).unwrap();
        assert_eq!(body.image, "nginx");
        assert_eq!(body.env.len(), 2);
        let hc = body.host_config.as_ref().unwrap();
        assert_eq!(hc.binds, vec!["/host:/container:ro".to_string()]);
        assert_eq!(hc.privileged, Some(true));
        assert_eq!(hc.cap_add, vec!["NET_ADMIN".to_string()]);
        assert_eq!(hc.devices.len(), 1);
        assert_eq!(hc.devices[0].path_on_host, "/dev/kvm");
        assert_eq!(hc.auto_remove, Some(true));
        assert_eq!(hc.memory, Some(536_870_912));
        assert_eq!(hc.nano_cpus, Some(1_000_000_000));
        assert_eq!(hc.network_mode.as_deref(), Some("bridge"));
        let rp = hc.restart_policy.as_ref().unwrap();
        assert_eq!(rp.name.as_deref(), Some("on-failure"));
        assert_eq!(rp.maximum_retry_count, Some(3));
        let aliases = &body.networking_config.as_ref().unwrap().endpoints_config["my-net"].aliases;
        assert_eq!(aliases.len(), 2);
    }

    #[test]
    fn parse_object_form_mounts() {
        let json = r#"{
            "Image": "nginx",
            "HostConfig": {
                "Mounts": [
                    {"Target":"/data","Source":"vol1","Type":"volume","ReadOnly":true,
                     "VolumeOptions":{"NoCopy":true,"Labels":{"x":"y"}}},
                    {"Target":"/host","Source":"/srv","Type":"bind",
                     "BindOptions":{"Propagation":"rprivate"}},
                    {"Target":"/tmp","Source":"","Type":"tmpfs",
                     "TmpfsOptions":{"SizeBytes":67108864,"Mode":1777}}
                ]
            }
        }"#;
        let body: ContainerCreateBody = serde_json::from_str(json).unwrap();
        let mounts = &body.host_config.as_ref().unwrap().mounts;
        assert_eq!(mounts.len(), 3);
        assert_eq!(mounts[0].mount_type, "volume");
        assert_eq!(mounts[0].read_only, Some(true));
        assert!(mounts[0].volume_options.is_some());
        assert_eq!(mounts[1].mount_type, "bind");
        assert_eq!(
            mounts[1]
                .bind_options
                .as_ref()
                .unwrap()
                .propagation
                .as_deref(),
            Some("rprivate")
        );
        assert_eq!(mounts[2].mount_type, "tmpfs");
        assert_eq!(
            mounts[2].tmpfs_options.as_ref().unwrap().size_bytes,
            Some(67_108_864)
        );
        assert_eq!(mounts[2].tmpfs_options.as_ref().unwrap().mode, Some(1777));
    }

    #[test]
    fn parse_healthcheck_block() {
        let json = r#"{
            "Image": "nginx",
            "Healthcheck": {
                "Test": ["CMD-SHELL", "curl -f http://localhost/ || exit 1"],
                "Interval": 30000000000,
                "Timeout": 5000000000,
                "Retries": 3,
                "StartPeriod": 10000000000
            }
        }"#;
        let body: ContainerCreateBody = serde_json::from_str(json).unwrap();
        let hc = body.healthcheck.as_ref().unwrap();
        assert_eq!(hc.test.len(), 2);
        assert_eq!(hc.test[0], "CMD-SHELL");
        assert_eq!(hc.interval, Some(30_000_000_000));
        assert_eq!(hc.retries, Some(3));
    }

    #[test]
    fn parse_ulimits() {
        let json = r#"{
            "Image": "nginx",
            "HostConfig": {
                "Ulimits": [{"Name":"nofile","Soft":4096,"Hard":8192}]
            }
        }"#;
        let body: ContainerCreateBody = serde_json::from_str(json).unwrap();
        let u = &body.host_config.as_ref().unwrap().ulimits;
        assert_eq!(u.len(), 1);
        assert_eq!(u[0].name, "nofile");
        assert_eq!(u[0].soft, 4096);
        assert_eq!(u[0].hard, 8192);
    }
}
