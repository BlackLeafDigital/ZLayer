//! `docker run` command implementation.
//!
//! Translates Docker `run` flags into a minimal `ZLayer` [`DeploymentSpec`]
//! and submits it to the daemon via `zlayer-client`. Flags that have no
//! `ZLayer` equivalent are logged as warnings but do not block the deploy.

use anyhow::{Context, Result};
use clap::{Args, Parser};
use std::collections::HashMap;
use std::io::Write as _;

use zlayer_client::{default_socket_path, DaemonClient};
use zlayer_spec::{
    CommandSpec, ContainerRestartKind, ContainerRestartPolicy, DeploymentSpec, EndpointSpec,
    ExposeType, HealthCheck, HealthSpec, ImageSpec, Protocol, PullPolicy, ScaleSpec, ServiceSpec,
    StorageSpec,
};

/// Arguments for `docker run`.
///
/// Mirrors the full Docker CLI `run` flag surface. Flags fall into three
/// buckets:
///
/// 1. **First-class**: directly translated into `ServiceSpec` fields
///    (e.g. `--memory`, `--cpus`, `--privileged`, `--restart`, capabilities).
/// 2. **Label-prefixed**: stashed onto `ServiceSpec.labels` under a
///    `docker.io/run/<flag>` key when no native field exists.
/// 3. **Warned**: parsed but emit a `tracing::warn!` line because there is
///    no daemon-side equivalent yet (e.g. `--gpus`, `--device-cgroup-rule`).
///    They are NEVER silently dropped.
#[derive(Debug, Parser)]
#[allow(clippy::struct_excessive_bools)]
pub struct RunArgs {
    /// Lifecycle / TTY / attach / cidfile flags
    #[command(flatten)]
    pub lifecycle: RunLifecycleArgs,

    /// Networking, DNS, expose, link, IP/MAC
    #[command(flatten)]
    pub networking: RunNetworkArgs,

    /// Volumes, mounts, tmpfs, storage, shm
    #[command(flatten)]
    pub volumes_group: RunVolumesArgs,

    /// Environment, labels, name, entrypoint, workdir, platform, pull, healthcheck
    #[command(flatten)]
    pub runtime: RunRuntimeArgs,

    /// Memory, CPU, blkio, gpus, device limits
    #[command(flatten)]
    pub resources: RunResourcesArgs,

    /// Capabilities, security, user, namespaces
    #[command(flatten)]
    pub security: RunSecurityArgs,

    /// Hostname, sysctls, ulimits, stop semantics
    #[command(flatten)]
    pub misc: RunMiscArgs,

    /// Image to run
    pub image: String,

    /// Command to run in the container. Everything after the image is
    /// passed verbatim — including flag-like tokens (`docker run alpine
    /// ls -la /` needs no `--`), matching the real docker CLI.
    #[clap(trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
}

/// Lifecycle / I/O / cidfile flags.
#[derive(Debug, Args)]
#[allow(clippy::struct_excessive_bools)]
pub struct RunLifecycleArgs {
    /// Run container in background and print container ID
    #[clap(short, long)]
    pub detach: bool,

    /// Attach to STDIN, STDOUT, or STDERR (`stdin`/`stdout`/`stderr`).
    #[clap(short = 'a', long = "attach")]
    pub attach: Vec<String>,

    /// Keep STDIN open even if not attached
    #[clap(short = 'i', long)]
    pub interactive: bool,

    /// Allocate a pseudo-TTY
    #[clap(short, long)]
    pub tty: bool,

    /// Automatically remove the container when it exits
    #[clap(long)]
    pub rm: bool,

    /// Proxy received signals to the process (default: true)
    #[clap(long, default_value_t = true)]
    pub sig_proxy: bool,

    /// Write the container ID to the given file
    #[clap(long)]
    pub cidfile: Option<String>,

    /// Skip image signature verification (no-op for compatibility).
    #[clap(long)]
    pub disable_content_trust: bool,
}

/// Networking, DNS, expose, link, addressing.
#[derive(Debug, Args)]
#[allow(clippy::struct_excessive_bools)]
pub struct RunNetworkArgs {
    /// Publish a container's port(s) to the host
    #[clap(short = 'p', long = "publish")]
    pub ports: Vec<String>,

    /// Publish all exposed ports to random host ports
    #[clap(short = 'P', long = "publish-all")]
    pub publish_all: bool,

    /// Connect a container to a network
    #[clap(long)]
    pub network: Option<String>,

    /// Add network-scoped alias for the container
    #[clap(long = "network-alias")]
    pub network_alias: Vec<String>,

    /// Add link to another container
    #[clap(long)]
    pub link: Vec<String>,

    /// Container link-local IPv4 addresses (compose-style)
    #[clap(long = "link-local-ip")]
    pub link_local_ip: Vec<String>,

    /// IPv4 address (e.g. `172.30.100.104`)
    #[clap(long)]
    pub ip: Option<String>,

    /// IPv6 address (e.g. `2001:db8::33`)
    #[clap(long)]
    pub ip6: Option<String>,

    /// Container MAC address (e.g. `92:d0:c6:0a:29:33`)
    #[clap(long = "mac-address")]
    pub mac_address: Option<String>,

    /// Expose a port without publishing it
    #[clap(long = "expose")]
    pub expose: Vec<String>,

    /// Container NIS domain name
    #[clap(long)]
    pub domainname: Option<String>,

    /// Set custom DNS servers (repeatable)
    #[clap(long)]
    pub dns: Vec<String>,

    /// Set DNS options
    #[clap(long = "dns-option")]
    pub dns_option: Vec<String>,

    /// Set DNS search domains
    #[clap(long = "dns-search")]
    pub dns_search: Vec<String>,

    /// Add a custom `host:ip` mapping to `/etc/hosts` (repeatable).
    /// Accepts the literal `host-gateway` as the ip half.
    #[clap(long = "add-host")]
    pub add_host: Vec<String>,
}

/// Volumes / mounts / shm.
#[derive(Debug, Args)]
#[allow(clippy::struct_excessive_bools)]
pub struct RunVolumesArgs {
    /// Bind mount a volume
    #[clap(short, long = "volume")]
    pub volumes: Vec<String>,

    /// Mount a volume from another container (`-v` aware)
    #[clap(long = "volumes-from")]
    pub volumes_from: Vec<String>,

    /// Driver to manage anonymous volumes
    #[clap(long = "volume-driver")]
    pub volume_driver: Option<String>,

    /// Attach a filesystem mount (Docker `--mount` long-form)
    #[clap(long = "mount")]
    pub mount: Vec<String>,

    /// Mount a tmpfs directory
    #[clap(long = "tmpfs")]
    pub tmpfs: Vec<String>,

    /// Mount the container's root filesystem as read-only
    #[clap(long = "read-only")]
    pub read_only: bool,

    /// Storage driver options for the container
    #[clap(long = "storage-opt")]
    pub storage_opt: Vec<String>,

    /// Size of `/dev/shm`
    #[clap(long = "shm-size")]
    pub shm_size: Option<String>,
}

/// Environment, labels, name, entrypoint, platform, pull, healthcheck.
#[derive(Debug, Args)]
#[allow(clippy::struct_excessive_bools)]
pub struct RunRuntimeArgs {
    /// Set environment variables (KEY=VAL). Short `-e` only: the long
    /// `--env` is reserved by the top-level `zlayer run` for selecting a
    /// `ZLayer` secret-environment to inject.
    #[clap(short = 'e')]
    pub env: Vec<String>,

    /// Read in a file of environment variables
    #[clap(long = "env-file")]
    pub env_file: Vec<String>,

    /// Assign a name to the container
    #[clap(long)]
    pub name: Option<String>,

    /// Restart policy to apply when a container exits
    #[clap(long)]
    pub restart: Option<String>,

    /// Container `entrypoint` override
    #[clap(long)]
    pub entrypoint: Option<String>,

    /// Working directory inside the container
    #[clap(short = 'w', long)]
    pub workdir: Option<String>,

    /// Set meta data on a container (repeatable)
    #[clap(short, long = "label")]
    pub labels: Vec<String>,

    /// Read in a line-delimited file of labels
    #[clap(long = "label-file")]
    pub label_file: Vec<String>,

    /// Run an init inside the container
    #[clap(long)]
    pub init: bool,

    /// Set the target platform
    #[clap(long)]
    pub platform: Option<String>,

    /// Pull image before running (`always`, `missing`, `never`)
    #[clap(long, default_value = "missing")]
    pub pull: String,

    /// Skip default healthcheck
    #[clap(long = "no-healthcheck")]
    pub no_healthcheck: bool,

    /// Override healthcheck command
    #[clap(long = "health-cmd")]
    pub health_cmd: Option<String>,

    /// Time between running healthcheck (e.g. `30s`)
    #[clap(long = "health-interval")]
    pub health_interval: Option<String>,

    /// Number of consecutive failures needed to mark unhealthy
    #[clap(long = "health-retries")]
    pub health_retries: Option<u32>,

    /// Start period for healthcheck
    #[clap(long = "health-start-period")]
    pub health_start_period: Option<String>,

    /// Time between healthchecks during start period
    #[clap(long = "health-start-interval")]
    pub health_start_interval: Option<String>,

    /// Maximum time per healthcheck run
    #[clap(long = "health-timeout")]
    pub health_timeout: Option<String>,
}

