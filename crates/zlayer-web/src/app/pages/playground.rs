//! Playground page component for interactive spec validation

use leptos::prelude::*;

use crate::app::components::{CodeEditor, Footer, Navbar};
use crate::app::server_fns::validate_spec;

/// Default example specification
const DEFAULT_SPEC: &str = r#"apiVersion: zlayer.dev/v1
kind: Container
metadata:
  name: example
  namespace: default
spec:
  image: nginx:alpine
  resources:
    cpu: 1
    memory: 256Mi
  network:
    ports:
      - containerPort: 80
        hostPort: 8080
"#;

/// Playground page component
#[component]
pub fn PlaygroundPage() -> impl IntoView {
    // Editor content signal
    let editor_content = RwSignal::new(DEFAULT_SPEC.to_string());
    // Output/result signal
    let output = RwSignal::new(String::from("Click 'Validate' to check your specification."));
    // Loading state
    let is_validating = RwSignal::new(false);

    // Validation action
    let validate_action = Action::new(move |content: &String| {
        let content = content.clone();
        async move { validate_spec(content).await }
    });

    // Handle validation button click
    let on_validate = move |_| {
        is_validating.set(true);
        validate_action.dispatch(editor_content.get());
    };

    // Update output when validation completes
    Effect::new(move |_| {
        if let Some(result) = validate_action.value().get() {
            is_validating.set(false);
            match result {
                Ok(msg) => output.set(format!("Success: {}", msg)),
                Err(e) => output.set(format!("Error: {}", e)),
            }
        }
    });

    view! {
        <div class="page-layout">
            <Navbar/>

            <main class="main-content playground-page">
                <div class="container">
                    <div class="playground-header">
                        <h1 class="playground-title">"ZLayer Playground"</h1>
                        <p class="playground-description">
                            "Write and validate container specifications in real-time. "
                            "Edit the YAML on the left and see validation results on the right."
                        </p>
                    </div>

                    <div class="playground-container">
                        <div class="playground-editor">
                            <div class="playground-panel-header">
                                <span class="playground-panel-title">"Specification"</span>
                                <button
                                    class="btn btn-gradient playground-run-btn"
                                    on:click=on_validate
                                    disabled=move || is_validating.get()
                                >
                                    {move || if is_validating.get() { "Validating..." } else { "Validate" }}
                                </button>
                            </div>
                            <div class="playground-editor-content">
                                <CodeEditor
                                    initial_content=DEFAULT_SPEC.to_string()
                                    placeholder="Enter your container specification...".to_string()
                                    read_only=false
                                    content=editor_content
                                />
                            </div>
                        </div>

                        <div class="playground-output">
                            <div class="playground-panel-header">
                                <span class="playground-panel-title">"Output"</span>
                            </div>
                            <div class="playground-output-content">
                                <pre>{move || output.get()}</pre>
                            </div>
                        </div>
                    </div>
                </div>
            </main>

            <Footer/>
        </div>
    }
}
