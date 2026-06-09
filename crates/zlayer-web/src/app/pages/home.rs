//! Homepage component

use leptos::prelude::*;

use crate::app::components::{CodeBlock, CodeEditor, Footer, Navbar};
use crate::app::icons;
use crate::app::server_fns::validate_spec;

/// Feature card component
#[component]
fn FeatureCard(
    /// Icon component to display
    icon: fn() -> AnyView,
    /// Feature title
    title: &'static str,
    /// Feature description
    description: &'static str,
) -> impl IntoView {
    view! {
        <div class="feature-card">
            <div class="feature-icon">
                {icon()}
            </div>
            <h3 class="feature-title">{title}</h3>
            <p class="feature-description">{description}</p>
        </div>
    }
}

/// Homepage component
#[component]
#[allow(clippy::too_many_lines)]
pub fn HomePage() -> impl IntoView {
    // Example YAML specification (real ZLayer spec format)
    const EXAMPLE_SPEC: &str = r#"version: v1
deployment: my-app

services:
  web:
    rtype: service
    image:
      name: ghcr.io/myorg/web:latest
      pull_policy: if_not_present

    resources:
      cpu: 1.0
      memory: 512Mi

    env:
      DATABASE_URL: "postgres://db:5432/app"
      RUST_LOG: "info"

    endpoints:
      - name: http
        protocol: http
        port: 3000
        host: app.example.com
        expose: public

    scale:
      mode: adaptive
      min_replicas: 2
      max_replicas: 10
      target_cpu_percent: 70

    health:
      start_grace: 15s
      interval: 10s
      timeout: 5s
      retries: 3
      check:
        type: tcp
        port: 3000
"#;

    view! {
        <div class="page-layout">
            <Navbar/>

            <main class="main-content">
                // Hero Section
                <section class="hero">
                    <div class="container hero-content">
                        <div class="hero-logo">
                            <img
                                src="/zlayer_logo.png"
                                alt="ZLayer Logo"
                                class="hero-logo-img"
                            />
                        </div>
                        <div class="hero-badge">
                            {icons::layers_icon("16")}
                            <span>"Open Source Container Orchestration"</span>
                        </div>
                        <h1 class="hero-title">
                            "Lightweight Container"<br/>
                            <span class="highlight">"Orchestration"</span>
                        </h1>
                        <p class="hero-description">
                            "Run containers natively on Linux (libcontainer), macOS (Apple Containerization), "
                            "and Windows (Hyper-V HCS plus a WSL2 delegate). Encrypted overlay networking, "
                            "a built-in image builder, and a Raft-backed scheduler — in a single binary, no daemon."
                        </p>
                        <div class="hero-buttons">
                            <a href="/docs" class="btn btn-primary">
                                {icons::rocket_icon("18")}
                                <span>"Get Started"</span>
                            </a>
                            <a href="/playground" class="btn btn-secondary">
                                <span>"Try It Now"</span>
                                {icons::arrow_right_icon("18")}
                            </a>
                        </div>
                    </div>
                </section>

                // Install Section
                <section class="install install-section">
                    <div class="container">
                        <div class="section-header">
                            <h2 class="section-title">"Install"</h2>
                            <p class="section-description">
                                "One binary. No daemon. Pick your platform."
                            </p>
                        </div>
                        <div class="install-grid">
                            <div class="install-card">
                                <h3 class="install-platform">"Linux / macOS"</h3>
                                <CodeBlock
                                    code="curl -fsSL https://zlayer.dev/install.sh | bash"
                                    title=Some("Linux / macOS")
                                    language="bash"
                                />
                            </div>
                            <div class="install-card">
                                <h3 class="install-platform">"Windows (PowerShell)"</h3>
                                <CodeBlock
                                    code="irm https://zlayer.dev/install.ps1 | iex"
                                    title=Some("Windows (PowerShell)")
                                    language="powershell"
                                />
                            </div>
                            <div class="install-card">
                                <h3 class="install-platform">"From source (Cargo)"</h3>
                                <CodeBlock
                                    code="cargo install --git https://forge.blackleafdigital.com/BlackLeafDigital/ZLayer zlayer"
                                    title=Some("From source (Cargo)")
                                    language="bash"
                                />
                            </div>
                            <div class="install-card">
                                <h3 class="install-platform">"Python installer"</h3>
                                <CodeBlock
                                    code="curl -sSL https://zlayer.dev/install.py | python3"
                                    title=Some("Python installer")
                                    language="bash"
                                />
                            </div>
                        </div>
                        <p class="install-verify">
                            "After install, run "<code>"zlayer --version"</code>" to verify."
                        </p>
                    </div>
                </section>

                // Features Section
                <section class="features">
                    <div class="container">
                        <div class="section-header">
                            <h2 class="section-title">"Why ZLayer?"</h2>
                            <p class="section-description">
                                "Purpose-built for simplicity, security, and performance. "
                                "Everything you need, nothing you don't."
                            </p>
                        </div>
                        <div class="features-grid">
                            <FeatureCard
                                icon=|| icons::monitor_icon().into_any()
                                title="Cross-Platform Native"
                                description="First-class support for Linux, macOS, and Windows. youki on Linux, Seatbelt on macOS, HCS native plus WSL2 delegate on Windows. No Docker Desktop required."
                            />
                            <FeatureCard
                                icon=|| icons::zap_icon("24").into_any()
                                title="Daemonless on Linux"
                                description="On Linux, each container runs as a direct child process via libcontainer. Complete control, full visibility, no daemon to keep alive."
                            />
                            <FeatureCard
                                icon=|| icons::hammer_icon("24").into_any()
                                title="Built-in Image Builder"
                                description="Build OCI images directly from Dockerfile or ZImagefile YAML. buildah on Linux and macOS, native HCS-backed builder on Windows. No external tools required."
                            />
                            <FeatureCard
                                icon=|| icons::network_icon("24").into_any()
                                title="Encrypted Overlay Networks"
                                description="Mesh networking via boringtun userspace WireGuard. IP allocation, DNS service discovery, and health checking. Wintun adapter on Windows."
                            />
                            <FeatureCard
                                icon=|| icons::shield_icon("24").into_any()
                                title="Security First"
                                description="Rootless containers, seccomp profiles, and namespace isolation. OpenID Connect SSO, RBAC with users, groups, and permissions, plus an audit log of every change."
                            />
                            <FeatureCard
                                icon=|| icons::book_icon("24").into_any()
                                title="GitOps & Multi-Tenancy"
                                description="Project-scoped deployments with git polling, webhook receivers, environments, secrets, and credentials. Workflows compose tasks, builds, and deploys into DAGs."
                            />
                        </div>
                    </div>
                </section>

                // Code Example Section
                <section class="code-section">
                    <div class="container">
                        <div class="section-header">
                            <h2 class="section-title">"Simple Configuration"</h2>
                            <p class="section-description">
                                "This is the actual spec format the "<code>"zlayer deploy"</code>" CLI consumes — not a Kubernetes-style approximation."
                            </p>
                        </div>
                        <div class="code-block-wrapper">
                            <CodeBlock
                                code=EXAMPLE_SPEC
                                title=Some("my-app.zlayer.yml")
                                language="yaml"
                            />
                        </div>
                    </div>
                </section>

                // Try It Now Section
                <TryItNowSection/>

                // CTA Section
                <section class="cta">
                    <div class="container cta-content">
                        <h2 class="cta-title">"Ready to Get Started?"</h2>
                        <p class="cta-description">
                            "Deploy your first container in minutes. Check out our documentation "
                            "or try the interactive playground."
                        </p>
                        <div class="hero-buttons">
                            <a href="/docs" class="btn btn-primary">
                                "Read the Docs"
                            </a>
                            <a href="https://github.com/BlackLeafDigital/ZLayer" target="_blank" rel="noopener noreferrer" class="btn btn-secondary">
                                "View on GitHub"
                            </a>
                        </div>
                    </div>
                </section>
            </main>

            <Footer/>
        </div>
    }
}