/// Memory, CPU, blkio, GPU, devices.
#[derive(Debug, Args)]
#[allow(clippy::struct_excessive_bools)]
pub struct RunResourcesArgs {
    /// Memory limit
    #[clap(short, long)]
    pub memory: Option<String>,

    /// Soft memory limit (`--memory-reservation`)
    #[clap(long = "memory-reservation")]
    pub memory_reservation: Option<String>,

    /// Total memory limit including swap (`--memory-swap`)
    #[clap(long = "memory-swap")]
    pub memory_swap: Option<String>,

    /// Container memory swappiness, 0-100 (`--memory-swappiness`)
    #[clap(long = "memory-swappiness")]
    pub memory_swappiness: Option<u8>,

    /// Kernel memory limit (deprecated by Docker but still parsed)
    #[clap(long = "kernel-memory")]
    pub kernel_memory: Option<String>,

    /// OOM-killer score adjustment
    #[clap(long = "oom-score-adj")]
    pub oom_score_adj: Option<i32>,

    /// Disable OOM killer for the container
    #[clap(long = "oom-kill-disable")]
    pub oom_kill_disable: bool,

    /// Tune host's OOM preferences
    #[clap(long = "pids-limit")]
    pub pids_limit: Option<i64>,

    /// Number of CPUs
    #[clap(long)]
    pub cpus: Option<String>,

    /// CPU period
    #[clap(long = "cpu-period")]
    pub cpu_period: Option<u64>,

    /// CPU quota
    #[clap(long = "cpu-quota")]
    pub cpu_quota: Option<i64>,

    /// CPU realtime period
    #[clap(long = "cpu-rt-period")]
    pub cpu_rt_period: Option<i64>,

    /// CPU realtime runtime
    #[clap(long = "cpu-rt-runtime")]
    pub cpu_rt_runtime: Option<i64>,

    /// CPU shares (relative weight)
    #[clap(short = 'c', long = "cpu-shares")]
    pub cpu_shares: Option<u32>,

    /// CPUs in which to allow execution
    #[clap(long = "cpuset-cpus")]
    pub cpuset_cpus: Option<String>,

    /// Memory nodes (MEMs) in which to allow execution
    #[clap(long = "cpuset-mems")]
    pub cpuset_mems: Option<String>,

    /// Block IO weight, between 10 and 1000
    #[clap(long = "blkio-weight")]
    pub blkio_weight: Option<u16>,

    /// Block IO weight (relative device weight)
    #[clap(long = "blkio-weight-device")]
    pub blkio_weight_device: Vec<String>,

    /// Limit read rate (bytes/sec) from a device
    #[clap(long = "device-read-bps")]
    pub device_read_bps: Vec<String>,

    /// Limit read rate (IO/sec) from a device
    #[clap(long = "device-read-iops")]
    pub device_read_iops: Vec<String>,

    /// Limit write rate (bytes/sec) to a device
    #[clap(long = "device-write-bps")]
    pub device_write_bps: Vec<String>,

    /// Limit write rate (IO/sec) to a device
    #[clap(long = "device-write-iops")]
    pub device_write_iops: Vec<String>,

    /// GPU devices to add to the container
    #[clap(long = "gpus")]
    pub gpus: Option<String>,

    /// Add a host device to the container
    #[clap(long = "device")]
    pub device: Vec<String>,

    /// Add a rule to the cgroup allowed devices list
    #[clap(long = "device-cgroup-rule")]
    pub device_cgroup_rule: Vec<String>,
}

/// Capabilities, security, user/namespace knobs.
#[derive(Debug, Args)]
#[allow(clippy::struct_excessive_bools)]
pub struct RunSecurityArgs {
    /// Add Linux capabilities
    #[clap(long)]
    pub cap_add: Vec<String>,

    /// Drop Linux capabilities
    #[clap(long)]
    pub cap_drop: Vec<String>,

    /// Give extended privileges to this container
    #[clap(long)]
    pub privileged: bool,

    /// Security options
    #[clap(long = "security-opt")]
    pub security_opt: Vec<String>,

    /// Username or UID
    #[clap(short, long)]
    pub user: Option<String>,

    /// Add additional groups to join
    #[clap(long = "group-add")]
    pub group_add: Vec<String>,

    /// User namespace to use
    #[clap(long = "userns")]
    pub userns: Option<String>,

    /// UTS namespace to use
    #[clap(long = "uts")]
    pub uts: Option<String>,

    /// PID namespace to use
    #[clap(long = "pid")]
    pub pid: Option<String>,

    /// IPC mode to use
    #[clap(long = "ipc")]
    pub ipc: Option<String>,

    /// Cgroup namespace to use (`host` | `private`)
    #[clap(long = "cgroupns")]
    pub cgroupns: Option<String>,

    /// Optional parent cgroup for the container
    #[clap(long = "cgroup-parent")]
    pub cgroup_parent: Option<String>,

    /// Container isolation technology (Windows only, no-op elsewhere)
    #[clap(long = "isolation")]
    pub isolation: Option<String>,

    /// Runtime to use for this container
    #[clap(long)]
    pub runtime: Option<String>,
}

/// Hostname, sysctls, ulimits, stop semantics.
#[derive(Debug, Args)]
#[allow(clippy::struct_excessive_bools)]
pub struct RunMiscArgs {
    /// Container host name
    #[clap(short = 'H', long)]
    pub hostname: Option<String>,

    /// Sysctl options
    #[clap(long = "sysctl")]
    pub sysctl: Vec<String>,

    /// Ulimit options
    #[clap(long = "ulimit")]
    pub ulimit: Vec<String>,

    /// Signal to stop the container
    #[clap(long = "stop-signal")]
    pub stop_signal: Option<String>,

    /// Timeout (in seconds) to stop the container
    #[clap(long = "stop-timeout")]
    pub stop_timeout: Option<u32>,
}

/// Internal flat view of [`RunArgs`].
///
/// `RunArgs` itself is split into several `#[command(flatten)]` groups so
/// that clap's derive emits multiple smaller `augment_args` functions
/// instead of one giant one. (The single-function variant blows the test
/// thread stack in debug builds — see the boxed `Run(Box<RunArgs>)` variant
/// in `cli::mod.rs` and the `RUST_MIN_STACK` analysis in PR #).
///
/// Internally the handler still wants a single `args.field` view, so we
/// fold the parsed groups into this flat struct on entry. No clap
/// derives here — pure data movement.
#[allow(clippy::struct_excessive_bools)]
struct FlatRunArgs {
    // lifecycle
    detach: bool,
    attach: Vec<String>,
    interactive: bool,
    tty: bool,
    // `--rm` is handled on the top-level `zlayer run` path (foreground teardown)
    // and advised on the `zlayer docker run` path in `handle_run`; the flattened
    // spec builder itself does not read it.
    #[allow(dead_code)]
    rm: bool,
    #[allow(dead_code)]
    sig_proxy: bool,
    cidfile: Option<String>,
    disable_content_trust: bool,
    // networking
    ports: Vec<String>,
    publish_all: bool,
    network: Option<String>,
    network_alias: Vec<String>,
    link: Vec<String>,
    link_local_ip: Vec<String>,
    ip: Option<String>,
    ip6: Option<String>,
    mac_address: Option<String>,
    expose: Vec<String>,
    domainname: Option<String>,
    dns: Vec<String>,
    dns_option: Vec<String>,
    dns_search: Vec<String>,
    add_host: Vec<String>,
    // volumes
    volumes: Vec<String>,
    volumes_from: Vec<String>,
    volume_driver: Option<String>,
    mount: Vec<String>,
    tmpfs: Vec<String>,
    read_only: bool,
    storage_opt: Vec<String>,
    shm_size: Option<String>,
    // runtime
    env: Vec<String>,
    env_file: Vec<String>,
    name: Option<String>,
    restart: Option<String>,
    entrypoint: Option<String>,
    workdir: Option<String>,
    labels: Vec<String>,
    label_file: Vec<String>,
    init: bool,
    platform: Option<String>,
    pull: String,
    no_healthcheck: bool,
    health_cmd: Option<String>,
    #[allow(dead_code)]
    health_interval: Option<String>,
    health_retries: Option<u32>,
    #[allow(dead_code)]
    health_start_period: Option<String>,
    #[allow(dead_code)]
    health_start_interval: Option<String>,
    #[allow(dead_code)]
    health_timeout: Option<String>,
    // resources
    memory: Option<String>,
    memory_reservation: Option<String>,
    memory_swap: Option<String>,
    memory_swappiness: Option<u8>,
    kernel_memory: Option<String>,
    oom_score_adj: Option<i32>,
    oom_kill_disable: bool,
    pids_limit: Option<i64>,
    cpus: Option<String>,
    cpu_period: Option<u64>,
    cpu_quota: Option<i64>,
    cpu_rt_period: Option<i64>,
    cpu_rt_runtime: Option<i64>,
    cpu_shares: Option<u32>,
    cpuset_cpus: Option<String>,
    cpuset_mems: Option<String>,
    blkio_weight: Option<u16>,
    blkio_weight_device: Vec<String>,
    device_read_bps: Vec<String>,
    device_read_iops: Vec<String>,
    device_write_bps: Vec<String>,
    device_write_iops: Vec<String>,
    gpus: Option<String>,
    device: Vec<String>,
    device_cgroup_rule: Vec<String>,
    // security
    cap_add: Vec<String>,
    cap_drop: Vec<String>,
    privileged: bool,
    security_opt: Vec<String>,
    user: Option<String>,
    group_add: Vec<String>,
    userns: Option<String>,
    uts: Option<String>,
    pid: Option<String>,
    ipc: Option<String>,
    cgroupns: Option<String>,
    cgroup_parent: Option<String>,
    isolation: Option<String>,
    runtime: Option<String>,
    // misc
    hostname: Option<String>,
    sysctl: Vec<String>,
    ulimit: Vec<String>,
    stop_signal: Option<String>,
    stop_timeout: Option<u32>,
    // positional
    image: String,
    command: Vec<String>,
}

