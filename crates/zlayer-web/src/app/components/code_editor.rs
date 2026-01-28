//! Code editor component for the playground

use leptos::prelude::*;

/// Simple code editor component
///
/// This is a basic textarea-based editor. In the future, this could be
/// replaced with a more sophisticated editor like CodeMirror or Monaco.
#[component]
pub fn CodeEditor(
    /// Initial content for the editor
    #[prop(default = String::new())]
    initial_content: String,
    /// Placeholder text
    #[prop(default = "Enter your specification here...".to_string())]
    placeholder: String,
    /// Whether the editor is read-only
    #[prop(default = false)]
    read_only: bool,
    /// Signal to get/set the content
    content: RwSignal<String>,
) -> impl IntoView {
    // Initialize content if provided
    if !initial_content.is_empty() && content.get().is_empty() {
        content.set(initial_content);
    }

    view! {
        <textarea
            class="code-editor-textarea"
            placeholder=placeholder
            readonly=read_only
            on:input=move |ev| {
                content.set(event_target_value(&ev));
            }
            prop:value=move || content.get()
        />
    }
}

/// Styled code block for displaying code
#[component]
pub fn CodeBlock(
    /// The code content to display
    code: &'static str,
    /// Optional title for the code block
    #[prop(default = None)]
    title: Option<&'static str>,
    /// Language for syntax highlighting (future use)
    #[prop(default = "yaml")]
    _language: &'static str,
) -> impl IntoView {
    view! {
        <div class="code-block">
            <div class="code-header">
                <span class="code-dot red"></span>
                <span class="code-dot yellow"></span>
                <span class="code-dot green"></span>
                {title.map(|t| view! { <span class="code-title">{t}</span> })}
            </div>
            <div class="code-content">
                <pre><code>{code}</code></pre>
            </div>
        </div>
    }
}
