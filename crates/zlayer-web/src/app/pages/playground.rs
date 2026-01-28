//! Playground page component for interactive spec validation and WASM execution

use leptos::prelude::*;

use crate::app::components::{CodeEditor, Footer, Navbar};
use crate::app::server_fns::{execute_wasm, execute_wasm_function, validate_spec, WasmExecutionResult};

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

/// Default Hello World WAT example
const HELLO_WORLD_WAT: &str = r#"(module
  ;; Import WASI fd_write function
  (import "wasi_snapshot_preview1" "fd_write"
    (func $fd_write (param i32 i32 i32 i32) (result i32)))

  ;; Memory export (required by WASI)
  (memory (export "memory") 1)

  ;; Store the message "Hello from ZLayer WASM!\n" at offset 8
  (data (i32.const 8) "Hello from ZLayer WASM!\n")

  ;; Entry point
  (func (export "_start")
    ;; iovec structure at offset 0:
    ;; - pointer to string (4 bytes): offset 8
    ;; - length of string (4 bytes): 24
    (i32.store (i32.const 0) (i32.const 8))   ;; iov_base = 8
    (i32.store (i32.const 4) (i32.const 24))  ;; iov_len = 24

    ;; Call fd_write(stdout=1, iovs=0, iovs_len=1, nwritten=100)
    (drop (call $fd_write
      (i32.const 1)    ;; fd: stdout
      (i32.const 0)    ;; iovs pointer
      (i32.const 1)    ;; iovs count
      (i32.const 100)  ;; nwritten pointer
    ))
  )
)"#;

/// Fibonacci WAT example (exports a function)
const FIBONACCI_WAT: &str = r#"(module
  ;; Recursive Fibonacci function
  ;; Usage: Call fib(n) to get the nth Fibonacci number
  (func $fib (export "fib") (param $n i32) (result i32)
    (if (result i32) (i32.lt_s (local.get $n) (i32.const 2))
      (then (local.get $n))
      (else
        (i32.add
          (call $fib (i32.sub (local.get $n) (i32.const 1)))
          (call $fib (i32.sub (local.get $n) (i32.const 2)))
        )
      )
    )
  )
)"#;

/// Counter WAT example with WASI output
const COUNTER_WAT: &str = r#"(module
  ;; Import WASI fd_write
  (import "wasi_snapshot_preview1" "fd_write"
    (func $fd_write (param i32 i32 i32 i32) (result i32)))

  (memory (export "memory") 1)

  ;; Counter message
  (data (i32.const 100) "Counting: 1 2 3 4 5\n")

  (func (export "_start")
    ;; Set up iovec
    (i32.store (i32.const 0) (i32.const 100))  ;; iov_base
    (i32.store (i32.const 4) (i32.const 21))   ;; iov_len

    ;; Write to stdout
    (drop (call $fd_write
      (i32.const 1)
      (i32.const 0)
      (i32.const 1)
      (i32.const 200)
    ))
  )
)"#;

/// Factorial WAT example
const FACTORIAL_WAT: &str = r#"(module
  ;; Iterative factorial function
  ;; Usage: Call factorial(n) to get n!
  (func $factorial (export "factorial") (param $n i32) (result i64)
    (local $result i64)
    (local $i i32)

    ;; Initialize result = 1
    (local.set $result (i64.const 1))
    ;; Initialize i = 1
    (local.set $i (i32.const 1))

    ;; Loop while i <= n
    (block $done
      (loop $loop
        ;; if i > n, break
        (br_if $done (i32.gt_s (local.get $i) (local.get $n)))

        ;; result = result * i
        (local.set $result
          (i64.mul (local.get $result) (i64.extend_i32_s (local.get $i))))

        ;; i = i + 1
        (local.set $i (i32.add (local.get $i) (i32.const 1)))

        ;; continue loop
        (br $loop)
      )
    )

    ;; return result
    (local.get $result)
  )
)"#;