impl From<RunArgs> for FlatRunArgs {
    fn from(p: RunArgs) -> Self {
        Self {
            detach: p.lifecycle.detach,
            attach: p.lifecycle.attach,
            interactive: p.lifecycle.interactive,
            tty: p.lifecycle.tty,
            rm: p.lifecycle.rm,
            sig_proxy: p.lifecycle.sig_proxy,
            cidfile: p.lifecycle.cidfile,
            disable_content_trust: p.lifecycle.disable_content_trust,
            ports: p.networking.ports,
            publish_all: p.networking.publish_all,
            network: p.networking.network,
            network_alias: p.networking.network_alias,
            link: p.networking.link,
            link_local_ip: p.networking.link_local_ip,
            ip: p.networking.ip,
            ip6: p.networking.ip6,
            mac_address: p.networking.mac_address,
            expose: p.networking.expose,
            domainname: p.networking.domainname,
            dns: p.networking.dns,
            dns_option: p.networking.dns_option,
            dns_search: p.networking.dns_search,
            add_host: p.networking.add_host,
            volumes: p.volumes_group.volumes,
            volumes_from: p.volumes_group.volumes_from,
            volume_driver: p.volumes_group.volume_driver,
            mount: p.volumes_group.mount,
            tmpfs: p.volumes_group.tmpfs,
            read_only: p.volumes_group.read_only,
            storage_opt: p.volumes_group.storage_opt,
            shm_size: p.volumes_group.shm_size,
            env: p.runtime.env,
            env_file: p.runtime.env_file,
            name: p.runtime.name,
            restart: p.runtime.restart,
            entrypoint: p.runtime.entrypoint,
            workdir: p.runtime.workdir,
            labels: p.runtime.labels,
            label_file: p.runtime.label_file,
            init: p.runtime.init,
            platform: p.runtime.platform,
            pull: p.runtime.pull,
            no_healthcheck: p.runtime.no_healthcheck,
            health_cmd: p.runtime.health_cmd,
            health_interval: p.runtime.health_interval,
            health_retries: p.runtime.health_retries,
            health_start_period: p.runtime.health_start_period,
            health_start_interval: p.runtime.health_start_interval,
            health_timeout: p.runtime.health_timeout,
            memory: p.resources.memory,
            memory_reservation: p.resources.memory_reservation,
            memory_swap: p.resources.memory_swap,
            memory_swappiness: p.resources.memory_swappiness,
            kernel_memory: p.resources.kernel_memory,
            oom_score_adj: p.resources.oom_score_adj,
            oom_kill_disable: p.resources.oom_kill_disable,
            pids_limit: p.resources.pids_limit,
            cpus: p.resources.cpus,
            cpu_period: p.resources.cpu_period,
            cpu_quota: p.resources.cpu_quota,
            cpu_rt_period: p.resources.cpu_rt_period,
            cpu_rt_runtime: p.resources.cpu_rt_runtime,
            cpu_shares: p.resources.cpu_shares,
            cpuset_cpus: p.resources.cpuset_cpus,
            cpuset_mems: p.resources.cpuset_mems,
            blkio_weight: p.resources.blkio_weight,
            blkio_weight_device: p.resources.blkio_weight_device,
            device_read_bps: p.resources.device_read_bps,
            device_read_iops: p.resources.device_read_iops,
            device_write_bps: p.resources.device_write_bps,
            device_write_iops: p.resources.device_write_iops,
            gpus: p.resources.gpus,
            device: p.resources.device,
            device_cgroup_rule: p.resources.device_cgroup_rule,
            cap_add: p.security.cap_add,
            cap_drop: p.security.cap_drop,
            privileged: p.security.privileged,
            security_opt: p.security.security_opt,
            user: p.security.user,
            group_add: p.security.group_add,
            userns: p.security.userns,
            uts: p.security.uts,
            pid: p.security.pid,
            ipc: p.security.ipc,
            cgroupns: p.security.cgroupns,
            cgroup_parent: p.security.cgroup_parent,
            isolation: p.security.isolation,
            runtime: p.security.runtime,
            hostname: p.misc.hostname,
            sysctl: p.misc.sysctl,
            ulimit: p.misc.ulimit,
            stop_signal: p.misc.stop_signal,
            stop_timeout: p.misc.stop_timeout,
            image: p.image,
            command: p.command,
        }
    }
}

