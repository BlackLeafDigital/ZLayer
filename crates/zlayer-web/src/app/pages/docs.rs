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
                        "ZLayer is distributed as a single binary. Download the latest release "
                        "from GitHub or build from source:"
                    </p>

                    <pre><code>"# Download the latest release
curl -fsSL https://zlayer.dev/install.sh | bash
# Or download directly:
# curl -LO https://github.com/zachhandley/ZLayer/releases/latest/download/zlayer-linux-amd64
chmod +x zlayer-linux-amd64
sudo mv zlayer-linux-amd64 /usr/local/bin/zlayer

# Verify installation
zlayer --version"</code></pre>

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
                        "ZLayer provides built-in overlay networking using WireGuard. This allows "
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

                    <h3>"CLI Commands"</h3>
                    <ul>
                        <li><code>"zlayer run"</code>" - Run a container from a specification"</li>
                        <li><code>"zlayer build"</code>" - Build a container image"</li>
                        <li><code>"zlayer ps"</code>" - List running containers"</li>
                        <li><code>"zlayer logs"</code>" - View container logs"</li>
                        <li><code>"zlayer exec"</code>" - Execute a command in a running container"</li>
                        <li><code>"zlayer stop"</code>" - Stop a running container"</li>
                        <li><code>"zlayer rm"</code>" - Remove a stopped container"</li>
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

                    <h2 id="configuration">"Configuration"</h2>

                    <p>
                        "ZLayer can be configured via environment variables or a configuration file. "
                        "See the "<a href="https://github.com/zachhandley/ZLayer/blob/main/README.md" target="_blank" rel="noopener noreferrer">"README"</a>
                        " for a complete list of configuration options."
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
                            "configuration and WireGuard peer connectivity"
                        </li>
                    </ul>

                    <p>
                        "For more help, please "<a href="https://github.com/zachhandley/ZLayer/issues" target="_blank" rel="noopener noreferrer">"open an issue"</a>
                        " on GitHub."
                    </p>
                </div>
            </main>

            <Footer/>
        </div>
    }
}
