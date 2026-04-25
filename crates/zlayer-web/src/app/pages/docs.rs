//! Documentation page component

use leptos::prelude::*;

use crate::app::components::{Footer, Navbar};

/// Documentation page component
#[component]
#[allow(clippy::too_many_lines)]
pub fn DocsPage() -> impl IntoView {
    view! {
        <div class="page-layout">
            <Navbar/>

            <main class="main-content docs-page">
                <div class="container docs-content">
                    <h1>"ZLayer Documentation"</h1>

                    <p>
                        "Welcome to the ZLayer documentation. ZLayer is a lightweight container "
                        "orchestration system designed for simplicity and security."
                    </p>

                    <h2 id="getting-started">"Getting Started"</h2>

                    <h3>"Installation"</h3>
                    <p>
                        "ZLayer ships as a single static binary. It runs natively on Linux "
                        "(kernel 5.4+), macOS 13+ Ventura, and Windows 10 21H2+ / Server 2019+. "
                        "On Windows, ZLayer uses the Host Compute Service (HCS) native runtime "
                        "for Windows containers, with optional WSL2 support for Linux workloads."
                    </p>

                    <h4>"Linux / macOS"</h4>
                    <pre><code>"# Quick install
curl -fsSL https://zlayer.dev/install.sh | bash

# Verify
zlayer --version"</code></pre>

                    <h4>"Windows (PowerShell)"</h4>
                    <pre><code>"# Quick install
irm https://zlayer.dev/install.ps1 | iex

# Verify
zlayer --version"</code></pre>

                    <h4>"From GitHub Releases (manual)"</h4>
                    <p>
                        "Prebuilt artifacts are published on the "
                        <a href="https://github.com/BlackLeafDigital/ZLayer/releases" target="_blank" rel="noopener noreferrer">"GitHub Releases"</a>
                        " page for every tagged version:"
                    </p>
                    <ul>
                        <li><code>"zlayer-linux-amd64.tar.gz"</code></li>
                        <li><code>"zlayer-linux-arm64.tar.gz"</code></li>
                        <li><code>"zlayer-darwin-arm64.tar.gz"</code></li>
                        <li><code>"zlayer-darwin-amd64.tar.gz"</code></li>
                        <li><code>"zlayer-windows-amd64.zip"</code></li>
                    </ul>
                    <p>
                        "Download the archive that matches your platform, extract the "
                        <code>"zlayer"</code>" binary, and place it on your "<code>"PATH"</code>"."
                    </p>

                    <h3>"Your First Container"</h3>
                    <p>
                        "Create a simple container specification in "<code>"container.yaml"</code>":"
                    </p>

                    <pre><code>"apiVersion: zlayer.dev/v1
kind: Container
metadata:
  name: hello-world
spec:
  image: docker.io/library/alpine:latest
  command: [\"echo\", \"Hello from ZLayer!\"]"</code></pre>

                    <p>"Run the container:"</p>
                    <pre><code>"zlayer run container.yaml"</code></pre>

                    <h2 id="concepts">"Core Concepts"</h2>

                    <h3>"Containers"</h3>
                    <p>
                        "Containers are the basic unit of execution in ZLayer. Each container "
                        "runs in an isolated environment with its own filesystem, network namespace, "
                        "and process tree."
                    </p>

                    <h3>"Pods"</h3>
                    <p>
                        "Pods group containers that need to share resources and communicate via "
                        "localhost. All containers in a pod share the same network namespace and "
                        "can access shared volumes."
                    </p>

                    <h3>"Overlay Networks"</h3>
                    <p>
                        "ZLayer provides built-in encrypted overlay networking. This allows "
                        "containers on different hosts to communicate securely without complex "
                        "network configuration."
                    </p>

                    <h3>"Scheduler"</h3>
                    <p>
                        "The ZLayer scheduler distributes containers across available nodes based "
                        "on resource requirements, constraints, and affinity rules. It supports "
                        "both immediate and queued scheduling modes."
                    </p>

                    <h2 id="api">"API Reference"</h2>

                    <h3>"Container Spec"</h3>
                    <p>"The container specification supports the following fields:"</p>
                    <ul>
                        <li><code>"image"</code>" - OCI image reference (required)"</li>
                        <li><code>"command"</code>" - Override the image entrypoint"</li>
                        <li><code>"args"</code>" - Arguments to pass to the command"</li>
                        <li><code>"env"</code>" - Environment variables"</li>
                        <li><code>"resources"</code>" - CPU and memory limits"</li>
                        <li><code>"network"</code>" - Network configuration"</li>
                        <li><code>"volumes"</code>" - Volume mounts"</li>
                        <li><code>"securityContext"</code>" - Security settings"</li>
                    </ul>

                    <h3>"Deployment Spec"</h3>
                    <p>
                        "Beyond raw containers, ZLayer ships a richer deployment spec used by "
                        <code>"zlayer deploy"</code>". A deployment is one of three kinds:"
                    </p>
                    <ul>
                        <li><code>"kind: Service"</code>" - long-running workloads (web servers, APIs, workers)"</li>
                        <li><code>"kind: Job"</code>" - run-to-completion tasks"</li>
                        <li><code>"kind: Cron"</code>" - scheduled jobs on a cron expression"</li>
                    </ul>
                    <p>"Deployments support:"</p>
                    <ul>
                        <li>
                            <code>"scaling: { mode: adaptive | fixed | manual }"</code>
                            " - automatic scaling driven by load, fixed replica counts, or manual control"
                        </li>
                        <li><code>"healthChecks"</code>" - liveness and readiness probes (HTTP, TCP, exec)"</li>
                        <li><code>"initActions"</code>" - pre-start lifecycle hooks (TCP/HTTP wait, S3 sync, custom commands)"</li>
                        <li><code>"storage"</code>" - persistent volumes backed by local disk or S3"</li>
                        <li><code>"tunnel"</code>" - optional secure tunneling for node-to-node access"</li>
                        <li><code>"os: linux | windows"</code>" - target operating system for the workload"</li>
                    </ul>

                    <h3>"CLI Commands"</h3>
                    <p>
                        "ZLayer is a single binary with a broad command surface. Commands are "
                        "grouped below by category. Run "<code>"zlayer --help"</code>" or "
                        <code>"zlayer <command> --help"</code>" for the full reference."
                    </p>

                    <h4>"Lifecycle"</h4>
                    <ul>
                        <li><code>"zlayer up"</code>" / "<code>"zlayer down"</code>" - bring a deployment up or tear it down"</li>
                        <li><code>"zlayer deploy"</code>" - apply a deployment spec"</li>
                        <li><code>"zlayer join"</code>" - join an existing cluster"</li>
                        <li><code>"zlayer ps"</code>" / "<code>"zlayer status"</code>" - list workloads or inspect cluster state"</li>
                        <li><code>"zlayer exec"</code>" / "<code>"zlayer logs"</code>" - exec into a workload or stream its logs"</li>
                        <li><code>"zlayer stop"</code>" / "<code>"zlayer run"</code>" / "<code>"zlayer validate"</code>" - control or validate a spec"</li>
                    </ul>

                    <h4>"Build & Image"</h4>
                    <ul>
                        <li><code>"zlayer build"</code>" - build an OCI image from a Dockerfile or ZImagefile"</li>
                        <li><code>"zlayer pipeline"</code>" - run a multi-image ZPipeline build"</li>
                        <li><code>"zlayer runtimes"</code>" - list bundled runtime templates (node, python, rust, go)"</li>
                        <li><code>"zlayer pull"</code>" / "<code>"zlayer import"</code>" / "<code>"zlayer export"</code>" - move images in and out of the local store"</li>
                        <li><code>"zlayer image ls | inspect | rm"</code>" - manage the local image cache"</li>
                        <li><code>"zlayer wasm build | export | push | validate"</code>" - WebAssembly component workflows"</li>
                    </ul>

                    <h4>"Cluster & Node"</h4>
                    <ul>
                        <li><code>"zlayer node init | join | list | status | remove"</code>" - manage cluster nodes"</li>
                        <li><code>"zlayer network ls | inspect | create | rm"</code>" - manage overlay networks"</li>
                        <li><code>"zlayer volume ls | rm"</code>" - manage persistent volumes"</li>
                    </ul>

                    <h4>"Server & Daemon"</h4>
                    <ul>
                        <li><code>"zlayer serve"</code>" - run the API server (add "<code>"--daemon"</code>" to background it)"</li>
                        <li>
                            <code>"zlayer daemon install | start | stop | status"</code>
                            " - cross-platform service installer (systemd on Linux, launchd on macOS, Windows SCM on Windows)"
                        </li>
                        <li>
                            <code>"zlayer windows compact"</code>
                            " - Windows-only: compact the WSL2 VHDX backing file to reclaim disk space"
                        </li>
                    </ul>

                    <h4>"Resources"</h4>
                    <ul>
                        <li><code>"zlayer secret ls | create | get | rm | rotate"</code>" - secrets management"</li>
                        <li><code>"zlayer env"</code>" / "<code>"zlayer variable"</code>" - environment and variable management"</li>
                        <li><code>"zlayer task ls | run"</code>" / "<code>"zlayer workflow ls | run"</code>" - tasks and workflows"</li>
                        <li><code>"zlayer notifier ls | test"</code>" - notification channels"</li>
                        <li><code>"zlayer job ls | trigger"</code>" - job scheduling"</li>
                        <li><code>"zlayer container ls | logs | stats"</code>" - low-level container introspection"</li>
                    </ul>

                    <h4>"Auth & RBAC"</h4>
                    <ul>
                        <li><code>"zlayer auth login | whoami"</code>" - authenticate against the API"</li>
                        <li><code>"zlayer token create | decode"</code>" - issue and inspect API tokens"</li>
                        <li><code>"zlayer user"</code>" / "<code>"zlayer group"</code>" - user and group administration"</li>
                        <li><code>"zlayer permission list | grant"</code>" - role-based access control"</li>
                        <li><code>"zlayer audit tail"</code>" - stream the audit log"</li>
                    </ul>

                    <h4>"Projects & GitOps"</h4>
                    <ul>
                        <li><code>"zlayer project ..."</code>" - manage projects"</li>
                        <li><code>"zlayer credential registry | git"</code>" - registry and Git credentials"</li>
                        <li><code>"zlayer sync ls | create | diff | apply"</code>" - GitOps-style sync from a Git repo"</li>
                    </ul>

                    <h4>"Tunneling"</h4>
                    <ul>
                        <li><code>"zlayer tunnel create | list | connect"</code>" - secure node-to-node tunnels"</li>
                    </ul>

                    <h4>"Manager UI"</h4>
                    <ul>
                        <li><code>"zlayer manager init | status | stop"</code>" - install and control the management UI"</li>
                    </ul>

                    <h4>"Docker Compatibility"</h4>
                    <ul>
                        <li><code>"zlayer docker install"</code>" / "<code>"zlayer docker uninstall"</code>" - install or remove the Docker shim"</li>
                        <li>
                            <code>"zlayer docker run | build | compose | ..."</code>
                            " - drop-in replacement for the "<code>"docker"</code>" CLI when built with the "
                            <code>"docker-compat"</code>" feature"
                        </li>
                    </ul>

                    <h4>"Tools"</h4>
                    <ul>
                        <li><code>"zlayer tui"</code>" - interactive terminal UI for builds and cluster state"</li>
                        <li><code>"zlayer completions"</code>" - generate shell completions (bash, zsh, fish, PowerShell)"</li>
                    </ul>

                    <h2 id="examples">"Examples"</h2>

                    <h3>"Web Application"</h3>
                    <pre><code>"apiVersion: zlayer.dev/v1
kind: Container
metadata:
  name: webapp
spec:
  image: nginx:alpine
  resources:
    cpu: 1
    memory: 256Mi
  network:
    ports:
      - containerPort: 80
        hostPort: 8080
  volumes:
    - name: static-files
      hostPath: /var/www/html
      mountPath: /usr/share/nginx/html"</code></pre>

                    <h3>"Database with Persistent Storage"</h3>
                    <pre><code>"apiVersion: zlayer.dev/v1
kind: Container
metadata:
  name: postgres
spec:
  image: postgres:15
  env:
    - name: POSTGRES_PASSWORD
      valueFrom:
        secretRef: db-password
  resources:
    cpu: 2
    memory: 1Gi
  volumes:
    - name: pgdata
      hostPath: /data/postgres
      mountPath: /var/lib/postgresql/data"</code></pre>

                    <h3>"Pod with Multiple Containers"</h3>
                    <pre><code>"apiVersion: zlayer.dev/v1
kind: Pod
metadata:
  name: app-with-sidecar
spec:
  containers:
    - name: app
      image: myapp:latest
      ports:
        - containerPort: 8080
    - name: metrics
      image: prometheus/node-exporter:latest
      ports:
        - containerPort: 9100"</code></pre>

                    <h3>"Windows Service (IIS)"</h3>
                    <pre><code>"apiVersion: zlayer.dev/v1
kind: Service
metadata:
  name: hello-iis
spec:
  os: windows
  image: mcr.microsoft.com/windows/servercore/iis:windowsservercore-ltsc2022
  ports:
    - containerPort: 80"</code></pre>

                    <h2 id="configuration">"Configuration"</h2>

                    <p>
                        "ZLayer is configured via a mix of CLI flags, environment variables, and "
                        "an optional configuration file. The most common knobs are exposed as both "
                        "flags on "<code>"zlayer serve"</code>" and equivalent environment variables."
                    </p>

                    <h3>"Environment Variables"</h3>
                    <ul>
                        <li><code>"ZLAYER_DATA_DIR"</code>" - root directory for image, volume, and cluster state"</li>
                        <li><code>"ZLAYER_STATE_DIR"</code>" - runtime state (sockets, PID files, scheduler state)"</li>
                        <li><code>"ZLAYER_BIND"</code>" - address the API server binds to (default "<code>"0.0.0.0:3669"</code>")"</li>
                        <li><code>"ZLAYER_API_TOKEN"</code>" - bearer token for authenticated API access"</li>
                        <li><code>"ZLAYER_WEB_ADDR"</code>" - address the web frontend binds to (default "<code>"0.0.0.0:3000"</code>")"</li>
                    </ul>

                    <h3>"Service Installation"</h3>
                    <p>
                        "Pass "<code>"--service"</code>" to "<code>"zlayer serve"</code>" to install ZLayer as "
                        "a managed system service. The flag selects the right backend per platform: "
                        "a systemd unit on Linux, a launchd plist on macOS, or a Windows Service "
                        "Control Manager (SCM) entry on Windows. "<code>"zlayer daemon install"</code>" wraps "
                        "the same plumbing if you prefer a dedicated subcommand."
                    </p>

                    <h3>"Windows-specific Flags"</h3>
                    <p>
                        "On Windows, "<code>"zlayer serve --vhd-gb <N>"</code>" caps the size of the WSL2 VHDX "
                        "image used for Linux workloads. This is useful in shared CI environments "
                        "where unbounded growth would exhaust the host disk. Use "
                        <code>"zlayer windows compact"</code>" to reclaim free space inside the VHDX after "
                        "deleting workloads."
                    </p>

                    <h3>"Data Directory Layout"</h3>
                    <ul>
                        <li>"Linux: "<code>"~/.local/share/zlayer/"</code></li>
                        <li>"macOS: "<code>"~/Library/Application Support/zlayer/"</code></li>
                        <li>"Windows: "<code>"%LOCALAPPDATA%\\ZLayer\\"</code></li>
                    </ul>
                    <p>
                        "When ZLayer runs as a system service, the data directory defaults to a "
                        "system-wide location appropriate for the platform (e.g. "
                        <code>"/var/lib/zlayer"</code>" on Linux, "<code>"C:\\ProgramData\\ZLayer"</code>" on Windows)."
                    </p>

                    <p>
                        "For Windows CI test harness setup, see "
                        <a href="https://github.com/BlackLeafDigital/ZLayer/blob/main/docs/windows-ci-runner.md" target="_blank" rel="noopener noreferrer">
                            <code>"docs/windows-ci-runner.md"</code>
                        </a>
                        ". For the complete list of configuration options, see the "
                        <a href="https://github.com/BlackLeafDigital/ZLayer/blob/main/README.md" target="_blank" rel="noopener noreferrer">"README"</a>
                        "."
                    </p>

                    <h2 id="troubleshooting">"Troubleshooting"</h2>

                    <h3>"Common Issues"</h3>
                    <ul>
                        <li>
                            <strong>"Permission denied"</strong>" - Ensure you have the necessary "
                            "privileges to run containers (rootless mode may require additional setup)"
                        </li>
                        <li>
                            <strong>"Image not found"</strong>" - Check that the image reference is "
                            "correct and the registry is accessible"
                        </li>
                        <li>
                            <strong>"Network unreachable"</strong>" - Verify overlay network "
                            "configuration and overlay peer connectivity"
                        </li>
                        <li>
                            <strong>"Windows: serve fails to bind"</strong>
                            " - Run as Administrator. The Windows firewall and HCS APIs require "
                            "elevation. For unattended operation, use "<code>"zlayer daemon install"</code>
                            " to register ZLayer as a Service Control Manager (SCM) service."
                        </li>
                    </ul>

                    <p>
                        "For more help, please "<a href="https://github.com/BlackLeafDigital/ZLayer/issues" target="_blank" rel="noopener noreferrer">"open an issue"</a>
                        " on GitHub."
                    </p>
                </div>
            </main>

            <Footer/>
        </div>
    }
}