impl From<&RunArgs> for FlatRunArgs {
    /// By-reference flatten used by [`build_deployment_spec`] so the spec
    /// builder can be reused (e.g. by the top-level `zlayer run` container
    /// runner) without consuming the parsed args. Every field is cloned.
    fn from(p: &RunArgs) -> Self {
        Self {
            detach: p.lifecycle.detach,
            attach: p.lifecycle.attach.clone(),
            interactive: p.lifecycle.interactive,
            tty: p.lifecycle.tty,
            rm: p.lifecycle.rm,
            sig_proxy: p.lifecycle.sig_proxy,
            cidfile: p.lifecycle.cidfile.clone(),
            disable_content_trust: p.lifecycle.disable_content_trust,
            ports: p.networking.ports.clone(),
            publish_all: p.networking.publish_all,
            network: p.networking.network.clone(),
            network_alias: p.networking.network_alias.clone(),
            link: p.networking.link.clone(),
            link_local_ip: p.networking.link_local_ip.clone(),
            ip: p.networking.ip.clone(),
            ip6: p.networking.ip6.clone(),
            mac_address: p.networking.mac_address.clone(),
            expose: p.networking.expose.clone(),
            domainname: p.networking.domainname.clone(),
            dns: p.networking.dns.clone(),
            dns_option: p.networking.dns_option.clone(),
            dns_search: p.networking.dns_search.clone(),
            add_host: p.networking.add_host.clone(),
            volumes: p.volumes_group.volumes.clone(),
            volumes_from: p.volumes_group.volumes_from.clone(),
            volume_driver: p.volumes_group.volume_driver.clone(),
            mount: p.volumes_group.mount.clone(),
            tmpfs: p.volumes_group.tmpfs.clone(),
            read_only: p.volumes_group.read_only,
            storage_opt: p.volumes_group.storage_opt.clone(),
            shm_size: p.volumes_group.shm_size.clone(),
            env: p.runtime.env.clone(),
            env_file: p.runtime.env_file.clone(),
            name: p.runtime.name.clone(),
            restart: p.runtime.restart.clone(),
            entrypoint: p.runtime.entrypoint.clone(),
            workdir: p.runtime.workdir.clone(),
            labels: p.runtime.labels.clone(),
            label_file: p.runtime.label_file.clone(),
            init: p.runtime.init,
            platform: p.runtime.platform.clone(),
            pull: p.runtime.pull.clone(),
            no_healthcheck: p.runtime.no_healthcheck,
            health_cmd: p.runtime.health_cmd.clone(),
            health_interval: p.runtime.health_interval.clone(),
            health_retries: p.runtime.health_retries,
            health_start_period: p.runtime.health_start_period.clone(),
            health_start_interval: p.runtime.health_start_interval.clone(),
            health_timeout: p.runtime.health_timeout.clone(),
            memory: p.resources.memory.clone(),
            memory_reservation: p.resources.memory_reservation.clone(),
            memory_swap: p.resources.memory_swap.clone(),
            memory_swappiness: p.resources.memory_swappiness,
            kernel_memory: p.resources.kernel_memory.clone(),
            oom_score_adj: p.resources.oom_score_adj,
            oom_kill_disable: p.resources.oom_kill_disable,
            pids_limit: p.resources.pids_limit,
            cpus: p.resources.cpus.clone(),
            cpu_period: p.resources.cpu_period,
            cpu_quota: p.resources.cpu_quota,
            cpu_rt_period: p.resources.cpu_rt_period,
            cpu_rt_runtime: p.resources.cpu_rt_runtime,
            cpu_shares: p.resources.cpu_shares,
            cpuset_cpus: p.resources.cpuset_cpus.clone(),
            cpuset_mems: p.resources.cpuset_mems.clone(),
            blkio_weight: p.resources.blkio_weight,
            blkio_weight_device: p.resources.blkio_weight_device.clone(),
            device_read_bps: p.resources.device_read_bps.clone(),
            device_read_iops: p.resources.device_read_iops.clone(),
            device_write_bps: p.resources.device_write_bps.clone(),
            device_write_iops: p.resources.device_write_iops.clone(),
            gpus: p.resources.gpus.clone(),
            device: p.resources.device.clone(),
            device_cgroup_rule: p.resources.device_cgroup_rule.clone(),
            cap_add: p.security.cap_add.clone(),
            cap_drop: p.security.cap_drop.clone(),
            privileged: p.security.privileged,
            security_opt: p.security.security_opt.clone(),
            user: p.security.user.clone(),
            group_add: p.security.group_add.clone(),
            userns: p.security.userns.clone(),
            uts: p.security.uts.clone(),
            pid: p.security.pid.clone(),
            ipc: p.security.ipc.clone(),
            cgroupns: p.security.cgroupns.clone(),
            cgroup_parent: p.security.cgroup_parent.clone(),
            isolation: p.security.isolation.clone(),
            runtime: p.security.runtime.clone(),
            hostname: p.misc.hostname.clone(),
            sysctl: p.misc.sysctl.clone(),
            ulimit: p.misc.ulimit.clone(),
            stop_signal: p.misc.stop_signal.clone(),
            stop_timeout: p.misc.stop_timeout,
            image: p.image.clone(),
            command: p.command.clone(),
        }
    }
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
pub async fn handle_run(parsed: RunArgs) -> Result<()> {
    // Capture the values we need after the deploy before the spec build
    // consumes nothing (build takes a reference). `want_attach` mirrors
    // the historical `-it` interactive-attach gate.
    let detach = parsed.lifecycle.detach;
    let want_attach = parsed.lifecycle.tty && parsed.lifecycle.interactive && !detach;

    // The `zlayer docker run` shim does not auto-remove on exit (unlike the
    // top-level `zlayer run`, which honors `--rm` client-side). Advise so the
    // container isn't left behind silently.
    if parsed.lifecycle.rm {
        tracing::warn!(
            "`zlayer docker run --rm` does not auto-remove; run `zlayer docker rm {}` when done \
             (the top-level `zlayer run --rm` does auto-remove)",
            parsed
                .runtime
                .name
                .clone()
                .unwrap_or_else(|| derive_service_name(&parsed.image))
        );
    }

    let spec = build_deployment_spec(&parsed)?;

    let deployment_name = spec.deployment.clone();
    let service_name = spec
        .services
        .keys()
        .next()
        .cloned()
        .unwrap_or_else(|| deployment_name.clone());

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
    if !want_attach {
        if let Some(id) = resp.get("id").and_then(|v| v.as_str()) {
            println!("{id}");
        } else if let Some(name) = resp.get("name").and_then(|v| v.as_str()) {
            println!("{name}");
        } else {
            println!("{deployment_name}");
        }
    }

    if want_attach {
        attach_interactive(&client, &deployment_name, &service_name).await?;
    } else if !detach {
        eprintln!("(running in background; use `zlayer docker logs {service_name}` to follow)");
    }

    Ok(())
}

/// Translate the parsed Docker `run` flags into a single-service `ZLayer`
/// [`DeploymentSpec`].
///
/// This is the spec-building half of [`handle_run`], factored out so the
/// top-level `zlayer run` container runner (which also injects `ZLayer`
/// secret-environments) can reuse the exact same translation. It emits the
/// same `tracing::warn!`/`tracing::debug!` lines for unsupported flags that
/// `handle_run` historically did, and produces an identical spec.
///
/// # Errors
///
/// Returns an error if the image name cannot be derived to a service name,
/// or a port/env/volume/label/network/restart/resource argument is malformed.
#[allow(clippy::too_many_lines)]
pub fn build_deployment_spec(parsed: &RunArgs) -> Result<DeploymentSpec> {
    // Flatten the grouped arg structs into a single `args` shape so the
    // rest of this function can keep its existing field access pattern.
    // The grouping only exists to reduce clap's debug-build stack
    // consumption when the parent CLI walks its command tree (large flat
    // structs blow the test-thread stack).
    let args = FlatRunArgs::from(parsed);
    tracing::info!("docker run: image={}, detach={}", args.image, args.detach);

    // ------------------------------------------------------------------
    // Warn on flags we accept but do not (yet) forward to the daemon.
    // Each warning identifies the flag explicitly so callers can see
    // exactly which surface area is still inert. None of these are
    // silently dropped.
    // ------------------------------------------------------------------
    if args.interactive && !args.tty {
        // Interactive stdin forwarding is wired for the `-it` (TTY) path via
        // `attach_interactive`. The non-TTY pipe case (`-i` alone, e.g.
        // `echo x | zlayer run -i ...`) is not routed through that attach loop,
        // so host stdin is not forwarded for this flag combination — use `-it`.
        tracing::warn!(
            "--interactive without --tty: stdin is forwarded only on the -it (TTY) path"
        );
    }
    if !args.attach.is_empty() {
        tracing::warn!("--attach is accepted but not yet forwarded");
    }
    if args.cidfile.is_some() {
        tracing::warn!("--cidfile is accepted but not yet forwarded");
    }
    if args.disable_content_trust {
        tracing::debug!("--disable-content-trust accepted; image trust is not enforced");
    }
    if args.publish_all {
        tracing::warn!("--publish-all is accepted but not yet forwarded");
    }
    if !args.link.is_empty() || !args.link_local_ip.is_empty() {
        tracing::warn!("--link/--link-local-ip are accepted but not yet forwarded");
    }
    if args.ip.is_some() || args.ip6.is_some() {
        tracing::warn!("--ip/--ip6 are accepted but not yet forwarded");
    }
    if args.mac_address.is_some() {
        tracing::warn!("--mac-address is accepted but not yet forwarded");
    }
    if !args.dns_option.is_empty() || !args.dns_search.is_empty() {
        tracing::warn!("--dns-option/--dns-search are accepted but not yet forwarded");
    }
    if args.domainname.is_some() {
        tracing::warn!("--domainname is accepted but not yet forwarded");
    }
    if !args.network_alias.is_empty() {
        tracing::warn!("--network-alias is accepted but not yet forwarded");
    }
    if !args.volumes_from.is_empty() {
        tracing::warn!("--volumes-from is accepted but not yet forwarded");
    }
    if args.volume_driver.is_some() {
        tracing::warn!("--volume-driver is accepted but not yet forwarded");
    }
    if !args.mount.is_empty() {
        tracing::warn!("--mount is accepted but not yet forwarded; use --volume for now");
    }
    if !args.tmpfs.is_empty() {
        tracing::warn!("--tmpfs is accepted but not yet forwarded");
    }
    if !args.storage_opt.is_empty() {
        tracing::warn!("--storage-opt is accepted but not yet forwarded");
    }
    if args.shm_size.is_some() {
        tracing::warn!("--shm-size is accepted but not yet forwarded");
    }
    if !args.env_file.is_empty() {
        tracing::warn!("--env-file is accepted but not yet forwarded; use --env for now");
    }
    if !args.label_file.is_empty() {
        tracing::warn!("--label-file is accepted but not yet forwarded; use --label for now");
    }
    if args.platform.is_some() {
        tracing::warn!("--platform is accepted but not yet forwarded");
    }
    if args.kernel_memory.is_some() {
        tracing::warn!("--kernel-memory is accepted but not yet forwarded (deprecated by Docker)");
    }
    if args.cpu_period.is_some()
        || args.cpu_quota.is_some()
        || args.cpu_rt_period.is_some()
        || args.cpu_rt_runtime.is_some()
    {
        tracing::warn!("--cpu-period/--cpu-quota/--cpu-rt-* are accepted but not yet forwarded");
    }
    if args.cpuset_mems.is_some() {
        tracing::warn!("--cpuset-mems is accepted but not yet forwarded");
    }
    if !args.blkio_weight_device.is_empty()
        || !args.device_read_bps.is_empty()
        || !args.device_read_iops.is_empty()
        || !args.device_write_bps.is_empty()
        || !args.device_write_iops.is_empty()
    {
        tracing::warn!("--device-{{read,write}}-{{bps,iops}} are accepted but not yet forwarded");
    }
    if args.gpus.is_some() {
        tracing::warn!("--gpus is accepted but not yet forwarded; use ServiceSpec.resources.gpu");
    }
    if !args.device_cgroup_rule.is_empty() {
        tracing::warn!("--device-cgroup-rule is accepted but not yet forwarded");
    }
    if !args.device.is_empty() {
        tracing::warn!("--device is accepted but not yet forwarded; use ServiceSpec.devices");
    }
    if args.uts.is_some() {
        tracing::warn!("--uts is accepted but not yet forwarded");
    }
    if args.cgroupns.is_some() {
        tracing::warn!("--cgroupns is accepted but not yet forwarded");
    }
    if args.isolation.is_some() {
        tracing::warn!("--isolation is accepted but not yet forwarded");
    }
    if args.runtime.is_some() {
        tracing::warn!("--runtime is accepted but not yet forwarded");
    }
    // NOTE: `--rm` is intentionally NOT warned about here. `build_deployment_spec`
    // is shared by the top-level `zlayer run`, which fully implements `--rm`
    // (foreground teardown + `delete_on_exit` for detached) — warning here would
    // be a false negative on that path. The `zlayer docker run` shim, which does
    // not auto-remove, emits the advisory itself in `handle_run`.
    if args.no_healthcheck {
        tracing::debug!("--no-healthcheck: emitting a placeholder TCP healthcheck on port 0");
    }
    if args.health_cmd.is_some() {
        tracing::warn!("--health-cmd is accepted but not yet forwarded as Command healthcheck");
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

    // --dns entries must each be a plausible IPv4 or IPv6 address.
    for entry in &args.dns {
        validate_dns_server(entry).context("Failed to parse --dns argument")?;
    }
    // --add-host entries must be of the form `hostname:ip` (ip can be the
    // literal `host-gateway`).
    for entry in &args.add_host {
        validate_extra_host(entry).context("Failed to parse --add-host argument")?;
    }

    // Labels: parse `KEY=VALUE` pairs. Bare `KEY` -> empty value (Docker
    // semantics). Duplicates: last write wins, matching Docker.
    let labels = parse_labels(&args.labels).context("Failed to parse --label argument")?;

    // Sysctls from --sysctl.
    let sysctls = parse_kv_pairs(&args.sysctl).context("Failed to parse --sysctl argument")?;

    // Ulimits from --ulimit (`name=soft:hard` or `name=value`).
    let ulimits = parse_ulimits(&args.ulimit).context("Failed to parse --ulimit argument")?;

    // Network mode: parse `host`, `none`, `bridge[:name]`, `container:<id>`.
    let network_mode = match args.network.as_deref() {
        Some(s) => parse_network_mode(s).context("Failed to parse --network argument")?,
        None => zlayer_spec::NetworkMode::default(),
    };

    // Pull policy from --pull.
    let pull_policy = match args.pull.to_ascii_lowercase().as_str() {
        "always" => PullPolicy::Always,
        "never" => PullPolicy::Never,
        // Docker uses "missing" as the default; we map it to IfNotPresent.
        "missing" | "if-not-present" | "ifnotpresent" | "" => PullPolicy::IfNotPresent,
        other => {
            anyhow::bail!("invalid --pull value: '{other}' (expected: always, missing, never)")
        }
    };

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
    if let Some(reservation) = &args.memory_reservation {
        resources.memory_reservation = Some(reservation.clone());
    }
    if let Some(swap) = &args.memory_swap {
        resources.memory_swap = Some(swap.clone());
    }
    if let Some(swappiness) = args.memory_swappiness {
        resources.memory_swappiness = Some(swappiness);
    }
    if let Some(score) = args.oom_score_adj {
        resources.oom_score_adj = Some(score);
    }
    if args.oom_kill_disable {
        resources.oom_kill_disable = Some(true);
    }
    if let Some(limit) = args.pids_limit {
        resources.pids_limit = Some(limit);
    }
    if let Some(shares) = args.cpu_shares {
        resources.cpu_shares = Some(shares);
    }
    if let Some(cpus) = &args.cpuset_cpus {
        resources.cpuset = Some(cpus.clone());
    }
    if let Some(weight) = args.blkio_weight {
        resources.blkio_weight = Some(weight);
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
        retries: args.health_retries.unwrap_or(3),
        check: HealthCheck::Tcp { port: health_port },
    };

    // Restart policy: Docker's --restart maps onto ContainerRestartPolicy.
    let restart_policy = match args.restart.as_deref() {
        Some(raw) => Some(parse_restart_policy(raw).context("Failed to parse --restart argument")?),
        None => None,
    };

    // Stop grace period from --stop-timeout (seconds).
    let stop_grace_period = args
        .stop_timeout
        .map(|s| std::time::Duration::from_secs(u64::from(s)));

    let service = ServiceSpec {
        rtype: zlayer_spec::ResourceType::Service,
        schedule: None,
        image: ImageSpec {
            name: args
                .image
                .parse()
                .with_context(|| format!("invalid image reference '{}'", args.image))?,
            pull_policy,
            source_policy: None,
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
        lifecycle: zlayer_spec::LifecycleSpec::default(),
        devices: Vec::new(),
        storage,
        port_mappings: Vec::new(),
        capabilities: args.cap_add.clone(),
        cap_drop: args.cap_drop.clone(),
        privileged: args.privileged,
        node_mode: zlayer_spec::NodeMode::default(),
        node_selector: None,
        affinity: None,
        service_type: zlayer_spec::ServiceType::default(),
        wasm: None,
        logs: None,
        host_network: matches!(network_mode, zlayer_spec::NetworkMode::Host),
        hostname: args.hostname.clone(),
        dns: args.dns.clone(),
        extra_hosts: args.add_host.clone(),
        restart_policy: restart_policy.clone(),
        platform: None,
        labels,
        user: args.user.clone(),
        stop_signal: args.stop_signal.clone(),
        stop_grace_period,
        sysctls,
        ulimits,
        security_opt: args.security_opt.clone(),
        pid_mode: args.pid.clone(),
        ipc_mode: args.ipc.clone(),
        network_mode,
        extra_groups: args.group_add.clone(),
        read_only_root_fs: args.read_only,
        init_container: if args.init { Some(true) } else { None },
        tty: args.tty,
        stdin_open: args.interactive,
        userns_mode: args.userns.clone(),
        cgroup_parent: args.cgroup_parent.clone(),
        expose: args.expose.clone(),
        replica_groups: None,
        isolation: None,
        overlay: None,
        localhost_reachability: zlayer_spec::LocalhostReachability::default(),
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

    Ok(spec)
}

/// Wait for the first container of `(deployment, service)` to appear and
/// return its container ID.
///
/// Polls `list_containers` until it gets a non-empty list or `timeout` is
/// reached. Returns an error if no container shows up in time.
async fn wait_for_first_container(
    client: &DaemonClient,
    deployment: &str,
    service: &str,
    timeout: std::time::Duration,
) -> Result<String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match client.list_containers(deployment, service).await {
            Ok(list) if !list.is_empty() => {
                if let Some(id) = list[0].get("id").and_then(|v| v.as_str()) {
                    return Ok(id.to_string());
                }
                if let Some(id) = list[0].get("Id").and_then(|v| v.as_str()) {
                    return Ok(id.to_string());
                }
            }
            Ok(_) => {}
            Err(e) => {
                tracing::debug!("list_containers errored while waiting for attach: {e}");
            }
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!(
                "timed out waiting for service '{service}' in deployment '{deployment}' \
                 to produce a container to attach to"
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

/// RAII guard that puts the local TTY into raw mode and restores cooked mode
/// on drop — including panic / early-return paths. Owning this for the whole
/// attach session guarantees the terminal is never left in raw mode.
struct RawModeGuard;

impl RawModeGuard {
    /// Enter raw mode, returning the guard. Restored automatically on drop.
    fn enter() -> Result<Self> {
        crossterm::terminal::enable_raw_mode().context("Failed to enable raw mode on local TTY")?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        // Best-effort restore; nothing useful to do if this fails.
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Drive the `docker run -it` interactive attach.
///
/// Sets the local terminal to raw mode, streams the container's combined
/// stdout/stderr in raw form to the host stdout, and forwards host keystrokes
/// to the container's stdin (via `POST /containers/{id}/stdin`). Ctrl-D (EOF
/// on host stdin) closes the container's stdin; Ctrl-C sends `stop_container`
/// and detaches.
///
/// Restores the terminal to cooked mode on every exit path (RAII guard).
///
/// # Errors
///
/// Returns an error if the target container never appears within the wait
/// window, the daemon log stream cannot be opened, or terminal raw-mode
/// toggling fails.
pub async fn attach_interactive(
    client: &DaemonClient,
    deployment: &str,
    service: &str,
) -> Result<()> {
    let container_id = wait_for_first_container(
        client,
        deployment,
        service,
        std::time::Duration::from_secs(30),
    )
    .await?;

    eprintln!(
        "(attached to container {} — stdin forwarded; Ctrl-D closes stdin, Ctrl-C detaches and stops)",
        &container_id[..container_id.len().min(12)]
    );

    // Hold raw mode for the whole session; the guard restores cooked mode on
    // every exit path (including the `?` early return below and any panic).
    let _raw = RawModeGuard::enter()?;

    let result = run_attach_loop(client, &container_id).await;

    // Print a trailing newline so the next shell prompt starts on its own line.
    // (Raw mode is still active here; restored when `_raw` drops on return.)
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(b"\r\n");
    let _ = stdout.flush();

    result
}

/// Inner attach loop: streams logs to stdout, waits on Ctrl-C, and on exit
/// calls `wait_container` to surface the container's exit code.
async fn run_attach_loop(client: &DaemonClient, container_id: &str) -> Result<()> {
    use futures_util::StreamExt as _;

    let mut stream = client
        .stream_container_logs(
            container_id,
            true,  // follow
            None,  // tail
            None,  // since
            None,  // until
            false, // timestamps
            true,  // stdout
            true,  // stderr
            true,  // format_raw
        )
        .await
        .context("Failed to open log stream for attach")?;

    let mut stdout = std::io::stdout();

    // Host stdin -> daemon: in raw mode we read the host terminal byte stream
    // on a dedicated blocking thread (raw `std::io::stdin` reads can't run on
    // the async reactor) and hand chunks back over a tokio channel. `None`
    // signals EOF (Ctrl-D), at which point we close the container's stdin.
    let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<Option<Vec<u8>>>(64);
    let stdin_handle = std::thread::spawn(move || {
        use std::io::Read as _;
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => {
                    // EOF (Ctrl-D): signal close and stop reading.
                    let _ = stdin_tx.blocking_send(None);
                    break;
                }
                Ok(n) => {
                    if stdin_tx.blocking_send(Some(buf[..n].to_vec())).is_err() {
                        // Receiver gone (attach loop ended): stop reading.
                        break;
                    }
                }
                Err(e) => {
                    tracing::debug!("attach stdin read error: {e}");
                    let _ = stdin_tx.blocking_send(None);
                    break;
                }
            }
        }
    });

    // Once host stdin has reached EOF (or its reader thread ended) we stop
    // polling the stdin branch so the select loop doesn't spin hot on a closed
    // channel — output streaming continues normally.
    let mut stdin_open = true;

    loop {
        tokio::select! {
            // Ctrl-C from the local terminal: ask the daemon to stop the
            // container, then break out of the loop. We do NOT wait for the
            // log stream to drain — the container's stdcopy framing may
            // never EOF cleanly on detach.
            res = tokio::signal::ctrl_c() => {
                res.context("Failed to install Ctrl-C handler")?;
                let _ = client.stop_container(container_id, Some(10)).await;
                break;
            }

            // Host keystrokes: forward each chunk to the container's stdin.
            // `Some(None)` is EOF (Ctrl-D) -> close stdin but keep streaming
            // output. `None` means the reader thread ended.
            maybe_input = stdin_rx.recv(), if stdin_open => {
                match maybe_input {
                    Some(Some(chunk)) => {
                        if let Err(e) = client.write_container_stdin(container_id, &chunk).await {
                            tracing::debug!("attach stdin forward error: {e}");
                            break;
                        }
                    }
                    Some(None) => {
                        let _ = client.close_container_stdin(container_id).await;
                        // Keep rendering output until the container exits or
                        // the user detaches; just stop forwarding stdin.
                        stdin_open = false;
                    }
                    None => {
                        // Reader thread gone; nothing more to forward.
                        stdin_open = false;
                    }
                }
            }

            // Next chunk of raw stdcopy framing from the daemon. Decode it
            // best-effort and write the payload bytes straight to stdout.
            chunk = stream.next() => {
                match chunk {
                    Some(Ok(bytes)) => {
                        for payload in decode_stdcopy_chunks(&bytes) {
                            if stdout.write_all(payload).is_err() {
                                drop(stdin_rx);
                                let _ = stdin_handle.join();
                                return Ok(());
                            }
                        }
                        let _ = stdout.flush();
                    }
                    Some(Err(e)) => {
                        tracing::debug!("attach log stream error: {e}");
                        break;
                    }
                    None => break,
                }
            }
        }
    }

    // Drop the receiver so the blocking stdin thread's next `blocking_send`
    // fails and it exits; then reap it so it doesn't linger. The thread may be
    // parked in a blocking `read` until the next keystroke — joining is
    // best-effort and we don't block the command shutdown on it.
    drop(stdin_rx);
    drop(stdin_handle);

    // Surface the container's exit code so `docker run -it` callers can
    // chain on it. We swallow errors here — a missing wait endpoint is not
    // worth failing the whole command over.
    if let Ok(resp) = client.wait_container(container_id, None).await {
        if resp.status_code != 0 {
            anyhow::bail!("container exited with status {}", resp.status_code);
        }
    }

    Ok(())
}

/// Non-TTY foreground follow: stream a one-shot container's combined
/// stdout/stderr to the host until it exits, then propagate its exit code.
///
/// Unlike [`attach_interactive`], this does NOT touch terminal raw mode —
/// it is the analogue of `docker run` (no `-it`) where output is piped
/// straight through. Stdin is not forwarded (there is nothing to forward in
/// the non-interactive case). Used by the top-level `zlayer run` container
/// runner for non-detached, non-`-it` invocations.
///
/// # Errors
///
/// Returns an error if no container appears in time, the log stream cannot
/// be opened, or the container exits with a non-zero status.
pub async fn stream_until_exit(
    client: &DaemonClient,
    deployment: &str,
    service: &str,
) -> Result<()> {
    use futures_util::StreamExt as _;

    let container_id = wait_for_first_container(
        client,
        deployment,
        service,
        std::time::Duration::from_secs(30),
    )
    .await?;

    let mut stream = client
        .stream_container_logs(
            &container_id,
            true,  // follow
            None,  // tail
            None,  // since
            None,  // until
            false, // timestamps
            true,  // stdout
            true,  // stderr
            true,  // format_raw
        )
        .await
        .context("Failed to open log stream for foreground run")?;

    let mut stdout = std::io::stdout();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => {
                for payload in decode_stdcopy_chunks(&bytes) {
                    if stdout.write_all(payload).is_err() {
                        break;
                    }
                }
                let _ = stdout.flush();
            }
            Err(e) => {
                tracing::debug!("foreground log stream error: {e}");
                break;
            }
        }
    }

    // Propagate the container's exit code.
    if let Ok(resp) = client.wait_container(&container_id, None).await {
        if resp.status_code != 0 {
            anyhow::bail!("container exited with status {}", resp.status_code);
        }
    }

    Ok(())
}

/// Best-effort decoder for Docker's stdcopy framing.
///
/// Each frame is `[stream:u8, 0,0,0, BE_u32(len), payload...]`. We slice the
/// payload out and ignore any partial trailing frame (the next chunk will
/// pick it up on its own and re-emit a full frame).
///
/// When the slice does not look like stdcopy framing at all (some daemons
/// emit raw bytes when `tty=true`), we return the input as a single payload
/// so the caller still sees output.
fn decode_stdcopy_chunks(buf: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut cursor = 0;
    let mut looked_like_frame = false;

    while cursor + 8 <= buf.len() {
        let stream = buf[cursor];
        // stdcopy: stream byte must be 0 (stdin), 1 (stdout), or 2 (stderr).
        // The next 3 bytes must be zero per the spec.
        if stream > 2 || buf[cursor + 1] != 0 || buf[cursor + 2] != 0 || buf[cursor + 3] != 0 {
            break;
        }
        let len = u32::from_be_bytes([
            buf[cursor + 4],
            buf[cursor + 5],
            buf[cursor + 6],
            buf[cursor + 7],
        ]) as usize;
        let frame_end = cursor + 8 + len;
        if frame_end > buf.len() {
            // Partial frame; bail and treat the rest as opaque next time.
            break;
        }
        looked_like_frame = true;
        out.push(&buf[cursor + 8..frame_end]);
        cursor = frame_end;
    }

    if !looked_like_frame {
        // Daemon emitted raw bytes (likely `tty=true` mode). Pass through.
        return vec![buf];
    }

    out
}

/// Parse a Docker `--restart` argument into a [`ContainerRestartPolicy`].
///
/// Accepts `no`, `always`, `unless-stopped`, `on-failure`, and
/// `on-failure:N` (where `N` is a non-negative max-attempt count). Any
/// other value is rejected with a clear error.
fn parse_restart_policy(raw: &str) -> Result<ContainerRestartPolicy> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        anyhow::bail!("--restart value cannot be empty");
    }
    // Split on the first ':' so we can take the optional `:N` tail for
    // on-failure[:N]. Anything else with a colon is rejected below.
    let (kind_str, tail) = match trimmed.split_once(':') {
        Some((k, t)) => (k, Some(t)),
        None => (trimmed, None),
    };
    let kind = match kind_str {
        "no" | "never" => ContainerRestartKind::No,
        "always" => ContainerRestartKind::Always,
        "unless-stopped" => ContainerRestartKind::UnlessStopped,
        "on-failure" => ContainerRestartKind::OnFailure,
        other => anyhow::bail!(
            "unsupported --restart value: '{other}' (expected one of: no, always, unless-stopped, on-failure[:N])"
        ),
    };
    let max_attempts = match (kind, tail) {
        (ContainerRestartKind::OnFailure, Some(n)) => Some(
            n.parse::<u32>()
                .with_context(|| format!("invalid on-failure max attempts: '{n}'"))?,
        ),
        (_, Some(t)) => {
            anyhow::bail!("--restart={kind_str} does not take a ':N' suffix (got ':{t}')")
        }
        (_, None) => None,
    };
    Ok(ContainerRestartPolicy {
        kind,
        max_attempts,
        delay: None,
    })
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
            target_role: None,
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

/// Validate a `--dns` value. Accepts IPv4 dotted-quad and IPv6 colon-hex.
fn validate_dns_server(raw: &str) -> Result<()> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        anyhow::bail!("--dns value cannot be empty");
    }
    if trimmed.parse::<std::net::IpAddr>().is_err() {
        anyhow::bail!("--dns value is not a valid IP address: '{raw}'");
    }
    Ok(())
}

/// Validate a `--add-host` value of the form `hostname:ip`.
///
/// The ip half may be the literal `host-gateway`, which bollard/Docker
/// resolve to the host-visible gateway address (see
/// `host.docker.internal:host-gateway`). Splits on the *first* colon so
/// IPv6 addresses (which themselves contain colons) can be used as-is.
fn validate_extra_host(raw: &str) -> Result<()> {
    let (host, ip) = raw.split_once(':').ok_or_else(|| {
        anyhow::anyhow!("--add-host value must be in the form 'hostname:ip': '{raw}'")
    })?;
    if host.trim().is_empty() {
        anyhow::bail!("--add-host hostname cannot be empty: '{raw}'");
    }
    if ip.trim().is_empty() {
        anyhow::bail!("--add-host ip cannot be empty: '{raw}'");
    }
    if ip == "host-gateway" {
        return Ok(());
    }
    if ip.parse::<std::net::IpAddr>().is_err() {
        anyhow::bail!("--add-host ip is not a valid address: '{raw}' (got '{ip}')");
    }
    Ok(())
}

/// Parse `--label KEY=VALUE` / `--label KEY` into a label map. Bare `KEY`
/// produces an empty value, matching Docker semantics. Empty keys are
/// rejected; the last write wins on duplicates.
fn parse_labels(specs: &[String]) -> Result<HashMap<String, String>> {
    let mut out = HashMap::with_capacity(specs.len());
    for raw in specs {
        let (key, value) = if let Some((k, v)) = raw.split_once('=') {
            (k.to_string(), v.to_string())
        } else {
            (raw.clone(), String::new())
        };
        if key.is_empty() {
            anyhow::bail!("invalid --label value: '{raw}' (empty key)");
        }
        out.insert(key, value);
    }
    Ok(out)
}

/// Parse a list of `KEY=VALUE` strings into a map. Used for `--sysctl`.
fn parse_kv_pairs(specs: &[String]) -> Result<HashMap<String, String>> {
    let mut out = HashMap::with_capacity(specs.len());
    for raw in specs {
        let (key, value) = raw
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("expected 'key=value', got '{raw}'"))?;
        if key.is_empty() {
            anyhow::bail!("invalid value: '{raw}' (empty key)");
        }
        out.insert(key.to_string(), value.to_string());
    }
    Ok(out)
}