/// Playground tab options
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaygroundTab {
    #[default]
    ContainerSpec,
    WasmPlayground,
}

/// WASM example options
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum WasmExample {
    #[default]
    HelloWorld,
    Fibonacci,
    Counter,
    Factorial,
}

impl WasmExample {
    fn code(&self) -> &'static str {
        match self {
            WasmExample::HelloWorld => HELLO_WORLD_WAT,
            WasmExample::Fibonacci => FIBONACCI_WAT,
            WasmExample::Counter => COUNTER_WAT,
            WasmExample::Factorial => FACTORIAL_WAT,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            WasmExample::HelloWorld => "Hello World",
            WasmExample::Fibonacci => "Fibonacci",
            WasmExample::Counter => "Counter",
            WasmExample::Factorial => "Factorial",
        }
    }

    fn description(&self) -> &'static str {
        match self {
            WasmExample::HelloWorld => "Prints a greeting using WASI",
            WasmExample::Fibonacci => "Recursive Fibonacci (exports fib function)",
            WasmExample::Counter => "Simple counter output using WASI",
            WasmExample::Factorial => "Iterative factorial (exports factorial function)",
        }
    }

    fn has_main(&self) -> bool {
        match self {
            WasmExample::HelloWorld | WasmExample::Counter => true,
            WasmExample::Fibonacci | WasmExample::Factorial => false,
        }
    }

    fn exported_function(&self) -> Option<(&'static str, i64)> {
        match self {
            WasmExample::Fibonacci => Some(("fib", 10)),
            WasmExample::Factorial => Some(("factorial", 10)),
            _ => None,
        }
    }
}

/// Playground page component
#[component]
pub fn PlaygroundPage() -> impl IntoView {
    // Active tab
    let active_tab = RwSignal::new(PlaygroundTab::default());

    view! {
        <div class="page-layout">
            <Navbar/>

            <main class="main-content playground-page">
                <div class="container">
                    <div class="playground-header">
                        <h1 class="playground-title">"ZLayer Playground"</h1>
                        <p class="playground-description">
                            "Validate container specifications or execute WebAssembly code in real-time."
                        </p>
                    </div>

                    // Tab navigation
                    <div class="playground-tabs">
                        <button
                            class=move || if active_tab.get() == PlaygroundTab::ContainerSpec {
                                "playground-tab active"
                            } else {
                                "playground-tab"
                            }
                            on:click=move |_| active_tab.set(PlaygroundTab::ContainerSpec)
                        >
                            "Container Spec"
                        </button>
                        <button
                            class=move || if active_tab.get() == PlaygroundTab::WasmPlayground {
                                "playground-tab active"
                            } else {
                                "playground-tab"
                            }
                            on:click=move |_| active_tab.set(PlaygroundTab::WasmPlayground)
                        >
                            "WASM Playground"
                        </button>
                    </div>

                    // Tab content
                    <div class="playground-tab-content">
                        {move || match active_tab.get() {
                            PlaygroundTab::ContainerSpec => view! { <ContainerSpecTab/> }.into_any(),
                            PlaygroundTab::WasmPlayground => view! { <WasmPlaygroundTab/> }.into_any(),
                        }}
                    </div>
                </div>
            </main>

            <Footer/>
        </div>
    }
}

/// Container Spec validation tab
#[component]
fn ContainerSpecTab() -> impl IntoView {
    // Editor content signal
    let editor_content = RwSignal::new(DEFAULT_SPEC.to_string());
    // Output/result signal
    let output = RwSignal::new(String::from(
        "Click 'Validate' to check your specification.",
    ));
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
    }
}

