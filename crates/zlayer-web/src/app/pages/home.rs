//! Homepage component

use leptos::prelude::*;

use crate::app::components::{CodeBlock, Footer, Navbar};
use crate::app::icons::{
    ArrowRightIcon, HammerIcon, LayersIcon, NetworkIcon, RocketIcon, ShieldIcon, ZapIcon,
};

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
pub fn HomePage() -> impl IntoView {
    // Example YAML specification
    const EXAMPLE_SPEC: &str = r#"apiVersion: zlayer.dev/v1
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
        secretRef: db-credentials"#;

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
                            <LayersIcon size="16"/>
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
                                <RocketIcon size="18"/>
                                <span>"Get Started"</span>
                            </a>
                            <a href="/playground" class="btn btn-secondary">
                                <span>"Try It Now"</span>
                                <ArrowRightIcon size="18"/>
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
                                icon=|| view! { <ZapIcon size="24"/> }.into_any()
                                title="Daemonless Architecture"
                                description="No background daemons required. Each container runs as a direct child process, giving you complete control and visibility."
                            />
                            <FeatureCard
                                icon=|| view! { <HammerIcon size="24"/> }.into_any()
                                title="Built-in Builder"
                                description="Build OCI-compliant container images directly. No need for external tools like Docker or Buildah."
                            />
                            <FeatureCard
                                icon=|| view! { <NetworkIcon size="24"/> }.into_any()
                                title="Overlay Networks"
                                description="WireGuard-based overlay networking with automatic peer discovery. Secure communication across hosts out of the box."
                            />
                            <FeatureCard
                                icon=|| view! { <ShieldIcon size="24"/> }.into_any()
                                title="Security First"
                                description="Rootless containers, seccomp profiles, and namespace isolation. Defense in depth for production workloads."
                            />
                            <FeatureCard
                                icon=|| view! { <LayersIcon size="24"/> }.into_any()
                                title="Kubernetes-like API"
                                description="Familiar YAML specifications for containers, pods, and services. Easy migration path from existing deployments."
                            />
                            <FeatureCard
                                icon=|| view! { <RocketIcon size="24"/> }.into_any()
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
                                _language="yaml"
                            />
                        </div>
                    </div>
                </section>

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