/// Parse `--ulimit name=soft[:hard]` entries into a [`UlimitSpec`] map.
///
/// Accepts both `name=value` (soft and hard share the value) and
/// `name=soft:hard`. Values may be `-1` for "unlimited" (per Docker
/// semantics). Empty names are rejected.
fn parse_ulimits(specs: &[String]) -> Result<HashMap<String, zlayer_spec::UlimitSpec>> {
    let mut out = HashMap::with_capacity(specs.len());
    for raw in specs {
        let (name, vals) = raw
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--ulimit must be 'name=soft[:hard]', got '{raw}'"))?;
        if name.is_empty() {
            anyhow::bail!("--ulimit name cannot be empty: '{raw}'");
        }
        let (soft_s, hard_s) = match vals.split_once(':') {
            Some((s, h)) => (s, h),
            None => (vals, vals),
        };
        let soft: i64 = soft_s
            .parse()
            .with_context(|| format!("invalid --ulimit soft value in '{raw}': '{soft_s}'"))?;
        let hard: i64 = hard_s
            .parse()
            .with_context(|| format!("invalid --ulimit hard value in '{raw}': '{hard_s}'"))?;
        out.insert(name.to_string(), zlayer_spec::UlimitSpec { soft, hard });
    }
    Ok(out)
}