/// WASM Playground tab
#[component]
fn WasmPlaygroundTab() -> impl IntoView {
    // Editor content signal
    let editor_content = RwSignal::new(HELLO_WORLD_WAT.to_string());
    // Selected example
    let selected_example = RwSignal::new(WasmExample::HelloWorld);
    // Function input for library modules
    let function_input = RwSignal::new(String::from("10"));
    // Output/result signal
    let output = RwSignal::new(WasmOutput::default());
    // Loading state
    let is_running = RwSignal::new(false);

    // Execution action for modules with _start
    let execute_action = Action::new(move |code: &String| {
        let code = code.clone();
        async move { execute_wasm(code, "wat".to_string(), String::new()).await }
    });

    // Execution action for function calls
    let execute_func_action = Action::new(move |(code, func_name, args): &(String, String, Vec<i64>)| {
        let code = code.clone();
        let func_name = func_name.clone();
        let args = args.clone();
        async move { execute_wasm_function(code, func_name, args).await }
    });

    // Handle example selection
    let on_example_select = move |example: WasmExample| {
        selected_example.set(example);
        editor_content.set(example.code().to_string());
        output.set(WasmOutput::default());
    };

    // Handle run button click
    let on_run = move |_| {
        is_running.set(true);
        let example = selected_example.get();
        let code = editor_content.get();

        if example.has_main() {
            execute_action.dispatch(code);
        } else if let Some((func_name, _default_arg)) = example.exported_function() {
            // Parse the function input
            let arg: i64 = function_input.get().parse().unwrap_or(10);
            execute_func_action.dispatch((code, func_name.to_string(), vec![arg]));
        }
    };

    // Update output when execution completes
    Effect::new(move |_| {
        if let Some(result) = execute_action.value().get() {
            is_running.set(false);
            match result {
                Ok(res) => output.set(WasmOutput::from_result(res)),
                Err(e) => output.set(WasmOutput::error(e.to_string())),
            }
        }
    });

    Effect::new(move |_| {
        if let Some(result) = execute_func_action.value().get() {
            is_running.set(false);
            match result {
                Ok(res) => output.set(WasmOutput::from_result(res)),
                Err(e) => output.set(WasmOutput::error(e.to_string())),
            }
        }
    });

    view! {
        <div class="wasm-playground">
            // Examples panel
            <div class="wasm-examples">
                <h3 class="wasm-examples-title">"Examples"</h3>
                <div class="wasm-example-list">
                    <ExampleButton
                        example=WasmExample::HelloWorld
                        selected=selected_example
                        on_select=on_example_select
                    />
                    <ExampleButton
                        example=WasmExample::Fibonacci
                        selected=selected_example
                        on_select=on_example_select
                    />
                    <ExampleButton
                        example=WasmExample::Counter
                        selected=selected_example
                        on_select=on_example_select
                    />
                    <ExampleButton
                        example=WasmExample::Factorial
                        selected=selected_example
                        on_select=on_example_select
                    />
                </div>
            </div>

            // Main editor area
            <div class="playground-container wasm-editor-container">
                <div class="playground-editor">
                    <div class="playground-panel-header">
                        <span class="playground-panel-title">"WAT Code"</span>
                        <div class="wasm-run-controls">
                            // Show input field for library modules
                            {move || {
                                let example = selected_example.get();
                                if !example.has_main() {
                                    if let Some((func_name, _)) = example.exported_function() {
                                        return view! {
                                            <div class="wasm-func-input">
                                                <label>{format!("{}(", func_name)}</label>
                                                <input
                                                    type="number"
                                                    class="wasm-arg-input"
                                                    prop:value=move || function_input.get()
                                                    on:input=move |ev| {
                                                        function_input.set(event_target_value(&ev));
                                                    }
                                                />
                                                <label>")"</label>
                                            </div>
                                        }.into_any();
                                    }
                                }
                                view! { <span></span> }.into_any()
                            }}
                            <button
                                class="btn btn-gradient playground-run-btn"
                                on:click=on_run
                                disabled=move || is_running.get()
                            >
                                {move || if is_running.get() { "Running..." } else { "Run" }}
                            </button>
                        </div>
                    </div>
                    <div class="playground-editor-content wasm-editor">
                        <CodeEditor
                            initial_content=HELLO_WORLD_WAT.to_string()
                            placeholder="Enter your WAT code here...".to_string()
                            read_only=false
                            content=editor_content
                        />
                    </div>
                </div>

                <div class="playground-output wasm-output">
                    <div class="playground-panel-header">
                        <span class="playground-panel-title">"Output"</span>
                        {move || {
                            let out = output.get();
                            if out.execution_time_ms > 0 {
                                view! {
                                    <span class="wasm-exec-time">
                                        {format!("{}ms", out.execution_time_ms)}
                                    </span>
                                }.into_any()
                            } else {
                                view! { <span></span> }.into_any()
                            }
                        }}
                    </div>
                    <div class="playground-output-content">
                        <WasmOutputDisplay output=output/>
                    </div>
                </div>
            </div>
        </div>
    }
}

