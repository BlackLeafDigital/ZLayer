//! Footer component

use leptos::prelude::*;

use crate::app::icons::{ContainerIcon, GithubIcon};

/// Footer component
#[component]
pub fn Footer() -> impl IntoView {
    view! {
        <footer class="footer">
            <div class="container">
                <div class="footer-content">
                    <div class="footer-brand">
                        <a href="/" class="footer-logo">
                            <ContainerIcon size="24"/>
                            <span>"ZLayer"</span>
                        </a>
                        <p class="footer-tagline">
                            "Lightweight container orchestration for modern infrastructure. "
                            "Daemonless, secure, and built for developers."
                        </p>
                    </div>

                    <div class="footer-section">
                        <h4>"Documentation"</h4>
                        <div class="footer-links">
                            <a href="/docs" class="footer-link">"Getting Started"</a>
                            <a href="/docs#concepts" class="footer-link">"Core Concepts"</a>
                            <a href="/docs#api" class="footer-link">"API Reference"</a>
                            <a href="/docs#examples" class="footer-link">"Examples"</a>
                        </div>
                    </div>

                    <div class="footer-section">
                        <h4>"Resources"</h4>
                        <div class="footer-links">
                            <a href="/playground" class="footer-link">"Playground"</a>
                            <a href="https://github.com/zachhandley/ZLayer/releases" target="_blank" rel="noopener noreferrer" class="footer-link">
                                "Releases"
                            </a>
                            <a href="https://github.com/zachhandley/ZLayer/blob/main/CHANGELOG.md" target="_blank" rel="noopener noreferrer" class="footer-link">
                                "Changelog"
                            </a>
                        </div>
                    </div>

                    <div class="footer-section">
                        <h4>"Community"</h4>
                        <div class="footer-links">
                            <a href="https://github.com/zachhandley/ZLayer" target="_blank" rel="noopener noreferrer" class="footer-link">
                                "GitHub"
                            </a>
                            <a href="https://github.com/zachhandley/ZLayer/issues" target="_blank" rel="noopener noreferrer" class="footer-link">
                                "Issues"
                            </a>
                            <a href="https://github.com/zachhandley/ZLayer/discussions" target="_blank" rel="noopener noreferrer" class="footer-link">
                                "Discussions"
                            </a>
                        </div>
                    </div>
                </div>

                <div class="footer-bottom">
                    <p class="footer-copyright">
                        "Copyright 2025 ZLayer Contributors. Licensed under Apache-2.0."
                    </p>
                    <div class="footer-social">
                        <a href="https://github.com/zachhandley/ZLayer" target="_blank" rel="noopener noreferrer" title="GitHub">
                            <GithubIcon size="20"/>
                        </a>
                    </div>
                </div>
            </div>
        </footer>
    }
}
