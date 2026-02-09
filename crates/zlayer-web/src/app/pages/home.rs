//! Homepage component

use leptos::prelude::*;

use crate::app::components::{CodeBlock, CodeEditor, Footer, Navbar};
use crate::app::icons;
use crate::app::server_fns::execute_wasm;

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
    // Example YAML specification
    const EXAMPLE_SPEC: &str = r"apiVersion: zlayer.dev/v1
kind: Container
metadata:
  name: my-app
  namespace: production
spec:
  image: ghcr.io/myorg/myapp:latest
  resources:
    cpu: 2
    memory: 512Mi
  network:
    overlay: my-network
    ports:
      - containerPort: 8080
        hostPort: 80
  env:
    - name: DATABASE_URL
      valueFrom:
        secretRef: db-credentials";

    view! {
        <div class="page-layout">
            <Navbar/>

            <main class="main-content">
                // Hero Section
                <section class="hero">
                    <div class="container hero-content">
                        <div class="hero-logo">
                            <img
                                src="/assets/zlayer_logo.png"
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
                            "ZLayer brings Kubernetes-like container orchestration to any Linux environment. "
                            "Daemonless architecture, built-in networking, and zero configuration overhead."
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
                                icon=|| icons::zap_icon("24").into_any()
                                title="Daemonless Architecture"
                                description="No background daemons required. Each container runs as a direct child process, giving you complete control and visibility."
                            />
                            <FeatureCard
                                icon=|| icons::hammer_icon("24").into_any()
                                title="Built-in Builder"
                                description="Build OCI-compliant container images directly. No need for external tools like Docker or Buildah."
                            />
                            <FeatureCard
                                icon=|| icons::network_icon("24").into_any()
                                title="Overlay Networks"
                                description="Encrypted overlay networking with automatic peer discovery. Secure communication across hosts out of the box."
                            />
                            <FeatureCard
                                icon=|| icons::shield_icon("24").into_any()
                                title="Security First"
                                description="Rootless containers, seccomp profiles, and namespace isolation. Defense in depth for production workloads."
                            />
                            <FeatureCard
                                icon=|| icons::layers_icon("24").into_any()
                                title="Kubernetes-like API"
                                description="Familiar YAML specifications for containers, pods, and services. Easy migration path from existing deployments."
                            />
                            <FeatureCard
                                icon=|| icons::rocket_icon("24").into_any()
                                title="Minimal Footprint"
                                description="Single binary under 20MB. Fast startup times and low resource consumption for edge and IoT deployments."
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
                                "Define your containers with familiar YAML syntax. "
                                "Everything you need in one declarative file."
                            </p>
                        </div>
                        <div class="code-block-wrapper">
                            <CodeBlock
                                code=EXAMPLE_SPEC
                                title=Some("container.yaml")
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
                            <a href="https://github.com/zachhandley/ZLayer" target="_blank" rel="noopener noreferrer" class="btn btn-secondary">
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

/// Mini Hello World WAT for the try-it-now section
const MINI_HELLO_WAT: &str = r#"(module
  (import "wasi_snapshot_preview1" "fd_write"
    (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 8) "Hello from ZLayer WASM!\n")
  (func (export "_start")
    (i32.store (i32.const 0) (i32.const 8))
    (i32.store (i32.const 4) (i32.const 24))
    (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 100)))
  )
)"#;

/// Try It Now section with mini WASM playground
#[component]
fn TryItNowSection() -> impl IntoView {
    let editor_content = RwSignal::new(MINI_HELLO_WAT.to_string());
    let output = RwSignal::new(String::new());
    let is_running = RwSignal::new(false);
    let has_run = RwSignal::new(false);

    // Execution action
    let execute_action = Action::new(move |code: &String| {
        let code = code.clone();
        async move { execute_wasm(code, "wat".to_string()).await }
    });

    // Handle run button click
    let on_run = move |_| {
        is_running.set(true);
        has_run.set(true);
        execute_action.dispatch(editor_content.get());
    };

    // Update output when execution completes
    Effect::new(move |_| {
        if let Some(result) = execute_action.value().get() {
            is_running.set(false);
            match result {
                Ok(res) => {
                    if !res.stdout.is_empty() {
                        output.set(res.stdout);
                    } else if let Some(info) = res.info {
                        output.set(format!(
                            "Executed successfully in {}ms\n{}",
                            res.execution_time_ms, info
                        ));
                    } else {
                        output.set(format!(
                            "Executed successfully (exit code: {})",
                            res.exit_code
                        ));
                    }
                }
                Err(e) => output.set(format!("Error: {e}")),
            }
        }
    });

    view! {
        <section class="try-it-now">
            <div class="container">
                <div class="section-header">
                    <h2 class="section-title">"Try It Now"</h2>
                    <p class="section-description">
                        "Execute WebAssembly code directly in your browser. "
                        "Click Run to see it in action."
                    </p>
                </div>

                <div class="try-it-container">
                    <div class="try-it-editor">
                        <div class="try-it-header">
                            <span class="try-it-label">"WebAssembly (WAT)"</span>
                            <button
                                class="btn btn-gradient try-it-run-btn"
                                on:click=on_run
                                disabled=move || is_running.get()
                            >
                                {move || if is_running.get() { "Running..." } else { "Run" }}
                            </button>
                        </div>
                        <div class="try-it-code">
                            <CodeEditor
                                initial_content=MINI_HELLO_WAT.to_string()
                                placeholder="WAT code...".to_string()
                                read_only=false
                                content=editor_content
                            />
                        </div>
                    </div>

                    <div class="try-it-output">
                        <div class="try-it-header">
                            <span class="try-it-label">"Output"</span>
                        </div>
                        <div class="try-it-result">
                            {move || {
                                let out = output.get();
                                let ran = has_run.get();

                                if out.is_empty() && !ran {
                                    view! {
                                        <span class="try-it-placeholder">"Click Run to execute"</span>
                                    }.into_any()
                                } else if out.is_empty() && ran {
                                    view! {
                                        <span class="try-it-placeholder">"Running..."</span>
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