/// Parse Docker's `--network` argument into a [`NetworkMode`].
///
/// Accepts `host`, `none`, `bridge`, `bridge:<name>`, `container:<id>`,
/// and `default`. Anything else (custom network names) maps to a
/// `Bridge { name: Some(<value>) }` entry, matching Docker's behaviour
/// of treating unknown network names as user-defined bridges.
fn parse_network_mode(raw: &str) -> Result<zlayer_spec::NetworkMode> {
    use zlayer_spec::NetworkMode;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        anyhow::bail!("--network value cannot be empty");
    }
    if let Some(rest) = trimmed.strip_prefix("container:") {
        if rest.is_empty() {
            anyhow::bail!("--network container:<id> requires an id");
        }
        return Ok(NetworkMode::Container {
            id: rest.to_string(),
        });
    }
    if let Some(rest) = trimmed.strip_prefix("bridge:") {
        return Ok(NetworkMode::Bridge {
            name: if rest.is_empty() {
                None
            } else {
                Some(rest.to_string())
            },
        });
    }
    Ok(match trimmed {
        "host" => NetworkMode::Host,
        "none" => NetworkMode::None,
        "default" => NetworkMode::Default,
        "bridge" => NetworkMode::Bridge { name: None },
        // Treat any other identifier as a user-defined bridge name, matching
        // Docker's "named network" semantics.
        other => NetworkMode::Bridge {
            name: Some(other.to_string()),
        },
    })
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
                name: "nginx:alpine".parse().expect("valid image reference"),
                pull_policy: PullPolicy::IfNotPresent,
                source_policy: None,
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
                target_role: None,
            }],
            scale: ScaleSpec::Fixed { replicas: 1 },
            replica_groups: None,
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
            lifecycle: zlayer_spec::LifecycleSpec::default(),
            devices: Vec::new(),
            storage: Vec::new(),
            port_mappings: Vec::new(),
            capabilities: Vec::new(),
            cap_drop: Vec::new(),
            privileged: false,
            node_mode: zlayer_spec::NodeMode::default(),
            node_selector: None,
            affinity: None,
            service_type: zlayer_spec::ServiceType::default(),
            wasm: None,
            logs: None,
            host_network: false,
            hostname: None,
            dns: Vec::new(),
            extra_hosts: Vec::new(),
            restart_policy: None,
            platform: None,
            labels: HashMap::new(),
            user: None,
            stop_signal: None,
            stop_grace_period: None,
            sysctls: HashMap::new(),
            ulimits: HashMap::new(),
            security_opt: Vec::new(),
            pid_mode: None,
            ipc_mode: None,
            network_mode: zlayer_spec::NetworkMode::default(),
            extra_groups: Vec::new(),
            read_only_root_fs: false,
            init_container: None,
            tty: false,
            stdin_open: false,
            userns_mode: None,
            cgroup_parent: None,
            expose: Vec::new(),
            isolation: None,
            overlay: None,
            localhost_reachability: zlayer_spec::LocalhostReachability::default(),
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

    #[test]
    fn validate_dns_server_accepts_v4_and_v6() {
        validate_dns_server("8.8.8.8").expect("v4 is valid");
        validate_dns_server("2001:4860:4860::8888").expect("v6 is valid");
    }

    #[test]
    fn validate_dns_server_rejects_garbage() {
        assert!(validate_dns_server("").is_err());
        assert!(validate_dns_server("not-an-ip").is_err());
        assert!(validate_dns_server("999.999.999.999").is_err());
    }

    #[test]
    fn validate_extra_host_accepts_plain_ip_and_host_gateway() {
        validate_extra_host("host.docker.internal:host-gateway").expect("host-gateway literal");
        validate_extra_host("myhost:10.0.0.1").expect("ipv4");
        validate_extra_host("ipv6host:fe80::1").expect("ipv6");
    }

    #[test]
    fn validate_extra_host_rejects_malformed() {
        assert!(validate_extra_host("no-colon").is_err());
        assert!(validate_extra_host(":10.0.0.1").is_err()); // empty host
        assert!(validate_extra_host("host:").is_err()); // empty ip
        assert!(validate_extra_host("host:not-an-ip").is_err());
    }

    // ----------------------------------------------------------------------
    // Task 4.3.2: cap_add / cap_drop / --restart wiring + flag parsing.
    // ----------------------------------------------------------------------

    /// Walk a representative `docker run` invocation through `RunArgs::parse`
    /// and confirm the parsed flags land in the right places. Crucially:
    ///
    ///   * `--cap-add` / `--cap-drop` are repeatable and survive parsing.
    ///   * `--hostname` works alongside `--help` (no `-h` collision).
    ///   * `--restart on-failure:5` parses without any clap `short`
    ///     ambiguity.
    #[test]
    fn run_args_parses_cap_and_restart_flags() {
        let args = RunArgs::try_parse_from([
            "run",
            "--cap-add",
            "NET_ADMIN",
            "--cap-add",
            "SYS_PTRACE",
            "--cap-drop",
            "NET_RAW",
            "--restart",
            "on-failure:5",
            "--hostname",
            "myhost",
            "alpine",
            "sh",
            "-lc",
            "echo hi",
        ])
        .expect("parse");
        assert_eq!(args.security.cap_add, vec!["NET_ADMIN", "SYS_PTRACE"]);
        assert_eq!(args.security.cap_drop, vec!["NET_RAW"]);
        assert_eq!(args.runtime.restart.as_deref(), Some("on-failure:5"));
        assert_eq!(args.misc.hostname.as_deref(), Some("myhost"));
        assert_eq!(args.image, "alpine");
        assert_eq!(args.command, vec!["sh", "-lc", "echo hi"]);
    }

    /// `-h` MUST belong to clap's auto-`--help`, not to `--hostname`. Before
    /// task 4.3.2 the field used `short = 'h'`, which made `docker run -h
    /// myhost ...` print help and exit instead of setting the hostname. The
    /// hostname now uses `-H`, so `-h` is free again.
    #[test]
    fn run_args_help_flag_not_shadowed_by_hostname() {
        // `-h` should produce clap's help error, NOT consume `myhost` as a
        // hostname value.
        let err = RunArgs::try_parse_from(["run", "-h"]).expect_err("`-h` must invoke help");
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);

        // `-H` is the explicit hostname short flag.
        let args = RunArgs::try_parse_from(["run", "-H", "myhost", "alpine"]).expect("parse");
        assert_eq!(args.misc.hostname.as_deref(), Some("myhost"));
    }

    /// `parse_restart_policy` covers Docker's full grammar. Each of the
    /// supported kinds maps onto the matching `ContainerRestartKind`, and
    /// `on-failure[:N]` carries `N` through as `max_attempts`.
    #[test]
    fn parse_restart_policy_accepts_all_docker_kinds() {
        let p = parse_restart_policy("no").expect("no");
        assert_eq!(p.kind, ContainerRestartKind::No);
        assert!(p.max_attempts.is_none());

        let p = parse_restart_policy("always").expect("always");
        assert_eq!(p.kind, ContainerRestartKind::Always);

        let p = parse_restart_policy("unless-stopped").expect("unless-stopped");
        assert_eq!(p.kind, ContainerRestartKind::UnlessStopped);

        let p = parse_restart_policy("on-failure").expect("on-failure");
        assert_eq!(p.kind, ContainerRestartKind::OnFailure);
        assert!(p.max_attempts.is_none());

        let p = parse_restart_policy("on-failure:5").expect("on-failure:5");
        assert_eq!(p.kind, ContainerRestartKind::OnFailure);
        assert_eq!(p.max_attempts, Some(5));
    }

    /// Bogus restart values must error out with the parsing context attached.
    #[test]
    fn parse_restart_policy_rejects_garbage() {
        assert!(parse_restart_policy("").is_err());
        assert!(parse_restart_policy("sometimes").is_err());
        // ':N' is only legal on on-failure.
        assert!(parse_restart_policy("always:3").is_err());
        // Non-integer attempt count.
        assert!(parse_restart_policy("on-failure:not-a-number").is_err());
    }

    /// stdcopy framing decodes one or more well-formed frames and skips a
    /// trailing partial frame without panicking.
    #[test]
    fn decode_stdcopy_chunks_recovers_payloads() {
        // Build two full frames: stdout "hi" and stderr "ERR".
        let mut buf = Vec::new();
        // Frame 1: stream=1 (stdout), len=2, payload "hi"
        buf.extend_from_slice(&[1, 0, 0, 0]);
        buf.extend_from_slice(&2u32.to_be_bytes());
        buf.extend_from_slice(b"hi");
        // Frame 2: stream=2 (stderr), len=3, payload "ERR"
        buf.extend_from_slice(&[2, 0, 0, 0]);
        buf.extend_from_slice(&3u32.to_be_bytes());
        buf.extend_from_slice(b"ERR");
        // Trailing partial frame (header only): must be ignored.
        buf.extend_from_slice(&[1, 0, 0, 0]);
        buf.extend_from_slice(&99u32.to_be_bytes());

        let payloads = decode_stdcopy_chunks(&buf);
        assert_eq!(payloads.len(), 2);
        assert_eq!(payloads[0], b"hi");
        assert_eq!(payloads[1], b"ERR");
    }

    /// When the buffer doesn't even start with stdcopy framing (TTY mode
    /// emits raw bytes), the decoder passes the whole buffer through as a
    /// single chunk so the caller doesn't drop output.
    #[test]
    fn decode_stdcopy_chunks_passes_through_non_framed_input() {
        let raw = b"plain text from a tty stream";
        let payloads = decode_stdcopy_chunks(raw);
        assert_eq!(payloads, vec![raw.as_slice()]);
    }

    // ----------------------------------------------------------------------
    // Task 8.3.x: expanded `docker run` flag parsing tests.
    // ----------------------------------------------------------------------

    /// Parse `--label` into a map. Bare keys become empty values, duplicates
    /// last-write-wins, empty keys are rejected.
    #[test]
    fn parse_labels_basic() {
        let m = parse_labels(&[
            "app=web".to_string(),
            "owner".to_string(),
            "app=api".to_string(),
        ])
        .expect("parse");
        assert_eq!(m.get("app").map(String::as_str), Some("api"));
        assert_eq!(m.get("owner").map(String::as_str), Some(""));
        assert!(parse_labels(&["=value".to_string()]).is_err());
    }

    /// `--sysctl` requires explicit `=`.
    #[test]
    fn parse_kv_pairs_requires_equals() {
        let m = parse_kv_pairs(&["net.ipv4.ip_forward=1".to_string()]).expect("parse");
        assert_eq!(m.get("net.ipv4.ip_forward").map(String::as_str), Some("1"));
        assert!(parse_kv_pairs(&["bad-no-equals".to_string()]).is_err());
    }

    /// `--ulimit` accepts both `name=value` and `name=soft:hard`. Values may
    /// be `-1` for "unlimited".
    #[test]
    fn parse_ulimits_value_and_pair() {
        let m = parse_ulimits(&["nofile=1024:65536".to_string()]).expect("parse");
        let nf = m.get("nofile").expect("nofile");
        assert_eq!(nf.soft, 1024);
        assert_eq!(nf.hard, 65536);

        let m = parse_ulimits(&["memlock=-1".to_string()]).expect("parse");
        let ml = m.get("memlock").expect("memlock");
        assert_eq!(ml.soft, -1);
        assert_eq!(ml.hard, -1);

        assert!(parse_ulimits(&["bad-value".to_string()]).is_err());
        assert!(parse_ulimits(&["=missing".to_string()]).is_err());
        assert!(parse_ulimits(&["nofile=oops".to_string()]).is_err());
    }

    /// `--network` parsing: `host`, `none`, `bridge`, `bridge:name`,
    /// `container:<id>`, and arbitrary names map to user-defined bridges.
    #[test]
    fn parse_network_mode_matches_docker_strings() {
        use zlayer_spec::NetworkMode;
        assert!(matches!(
            parse_network_mode("host").unwrap(),
            NetworkMode::Host
        ));
        assert!(matches!(
            parse_network_mode("none").unwrap(),
            NetworkMode::None
        ));
        assert!(matches!(
            parse_network_mode("default").unwrap(),
            NetworkMode::Default
        ));
        assert!(matches!(
            parse_network_mode("bridge").unwrap(),
            NetworkMode::Bridge { name: None }
        ));
        match parse_network_mode("bridge:my-net").unwrap() {
            NetworkMode::Bridge { name: Some(n) } => assert_eq!(n, "my-net"),
            other => panic!("expected Bridge(my-net), got {other:?}"),
        }
        match parse_network_mode("container:abc123").unwrap() {
            NetworkMode::Container { id } => assert_eq!(id, "abc123"),
            other => panic!("expected Container, got {other:?}"),
        }
        // Unknown name -> user-defined bridge.
        match parse_network_mode("my-overlay").unwrap() {
            NetworkMode::Bridge { name: Some(n) } => assert_eq!(n, "my-overlay"),
            other => panic!("expected Bridge(my-overlay), got {other:?}"),
        }
        assert!(parse_network_mode("").is_err());
        assert!(parse_network_mode("container:").is_err());
    }

    /// Verify the new flag surface parses end-to-end without colliding
    /// with existing short/long flags.
    #[test]
    fn run_args_parses_full_flag_surface() {
        let args = RunArgs::try_parse_from([
            "run",
            "--memory",
            "512m",
            "--memory-reservation",
            "256m",
            "--memory-swap",
            "1g",
            "--memory-swappiness",
            "30",
            "--cpus",
            "1.5",
            "--cpu-shares",
            "1024",
            "--cpuset-cpus",
            "0-3",
            "--blkio-weight",
            "500",
            "--pids-limit",
            "100",
            "--label",
            "app=web",
            "--sysctl",
            "net.ipv4.ip_forward=1",
            "--ulimit",
            "nofile=1024:65536",
            "--security-opt",
            "no-new-privileges:true",
            "--read-only",
            "--init",
            "--user",
            "1000:1000",
            "--stop-signal",
            "SIGTERM",
            "--stop-timeout",
            "30",
            "--network",
            "host",
            "--expose",
            "8080",
            "--pull",
            "always",
            "--health-retries",
            "5",
            "alpine",
        ])
        .expect("parse");
        assert_eq!(args.resources.memory.as_deref(), Some("512m"));
        assert_eq!(args.resources.memory_reservation.as_deref(), Some("256m"));
        assert_eq!(args.resources.memory_swap.as_deref(), Some("1g"));
        assert_eq!(args.resources.memory_swappiness, Some(30));
        assert_eq!(args.resources.cpus.as_deref(), Some("1.5"));
        assert_eq!(args.resources.cpu_shares, Some(1024));
        assert_eq!(args.resources.cpuset_cpus.as_deref(), Some("0-3"));
        assert_eq!(args.resources.blkio_weight, Some(500));
        assert_eq!(args.resources.pids_limit, Some(100));
        assert_eq!(args.runtime.labels, vec!["app=web"]);
        assert_eq!(args.misc.sysctl, vec!["net.ipv4.ip_forward=1"]);
        assert_eq!(args.misc.ulimit, vec!["nofile=1024:65536"]);
        assert_eq!(args.security.security_opt, vec!["no-new-privileges:true"]);
        assert!(args.volumes_group.read_only);
        assert!(args.runtime.init);
        assert_eq!(args.security.user.as_deref(), Some("1000:1000"));
        assert_eq!(args.misc.stop_signal.as_deref(), Some("SIGTERM"));
        assert_eq!(args.misc.stop_timeout, Some(30));
        assert_eq!(args.networking.network.as_deref(), Some("host"));
        assert_eq!(args.networking.expose, vec!["8080"]);
        assert_eq!(args.runtime.pull, "always");
        assert_eq!(args.runtime.health_retries, Some(5));
        assert_eq!(args.image, "alpine");
    }

    /// `--pull` defaults to `missing` and accepts the documented values.
    #[test]
    fn run_args_pull_default_and_values() {
        let args = RunArgs::try_parse_from(["run", "alpine"]).expect("parse");
        assert_eq!(args.runtime.pull, "missing");

        for v in ["always", "missing", "never"] {
            let args = RunArgs::try_parse_from(["run", "--pull", v, "alpine"]).expect("parse");
            assert_eq!(args.runtime.pull, v);
        }
    }
}