/// Mini `ZLayer` deployment spec for the Try It Now validator demo
const MINI_DEMO_SPEC: &str = r"version: v1
deployment: demo

services:
  api:
    rtype: service
    image:
      name: ghcr.io/myorg/api:latest

    endpoints:
      - name: http
        protocol: http
        port: 8080
        expose: public

    scale:
      mode: fixed
      replicas: 3
";

/// Try It Now section — deployment-spec validator demo
#[component]
fn TryItNowSection() -> impl IntoView {
    let editor_content = RwSignal::new(MINI_DEMO_SPEC.to_string());
    let output = RwSignal::new(String::new());
    let is_running = RwSignal::new(false);
    let has_run = RwSignal::new(false);

    // Validation action — calls the real zlayer-spec parser on the server
    let validate_action = Action::new(move |yaml: &String| {
        let yaml = yaml.clone();
        async move { validate_spec(yaml).await }
    });

    // Handle validate button click
    let on_run = move |_| {
        is_running.set(true);
        has_run.set(true);
        validate_action.dispatch(editor_content.get());
    };

    // Update output when validation completes
    Effect::new(move |_| {
        if let Some(result) = validate_action.value().get() {
            is_running.set(false);
            match result {
                Ok(msg) => output.set(msg),
                Err(e) => output.set(format!("{e}")),
            }
        }
    });

    view! {
        <section class="try-it-now">
            <div class="container">
                <div class="section-header">
                    <h2 class="section-title">"Validate a deployment"</h2>
                    <p class="section-description">
                        "Paste a ZLayer deployment spec. We run the actual zlayer-spec parser on "
                        "the server and tell you exactly what's wrong — same code path as "
                        <code>"zlayer deploy"</code>". Includes a WebAssembly runtime — see the Playground for that."
                    </p>
                </div>

                <div class="try-it-container">
                    <div class="try-it-editor">
                        <div class="try-it-header">
                            <span class="try-it-label">"Container Spec (YAML)"</span>
                            <button
                                class="btn btn-gradient try-it-run-btn"
                                on:click=on_run
                                disabled=move || is_running.get()
                            >
                                {move || if is_running.get() { "Validating..." } else { "Validate" }}
                            </button>
                        </div>
                        <div class="try-it-code">
                            <CodeEditor
                                initial_content=MINI_DEMO_SPEC.to_string()
                                placeholder="ZLayer spec YAML...".to_string()
                                read_only=false
                                content=editor_content
                            />
                        </div>
                    </div>

                    <div class="try-it-output">
                        <div class="try-it-header">
                            <span class="try-it-label">"Validation"</span>
                        </div>
                        <div class="try-it-result">
                            {move || {
                                let out = output.get();
                                let ran = has_run.get();

                                if out.is_empty() && !ran {
                                    view! {
                                        <span class="try-it-placeholder">"Click Validate to check the spec"</span>
                                    }.into_any()
                                } else if out.is_empty() && ran {
                                    view! {
                                        <span class="try-it-placeholder">"Validating..."</span>
                                    }.into_any()
                                } else {
                                    view! {
                                        <pre class="try-it-output-text">{out}</pre>
                                    }.into_any()
                                }
                            }}
                        </div>
                    </div>
                </div>

                <div class="try-it-cta">
                    <a href="/playground" class="btn btn-outline">
                        "Open Full Playground"
                        {icons::arrow_right_icon("18")}
                    </a>
                </div>
            </div>
        </section>
    }
}