/// Example button component
#[component]
fn ExampleButton(
    example: WasmExample,
    selected: RwSignal<WasmExample>,
    on_select: impl Fn(WasmExample) + 'static + Copy,
) -> impl IntoView {
    view! {
        <button
            class=move || if selected.get() == example {
                "wasm-example-btn active"
            } else {
                "wasm-example-btn"
            }
            on:click=move |_| on_select(example)
        >
            <span class="wasm-example-name">{example.name()}</span>
            <span class="wasm-example-desc">{example.description()}</span>
        </button>
    }
}

/// WASM output data structure
#[derive(Clone, Default)]
struct WasmOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
    execution_time_ms: u64,
    info: Option<String>,
    is_error: bool,
    error_message: Option<String>,
}

impl WasmOutput {
    fn from_result(result: WasmExecutionResult) -> Self {
        Self {
            stdout: result.stdout,
            stderr: result.stderr,
            exit_code: result.exit_code,
            execution_time_ms: result.execution_time_ms,
            info: result.info,
            is_error: false,
            error_message: None,
        }
    }

    fn error(message: String) -> Self {
        Self {
            is_error: true,
            error_message: Some(message),
            ..Default::default()
        }
    }
}

/// WASM output display component
#[component]
fn WasmOutputDisplay(output: RwSignal<WasmOutput>) -> impl IntoView {
    view! {
        <div class="wasm-output-display">
            {move || {
                let out = output.get();

                if out.is_error {
                    return view! {
                        <div class="wasm-output-error">
                            <span class="wasm-output-label error">"Error"</span>
                            <pre class="wasm-error-text">{out.error_message.unwrap_or_default()}</pre>
                        </div>
                    }.into_any();
                }

                if out.stdout.is_empty() && out.stderr.is_empty() && out.info.is_none() {
                    return view! {
                        <div class="wasm-output-placeholder">
                            "Click 'Run' to execute the WASM code."
                        </div>
                    }.into_any();
                }

                view! {
                    <div class="wasm-output-sections">
                        // Info section
                        {out.info.as_ref().map(|info| view! {
                            <div class="wasm-output-section">
                                <span class="wasm-output-label info">"Info"</span>
                                <pre>{info.clone()}</pre>
                            </div>
                        })}

                        // Stdout section
                        {if !out.stdout.is_empty() {
                            Some(view! {
                                <div class="wasm-output-section">
                                    <span class="wasm-output-label stdout">"stdout"</span>
                                    <pre>{out.stdout.clone()}</pre>
                                </div>
                            })
                        } else {
                            None
                        }}

                        // Stderr section
                        {if !out.stderr.is_empty() {
                            Some(view! {
                                <div class="wasm-output-section">
                                    <span class="wasm-output-label stderr">"stderr"</span>
                                    <pre>{out.stderr.clone()}</pre>
                                </div>
                            })
                        } else {
                            None
                        }}

                        // Exit code
                        <div class="wasm-output-footer">
                            <span class=move || if out.exit_code == 0 {
                                "wasm-exit-code success"
                            } else {
                                "wasm-exit-code error"
                            }>
                                {format!("Exit code: {}", out.exit_code)}
                            </span>
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}
