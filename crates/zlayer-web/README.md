# zlayer-web

Leptos SSR + WASM frontend for ZLayer (marketing site, docs, dashboard, and
WASM playground).

## Overview

`zlayer-web` is the public-facing web UI for ZLayer. It is a single Leptos
0.8 application that compiles into two artifacts via cargo-leptos:

- a server binary (`zlayer-web`, requires the `ssr` feature) that serves
  the SSR-rendered HTML and Leptos server-function endpoints with Axum, and
- a WASM bundle (the `hydrate` feature) that hydrates the server-rendered
  HTML into a fully reactive client-side app.

The crate has `publish = false` and is not intended for use as a library
outside this workspace.

## Routes

The router lives in `src/app/mod.rs` and exposes four top-level routes:

| Path          | Page component               | Purpose                                                                |
|---------------|------------------------------|------------------------------------------------------------------------|
| `/`           | `pages::HomePage`            | Marketing / landing page with hero, features, and getting-started CTA.|
| `/docs`       | `pages::DocsPage`            | Full product documentation with anchor sections (also matches `/docs/*` for deep links). |
| `/playground` | `pages::PlaygroundPage`      | Spec validator and in-browser WASM runner that calls server functions for execution. |
| `/dashboard`  | `pages::DashboardPage`       | Live runtime view backed by `WebState` (services, containers, metrics). |

A `NotFound` component handles unmatched routes. All pages share the
`Navbar`, `Footer`, theme toggle (light / dark / system, persisted in
`localStorage` under `zlayer-theme`), and inlined SVG icons from
`app::icons` (e.g. `container_icon`, `layers_icon`, `book_icon`,
`terminal_icon`, etc.).

## Server functions

`src/app/server_fns.rs` exposes the server-only endpoints invoked from
Leptos components via `#[server]`:

- `get_system_stats() -> SystemStats` — version, container counts,
  service health, CPU / memory usage, uptime.
- `get_all_service_stats() -> Vec<ServiceStats>` — per-service replicas,
  CPU %, memory %, health status.
- `validate_spec(yaml_content) -> String` — validates a deployment YAML
  document via `zlayer_spec`.
- `execute_wasm(...)`, `execute_wasm_bytes_api(...)`,
  `execute_wasm_function(...)` — load and execute a WASM module
  server-side using `wasmtime` + `wasmtime-wasi`, returning
  `WasmExecutionResult { stdout, stderr, exit_code, execution_time_ms, info }`.

Server functions read shared backend state from `server::state::WebState`
(set via Leptos context). `WebState` optionally holds an
`Arc<RwLock<zlayer_agent::ServiceManager>>` and an
`Arc<zlayer_scheduler::metrics::MetricsCollector>`; when either is absent
the corresponding endpoints fall back to defaults.

## Running the server

`server::ui::start_ui_server(web_state)` wires the Leptos SSR router,
server-function handler, WASM `pkg` directory (`/pkg`), and static assets
(`/assets`) into a single Axum app. Configuration:

| Env var            | Default              | Purpose                                |
|--------------------|----------------------|----------------------------------------|
| `ZLAYER_WEB_ADDR`  | `0.0.0.0:3000`       | Bind address for the SSR server.       |
| `LEPTOS_SITE_ROOT` | `target/site` (dev) / `/app/target/site` (release) | Directory containing the built WASM `pkg/` and assets. |

Build it with cargo-leptos using the `[package.metadata.leptos]`
configuration in `Cargo.toml` (`output-name = "zlayer-web"`,
`bin-features = ["ssr"]`, `lib-features = ["hydrate"]`,
`reload-port = 3001`).

## Cargo features

| Feature   | Effect                                                                                                  |
|-----------|---------------------------------------------------------------------------------------------------------|
| `ssr`     | Enables the server binary: `leptos/ssr`, `leptos_axum`, `tokio`, `axum`, `tower-http`, `wasmtime`/`wasmtime-wasi`/`wat`, plus the `zlayer-agent`, `zlayer-spec`, `zlayer-scheduler`, and `zlayer-observability` integrations. |
| `hydrate` | Enables the WASM client: `leptos/hydrate`, `console_error_panic_hook`, `gloo-timers`.                    |

The crate compiles to both `cdylib` (for the WASM bundle) and `rlib`. The
`zlayer-web` binary is gated on `required-features = ["ssr"]`.

## Platform notes

- The WASM build needs `getrandom` with the `wasm_js` feature, which is
  enabled automatically via the `cfg(target_arch = "wasm32")` target table.
- WASM execution in `execute_wasm*` runs entirely on the server (wasmtime
  + WASI), not in the browser; the playground page submits the module
  bytes via a server function.
- Hot reload during development uses cargo-leptos on port 3001; the SSR
  server itself listens on 3000.

## When to edit this crate

Edit pages and components under `src/app/pages` and `src/app/components`
for UI changes; edit `src/app/server_fns.rs` plus `src/server/state.rs`
when adding server-side data sources. Avoid pulling Markdown rendering or
heavyweight SSR helpers in — the project ships hand-coded `view!` macros
to keep the bundle small. Because this crate is internal and unpublished,
breaking changes do not require a workspace-wide version bump.

Repository: <https://github.com/BlackLeafDigital/ZLayer>
