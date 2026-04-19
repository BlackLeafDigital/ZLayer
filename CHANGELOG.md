# Changelog

All notable changes to this project will be documented in this file.

## [0.10.104]

### Added
- `wasm.oci: false` ZImagefile opt-out — skip OCI layout + push, still produce the raw `.wasm` with full caching / wasm-opt / adapter support.
- `docs/wasm-portability.md` — consumer compatibility matrix (wkg, Spin, wasmCloud, runwasi, ORAS, crane) + WIT-world portability guidance.
- ZImagefile `runtime: wasm` now delegates to WASM build mode with target autodetection (cargo-component, wasm32-wasip*, jco, componentize-py).
- `zlayer build` with `-t <tag>` now pushes WASM OCI artifacts to the registry, matching the container push flow.
- Integration test `wasm_oci_e2e` verifies the WASM builder->OCI pipeline end-to-end (manifest artifactType, layer media type, runtime routing).
- Docker-compat server endpoints: per-container stop/kill/start/restart/exec
  under `/api/v1/containers/{id}/{action}`; `POST /api/v1/images/{tag,pull}`
  for single-image ops. OpenAPI coverage included.
- `Runtime::kill_container(signal)` and `Runtime::tag_image(src, dst)` added on all backends (youki, docker, macos_*, wasm). Exposed as `POST /api/v1/containers/{id}/kill` and `POST /api/v1/images/tag`. Unblocks full Docker CLI compatibility (kill/tag).
- `zlayer docker` image subcommands (images, rmi, tag, pull, push, build) now work — bridged via zlayer-client. `build` maps Docker args to BuildSpec and returns the build id.
- `zlayer docker` container subcommands (ps, logs, stop, kill, start, restart, rm, exec, run) now work — bridged to the daemon via `zlayer-client`. `run` converts Docker args to a ServiceSpec and deploys.
- `zlayer docker compose up/down/ps/logs` now work end-to-end. `up` converts compose YAML to DeploymentSpec via existing `compose_to_deployment` and deploys via `DaemonClient`.
- Docker Engine API compat: `GET /info` returns real container/image counts, host OS/arch/ncpu. `GET /events` emits a live SSE-style event stream.
- Docker Engine API compat: real `GET /images/json`, `GET /images/{name}/json`, `POST /images/create` (pull), `POST /images/{name}/tag`, `DELETE /images/{name}`. `docker images`, `docker pull`, `docker tag`, `docker rmi` now work.
- Docker Engine API compat: real `GET /containers/json`, `GET /containers/{id}/json`, `GET /containers/{id}/logs`, `POST /containers/{id}/{start,stop,kill,restart}`, `DELETE /containers/{id}`. `docker ps`, `docker logs`, `docker start/stop/kill/restart/rm` now work against the zlayer-managed socket.
- Python binding (`zlayer-py`): new `zlayer.Client(socket=None, host=None)`
  class for driving a remote zlayer daemon from Python
  (ps/deploy/logs/exec/build/stop/status/scale). Unix-only (the daemon
  socket is Unix).
- Python binding: `zlayer.ensure_daemon(system=False, version=None)` bootstraps the zlayer binary from Python (userland to `~/.local/share/zlayer/bin/`; `system=True` runs `sudo zlayer daemon install`).
- Example: `examples/python/deploy_hello.py` — complete end-to-end example deploying a trivial service via the Python binding.
- Python binding: `tests/test_client.py` integration tests spawn a real daemon and exercise `Client` end-to-end (ps, status, deploy/stop).
- Python binding packaged for PyPI (full `pyproject.toml` metadata, `zlayer` distribution name, abi3-py38 wheels).
- Internal: DaemonClient extracted to zlayer-client library crate for reuse
  by zlayer-docker / zlayer-py / language SDKs. No behavior change.
- `zlayer-client` gains per-container stop/start/restart/exec and server-side `pull_image` + `start_build` methods, matching the Docker-compat and Python SDK needs.
- `zlayer-client::DaemonClient` gains `kill_container(id, signal)` and `tag_image(src, dst)` methods, completing the Docker-compat API surface.
- OpenAPI spec coverage: regenerated `openapi.json` now documents all REST domains (>=50 paths including nodes, overlay, tunnels, networks, proxy, cluster, environments, variables, projects, webhooks, credentials, groups, permissions, audit, users, tasks, workflows, notifiers, syncs, volumes, storage, cron, jobs, and the existing deployments/services/builds/secrets).
- Generated TypeScript REST client at `clients/typescript/` (from `openapi.json`, 117 paths, typescript-fetch generator). Ready for a façade package to wrap.
- New npm package `@zlayer/client` (at `clients/typescript-client/`): ergonomic TypeScript SDK wrapping the generated OpenAPI client. Methods: deploy, ps, logs, exec, build, stop, status, scale, kill, tag. `ensureDaemon()` optional binary bootstrap (userland default, `system: true` escalates).
- CI: shell completions (bash/zsh/fish/powershell/elvish) emitted during release builds and bundled into every platform's release artifacts.
- CI: `.forgejo/workflows/python-publish.yml` builds wheels for manylinux x86_64/aarch64, macOS x86_64/aarch64, and Windows, and publishes `zlayer` to PyPI on version tags.
- CI: `.forgejo/workflows/node-publish.yml` publishes `@zlayer/client` (and `@zlayer/api-client`) to npm on version tags.

### Changed
- CI: SDK publishing moved to `.forgejo/workflows/publish-sdks.yml`, guarded by `detect-changes` registry-check. `release.yml` triggers it via workflow_dispatch. `python-publish.yml` and `node-publish.yml` folded in. New `.forgejo/build-config.yaml` defines the `sdks` service group.
- WASM builds via `zlayer build` now produce a real OCI artifact
  (`artifactType: application/vnd.wasm.component.v1+wasm` or
  `module.v1+wasm`) instead of a bare `.wasm` file with null `oci_path`. The
  builder writes a full OCI image layout (`oci-layout`, `blobs/sha256/...`,
  `index.json`) next to the compiled WASM binary in a `<module>-oci`
  directory, and `BuiltImage.image_id` now uses the first user tag when
  present or a proper `wasm:sha256:...` manifest digest ref as a fallback
  (previously the image id embedded the full filesystem path).

### Fixed
- CI: dev pushes now actually dispatch `build.yml` after `auto-tag`. The
  earlier removal of the direct dispatch assumed the `trigger-e2e → e2e.yml
  → trigger-build` chain would handle it, but that chain is gated on
  `inputs.version != ''` which is always empty for branch pushes, so
  `build.yml` never ran. Restored the explicit `curl` dispatch inside the
  `auto-tag` job, passing the freshly-created `vX.Y.Z` as `version`. Also
  removed all unreachable `startsWith(github.ref, 'refs/tags/')`
  conditionals from ci.yaml — the workflow has no `tags:` filter, so those
  branches were dead code and were what made the broken reasoning look
  plausible.

## [0.10.103]

### Added
- **Shell completion generation.** New `zlayer completions <SHELL>` subcommand
  emits a completion script for `bash`, `zsh`, `fish`, `powershell`, or
  `elvish` to stdout, built from the live clap command tree. Install with
  e.g. `zlayer completions bash > /etc/bash_completion.d/zlayer`.
- Install scripts now drop shell completions for bash, zsh, fish (Linux/macOS)
  and PowerShell (Windows) after installing the binary. Fail-soft: install
  succeeds even if completion drop fails.

### Fixed
- Forgejo CI: `build.yml` now uses `christopherhx/gitea-upload-artifact` and `christopherhx/gitea-download-artifact` forks (upstream `actions/upload-artifact@v4` and `actions/download-artifact@v4` error with `GHESNotSupportedError` on Forgejo).
- TUI build progress counter denominator (`[X/Y] instructions`) previously grew in lockstep with the numerator because totals were summed from events as they arrived; now emitted once up-front via `BuildEvent::BuildStarted { total_stages, total_instructions }`.
- **Latent clap CLI-definition bugs that made `zlayer completions` (and
  `--help` on the affected subcommands) panic in debug builds.** Four
  issues in `bin/zlayer/src/cli.rs` surfaced by `clap_complete::generate`
  walking the whole subcommand tree:
  - `manager init`'s custom `--version` flag collided with clap's
    auto-injected `--version`; now annotated with `disable_version_flag`.
  - `job trigger`, `job status`, and `cron status` declared a non-required
    positional (`deployment`) before a required one (`job`/`cron`).
    `deployment` is now a `--deployment` flag, matching the pattern
    already used by `zlayer logs`.
  - The global `--detach` flag's `short = 'd'` collided with docker
    compat's `docker volume create -d <driver>`. The short form is
    dropped (use `--detach`); `-b/--background` is unaffected.

## [0.10.102]

### Fixed
- **Container log-follow SSE now terminates when the container exits.**
  `GET /api/v1/containers/{id}/logs?follow=true` previously polled forever,
  keeping the SSE body open even after the container had exited. Clients
  (notably ZArcRunner) blocked indefinitely on the next read. The follow
  stream now checks `Runtime::container_state` every fourth poll (plus the
  initial poll), performs a final log drain on observing `Exited` or
  `Failed`, and closes the stream. Detection latency ≤ 2s. Covered by
  `container_log_follow_stream_terminates_on_exit`.
- **`zlayer container logs` no longer fails with "Failed to parse JSON
  response from daemon".** The `/logs` (non-follow) endpoint returns plain
  text, but the daemon client was calling `serde_json::from_slice` on the
  body. `DaemonClient::get_container_logs` now returns `Result<String>`
  via a new `decode_text_body` helper; the caller in `commands/container`
  was simplified to a direct print. Covered by
  `container_logs_cli_decodes_plain_text` and
  `container_logs_cli_rejects_invalid_utf8`.

## [0.10.101]

### Added
- **`crates/zlayer-proxy/README.md`** so the crate's `Cargo.toml`
  `readme = "README.md"` line points at an actual file.

### Changed
- **CI workflows (`.forgejo/workflows/ci.yaml`, `.forgejo/workflows/build.yml`)
  now use the `setup-system-deps` action for all package installs.**
  Replaced inline `apt-get install`, `brew install`, and `choco install`
  steps with
  `https://forge.blackleafdigital.com/Public/actions/setup-system-deps@main`
  so apt runs get dpkg-lock timeout + noninteractive guards uniformly
  and OS detection / package-manager selection lives in one place.
  The Windows `build-windows-amd64` job also gains an
  `install-git-bash` step after `setup-rust`, which installs Git on the
  Forgejo Windows runner and fixes the `Cannot find: bash in PATH`
  failure that was blocking the protobuf and packaging `shell: bash`
  steps. `ci.yaml` also now installs build deps explicitly on every
  cargo job rather than relying on the runner image to preinstall
  `protoc` / `libseccomp-dev`.

### Fixed
- **`zlayer-git` reflog failure on machines without global git config.**
  `pull_ff` and `checkout` previously panicked with
  `"reflog messages need a committer which isn't set"` on CI runners
  and inside fresh containers / rootless sandboxes — any environment
  without a `user.name` / `user.email` in `~/.gitconfig` or the
  `GIT_COMMITTER_*` env vars. gix 0.81 is stricter than the `git` CLI
  (which silently falls back to `whoami@hostname`) and refuses
  reflog-writing ref updates without an identity. A new internal
  `ensure_committer` helper installs an in-memory fallback
  (`ZLayer <zlayer@localhost>`) on each opened `Repository` when none
  is configured; applied at every blocking entry point that writes
  reflogs (`fetch`, `pull_ff`, `checkout`). Per-instance mutation via
  `Repository::config_snapshot_mut` — no env vars, no global mutex.
  Caller-supplied identities (user config, env vars) take precedence.

## [0.10.100]

### Added
- **Manager UI `/projects` + `/projects/:id` pages**
  (`crates/zlayer-manager/src/app/pages/projects.rs` and
  `project_detail.rs`). The largest page in the Manager — a list view
  with a nested detail route. List page has a single table (Name, Git
  URL, Branch, Auto-deploy badge, Updated, Actions); row clicks anywhere
  except the Delete button navigate to `/projects/{id}` via
  `use_navigate()`. The **New project** modal drives
  `manager_create_project` with name + git URL + branch + build-kind
  selector (dropdown of `dockerfile | compose | zimagefile | spec`,
  matching the daemon's `BuildKind` exactly) + build path + auto-deploy
  toggle + poll-interval slider (0–3600s, step 30, 0 = disabled).
  Detail page uses the shared `TabBar` with four tabs: **Source**
  (read-only git URL, editable branch, **Pull now** button that drives
  `POST /api/v1/projects/{id}/pull` and shows the resulting
  `ProjectPullResponse` inline plus the truncated HEAD SHA), **Build**
  (segmented build-kind buttons, build path, deploy spec path,
  auto-deploy toggle, poll-interval slider, Save button), **Credentials**
  (merged registry + git credential picker populated from
  `GET /api/v1/credentials/registry` and `/api/v1/credentials/git`;
  attach writes `registry_credential_id` / `git_credential_id` via
  `PATCH /api/v1/projects/{id}`; detach sends the empty string to clear
  — see `update_project` in `handlers/projects.rs` for the empty-string
  semantics), and **Envs & Deployments** (linked-deployments table
  backed by `GET /api/v1/projects/{id}/deployments`, with a **Link
  deployment** name picker and an unlink confirmation modal, plus a
  webhook card showing the template URL + masked secret with a
  Show/Hide toggle and a **Rotate webhook secret** button gated by a
  warning `ConfirmDeleteModal`). Eleven server fns added in
  `app/server_fns.rs`: `manager_list_projects`, `manager_get_project`,
  `manager_create_project`, `manager_update_project`,
  `manager_delete_project`, `manager_pull_project`,
  `manager_list_project_deployments`,
  `manager_link_project_deployment`,
  `manager_unlink_project_deployment`, `manager_get_project_webhook`,
  `manager_rotate_project_webhook`, plus a `manager_list_credentials`
  helper that merges both credential endpoints into a single
  `WireProjectCredential` shape for the picker. Wire types live in
  `crates/zlayer-manager/src/wire/projects.rs`: `WireProject`,
  `WireBuildKind` (four variants matching the daemon's `BuildKind`),
  `WireProjectSpec` (shared create + patch body with
  `skip_serializing_if = "Option::is_none"` everywhere so partial
  patches stay minimal), `WirePullResult`, `WireWebhookInfo` (mirrors
  `WebhookInfoResponse` — URL + secret), `WireProjectCredential`. Nine
  new route smoke tests in `tests/routes_200.rs` cover the full and
  minimum project shapes, all four build-kind wire encodings, the
  `WireProjectSpec` omit-none serialisation, pull-result round-trip,
  webhook-info round-trip, the `parse_wire` helper, and compile-check
  guards for both `Projects` and `ProjectDetail`. Sidebar entry added
  under the existing "Workspace" section using the Heroicons outline
  `folder` icon; routes `/projects` and `/projects/:id` mounted behind
  `<AuthGuard>` in `app/mod.rs`; pages exported via
  `app/pages/mod.rs::{Projects, ProjectDetail}`.
- **Manager UI `/audit` page**
  (`crates/zlayer-manager/src/app/pages/audit.rs`). Admin-only,
  read-only filterable/paginated audit log viewer backed by
  `GET /api/v1/audit`. Filter bar exposes the four daemon-supported
  predicates — user id (free-form, substring match on the server),
  resource kind (free-form string — the Manager deliberately does NOT
  pin this to a dropdown because new kinds get added without a client
  upgrade), since, and until — with "Apply" committing the form into
  the filter signals so typing doesn't spam the daemon. `datetime-local`
  inputs are converted to RFC-3339 UTC at apply time (`YYYY-MM-DDTHH:MM`
  → `YYYY-MM-DDTHH:MM:00Z`) so `chrono::DateTime<Utc>` on the daemon
  can parse them. Pagination is client-side because the daemon's list
  endpoint returns a bare `Vec<AuditEntry>` with no `{entries, total}`
  envelope or `offset` parameter — we over-fetch
  `(page + 1) * PER_PAGE + 1` rows at `limit = <N>` and slice the
  current page out client-side; the trailing "probe row" decides
  whether "Next" should be enabled. `PER_PAGE = 50` is a constant in
  the page module. Row shape: Time (pretty-formatted, full RFC-3339 in
  the `title=` tooltip), User (short 8-char prefix of the user id,
  full id in the tooltip), Action (badge), Resource (`kind:id` when the
  entry has a specific resource id, bare `kind` otherwise), IP, Details
  (collapsed `<details>/<summary>` disclosure showing a pretty-printed
  JSON block; the `user-agent` string is folded into the expanded
  pane). Admin gate at the component level: non-admins see an
  `alert-error` "Admin access required" banner and the page never
  issues a server call; the backend independently enforces admin-only
  via `actor.require_admin()?` in the handler. One server fn added to
  `app/server_fns.rs` — `manager_list_audit(filter: WireAuditFilter)
  -> Vec<WireAuditEntry>` — builds the query string by appending only
  non-empty `Option` fields (so `None` maps to "param omitted", not
  `?key=` which the daemon would deserialize as `Some("")`), with a
  local `pct_encode_query()` helper for RFC 3986 unreserved-set
  encoding of value strings. Wire types (`WireAuditEntry`,
  `WireAuditFilter`) live in `crates/zlayer-manager/src/wire/audit.rs`
  with round-trip tests in `tests/routes_200.rs` covering the full
  shape, explicit-null optionals, missing-optionals-via-serde-default,
  and `WireAuditFilter::default()` round-trip. Page-local unit tests
  cover the datetime-local → RFC-3339 conversion (with + without
  seconds + already-Z-terminated + empty), the pretty-printed
  timestamp formatter, the short-id truncator, the `page_has_next()`
  probe-row logic, and `describe_details()` for the summary preview
  (object / array / primitive / overflow-of-4+ keys). Sidebar entry
  added under the existing "System" section using the Heroicons
  outline `clipboard-document-list` icon, hidden for non-admin
  sessions via the same `CurrentUser::is_admin()` derived class that
  Groups and Permissions use; route `/audit` mounted behind
  `<AuthGuard>` in `app/mod.rs`; page exported via
  `app/pages/mod.rs::Audit`.
- **Manager UI `/permissions` page**
  (`crates/zlayer-manager/src/app/pages/permissions.rs`). Admin-only
  flat-list view of subject→resource access grants. Filter bar narrows
  by subject kind (`all | user | group`), subject id (substring match),
  and resource kind (dropdown of the eight kinds the daemon recognises:
  `deployment | project | secret | task | workflow | notifier |
  variable | environment`), all applied client-side on a union of the
  per-subject grant lists. This approach was dictated by the daemon's
  narrow filter surface (`GET /api/v1/permissions` accepts EXACTLY one
  of `?user=<id>` or `?group=<id>`) — we fan out one request per
  user/group rather than inventing a query parameter the server
  doesn't understand. Grant modal has cascading pickers: flipping the
  subject-kind toggle swaps between a user `<select>` (populated from
  `manager_list_users`, labelled `email (id)`) and a group `<select>`
  (from `manager_list_groups`, labelled `name (id)`), with a free-form
  text fallback if either list hasn't loaded yet. Resource kind is a
  fixed dropdown; resource id is a free-form text input (blank ==
  wildcard grant). Level selector exposes `read | execute | write`
  (the daemon also accepts `none` but a `none` grant is never useful).
  Revoke is an inline confirmation dialog that names the subject /
  resource / level to prevent fat-finger revocations. Admin gate at
  the component level: non-admins see an `alert-warning` "Admin role
  required" banner and no server calls are issued; the backend
  independently enforces admin-only on `POST` and `DELETE`. Three
  server fns added to `app/server_fns.rs` —
  `manager_list_permissions_for_subject`, `manager_grant_permission`,
  `manager_revoke_permission` — all cookie/CSRF-forwarded via
  `raw_request`. Wire types (`WirePermission`,
  `WireGrantPermissionRequest`) live in
  `crates/zlayer-manager/src/wire/permissions.rs` with round-trip
  tests in `tests/routes_200.rs` covering the specific-resource shape,
  the wildcard (`resource_id: null`) shape, and the `skip_serializing_if`
  on the grant request so a wildcard grant's JSON body omits
  `resource_id` entirely (matching the daemon's `#[serde(default)]`
  on `GrantPermissionRequest.resource_id`). Eight unit tests cover
  filter composition, subject/resource label formatting, and the
  fallback-to-raw-id behaviour when a grant's subject was deleted
  after the grant was created. Sidebar entry added under the existing
  "System" section using the Heroicons outline `shield-check` icon,
  hidden for non-admin sessions via the same `CurrentUser::is_admin()`
  derived class the Groups link uses; route `/permissions` mounted
  behind `<AuthGuard>` in `app/mod.rs`; page exported via
  `app/pages/mod.rs::Permissions`.
- **Manager UI `/groups` page**
  (`crates/zlayer-manager/src/app/pages/groups.rs`). Admin-only
  master-detail layout for user group management. Left column is a
  clickable groups table (name + description + Edit/Delete actions).
  Right column shows the selected group's detail: a header card with
  name/description/created timestamp, an "Add member" picker populated
  from `manager_list_users` minus the current membership, and a
  members table with display_name + email + Remove actions. The
  daemon's `GET /api/v1/groups/{id}/members` endpoint returns only
  `user_id` values — names/emails are joined client-side against the
  users resource; unknown ids are rendered verbatim as `(unknown
  user)` rather than dropped. Seven server fns added to
  `app/server_fns.rs`: `manager_list_groups`, `manager_create_group`,
  `manager_update_group` (PATCH, supported by the daemon),
  `manager_delete_group`, `manager_list_group_members`,
  `manager_add_group_member`, `manager_remove_group_member` — all
  cookie/CSRF-forwarded via `raw_request`. Wire types
  (`WireUserGroup`, `WireGroupMembers`, `WireCreateGroup`,
  `WireUpdateGroup`, `WireAddMember`) live in
  `crates/zlayer-manager/src/wire/groups.rs` with round-trip tests in
  `tests/routes_200.rs` (null description, empty members list,
  skip-serializing partial patches). Admin gate at the component
  level — non-admins see an `alert-error` "Admin access required"
  banner and no server calls are issued; the backend independently
  enforces admin-only on every mutation. Sidebar entry added under
  the existing "System" section using the Heroicons outline
  `user-group` icon, hidden entirely for non-admin sessions via a
  `CurrentUser::is_admin()` derived class so regular users don't see
  a link that would only show them an error page; route `/groups`
  mounted behind `<AuthGuard>` in `app/mod.rs`; page exported via
  `app/pages/mod.rs::Groups`.
- **Manager UI `/workflows` page**
  (`crates/zlayer-manager/src/app/pages/workflows.rs`). Master-detail
  layout for the daemon's named step-DAGs: left column is a clickable
  workflow table (name + step count + updated timestamp), right column
  shows the selected workflow's steps as a DaisyUI `steps steps-vertical`
  rail (semantic action badges only — `badge-primary` for run_task,
  `badge-secondary` for build_project, `badge-accent` for deploy_project,
  `badge-info` for apply_sync), a Run button with an in-flight
  spinner/disabled state, and a "Recent runs" panel where each row
  expands inline to reveal the per-step breakdown with `step-success` /
  `step-error` / `step-neutral` coloring keyed on the daemon's
  `StepResult.status` strings (`"ok" | "failed" | "skipped"`). Step
  output/error text renders in a `mockup-code` block beneath each step
  label. The Run button surfaces the finished run in a dedicated
  `RunResultModal` (for a side-by-side view) while ALSO prepending it to
  the history list so the user can keep browsing. Create modal is the
  hardest form in the manager so far — a dynamic `Vec<StepDraft>` backed
  by per-step `RwSignal`s for name, kind selector (run_task /
  build_project / deploy_project / apply_sync), variant-specific id
  field, and optional on_failure task id. Each draft carries a stable
  `key: u64` so move-up / move-down reorders don't re-mount inputs and
  lose focus. On submit the drafts are built into a
  `Vec<WireWorkflowStep>` via per-variant validation that surfaces
  inline as `alert-error` text before hitting the daemon. Edit modal is
  intentionally omitted because the daemon exposes no
  `PATCH /api/v1/workflows/{id}` endpoint (to change a workflow, delete
  and recreate — same pattern Tasks uses), consistent with the "match
  daemon API shapes exactly" ground rule. Six server fns added to
  `app/server_fns.rs` — `manager_list_workflows`, `manager_get_workflow`,
  `manager_create_workflow`, `manager_delete_workflow`,
  `manager_run_workflow`, `manager_list_workflow_runs` — all
  cookie/CSRF-forwarded through `raw_request`. Wire types
  (`WireWorkflow`, `WireWorkflowStep`, `WireWorkflowAction`,
  `WireWorkflowRun`, `WireStepResult`, `WireWorkflowSpec`) live in
  `crates/zlayer-manager/src/wire/workflows.rs` with round-trip tests in
  `tests/routes_200.rs` covering the `{"type": "<snake_case>"}` action
  tagging, null `project_id` on global workflows, and null `finished_at`
  for still-running runs. Sidebar entry added under the existing
  "CI/CD" section using the Heroicons outline `rectangle-stack` icon;
  route `/workflows` mounted behind `<AuthGuard>` in `app/mod.rs`; page
  exported via `app/pages/mod.rs::Workflows`.
- **Manager UI `/tasks` page**
  (`crates/zlayer-manager/src/app/pages/tasks.rs`). Master-detail layout
  for the daemon's named runnable scripts: left column is a clickable
  task table, right column shows the selected task's body, a Run button
  with an in-flight spinner/disabled state, a "Recent runs" table
  (truncated run id + started timestamp + computed duration + colored
  exit-code badge), and a "Latest output" panel that renders the most
  recent run's stdout/stderr in `mockup-code` blocks. Selection is
  stored as an id in a `RwSignal<Option<String>>` so it survives list
  refetches. Runs live in a page-local `RwSignal<Vec<WireTaskRun>>`
  that seeds from `GET /api/v1/tasks/{id}/runs` whenever the selection
  changes and prepends the newly-returned run after each successful
  `POST /api/v1/tasks/{id}/run` call — no polling. Includes a
  dependency-free RFC-3339 parser (`rfc3339_to_unix_ms`,
  `ymd_to_unix_days`) so the WASM/hydrate build doesn't pull in chrono,
  matching the same pattern `secrets.rs` established. Create + Delete
  modals are wired; an Edit modal is intentionally omitted because the
  daemon exposes no `PATCH /api/v1/tasks/{id}` endpoint (to change a
  task, delete and recreate), consistent with the "match daemon API
  shapes exactly" ground rule. Six server fns added to
  `app/server_fns.rs` — `manager_list_tasks`, `manager_get_task`,
  `manager_create_task`, `manager_delete_task`, `manager_run_task`,
  `manager_list_task_runs` — all cookie/CSRF-forwarded through
  `raw_request`. Wire types (`WireTask`, `WireTaskRun`, `WireTaskSpec`)
  live in `crates/zlayer-manager/src/wire/tasks.rs` with round-trip
  tests in `tests/routes_200.rs` (null `project_id`, null
  `exit_code`/`finished_at` for still-running jobs). Sidebar entry
  added under the existing "CI/CD" section using the Heroicons outline
  `play` icon; route `/tasks` mounted behind `<AuthGuard>` in
  `app/mod.rs`; page exported via `app/pages/mod.rs::Tasks`.
- **Manager UI `/secrets` page**
  (`crates/zlayer-manager/src/app/pages/secrets.rs`). Environment-scoped
  secrets management with an env tab filter (All + one per environment,
  including project-scoped envs via `?project=*`), a masked value column
  with per-row reveal that auto-hides after 10 seconds (via
  `gloo_timers::callback::Timeout` on hydrate, no-op on SSR), inline
  create/edit/delete modals, and a bulk `.env` import modal with a
  client-side `KEY=value` parser that mirrors the daemon's quote-stripping
  and `export ` handling. Uses the daemon's existing
  `GET/POST/DELETE /api/v1/secrets`, `GET /api/v1/secrets/{name}?reveal=true`,
  and `POST /api/v1/secrets/bulk-import` endpoints — no new API surface.
  Wire types (`WireSecret`, `WireEnvironment`, `BulkImportResult`) live in
  `crates/zlayer-manager/src/wire/secrets.rs` and are guarded by
  round-trip tests in `tests/routes_200.rs`. Sidebar entry added under the
  "Workspace" section using the Heroicons outline `key` icon; route wired
  at `/secrets` behind `<AuthGuard>`.
- **Manager UI `/variables` page**
  (`crates/zlayer-manager/src/app/pages/variables.rs`). First page of the
  Phase 4-8 Manager UI roll-out: a DaisyUI table of workspace variables
  with modals for creating, editing, and deleting. Edits push through the
  existing `PATCH /api/v1/variables/{id}` endpoint; creates use
  `POST /api/v1/variables` with an optional `scope` id (blank = global).
  Wired into the sidebar under a new "Workspace" section (above "CI/CD")
  and registered at `/variables` behind `<AuthGuard>`; the daemon still
  enforces admin-only on every mutation so non-admins see server-side
  errors rather than a hard client-side gate.
- **Shared Manager UI scaffolding used by the Variables page and every
  subsequent Phase 4-8 page**:
  - `crates/zlayer-manager/src/app/util/errors.rs` — `humanize_error`
    (moved out of `pages/bootstrap.rs`) plus a new `format_server_error`
    convenience wrapper that takes a `ServerFnError` directly. Tests
    cover the 409/bootstrap/network branches.
  - `crates/zlayer-manager/src/app/components/forms.rs` — five reusable
    modal/form primitives: `ConfirmDeleteModal`, `TextFieldModal`,
    `FormField`, `TabBar`, `Pagination`. DaisyUI semantic classes only;
    no hand-picked colors. Reactive signals for the "submitting" flag
    and optional error banner.
  - `crates/zlayer-manager/src/wire/` — new top-level module for wire
    types shared between SSR and hydrate builds. Contains
    `common::ErrorBody` and `variables::WireVariable`. Cannot depend on
    `zlayer-api` because hydrate compiles as WASM and has no access to
    that crate; the round-trip test in `tests/routes_200.rs` guards
    against field drift.
- **Real workflow action execution for `BuildProject`, `DeployProject`, and
  `ApplySync`** (`crates/zlayer-api/src/handlers/workflows.rs`). Previously
  these three `WorkflowAction` arms were log-only — the daemon printed a
  "would execute …" line and returned a placeholder string without touching
  anything. They now drive the real pipeline end-to-end:
  - `BuildProject` looks up the project, ensures the git working copy is
    cloned / fast-forwarded at `{clone_root}/{project_id}` (reusing the
    shared `ensure_project_clone` helper with HTTP pulls), registers a new
    build against the shared `BuildManager`, kicks off `spawn_build` (the
    same code path the `/api/v1/build/json` HTTP endpoint uses), blocks on
    `BuildManager::wait_for_build` for terminal status, and propagates
    `Complete` as step-ok / `Failed` as step-failed so the workflow's
    `on_failure` handler (if any) still gets a chance to run.
  - `DeployProject` requires a new `StoredProject::deploy_spec_path` field;
    when set, it reads that file from the cloned working copy, parses it as
    a `DeploymentSpec` (via `zlayer_spec::from_yaml_str`, exactly like the
    HTTP deployments endpoint and sync apply), upserts the stored
    deployment, and links it to the project so "list project deployments"
    surfaces the new record. When `deploy_spec_path` is `None` the step
    fails with a clear, actionable error message rather than silently
    succeeding.
  - `ApplySync` delegates directly to the shared `apply_sync_inner` helper,
    so workflow-triggered applies produce the same per-resource reconcile
    results (create / update / delete / skip) and the same
    `last_applied_sha` bookkeeping as a `POST /syncs/{id}/apply` HTTP
    request. The step output is the JSON-serialized `SyncApplyResponse` so
    downstream consumers can parse structured results rather than regexing
    a printf'd string.
- **`BuildManager::wait_for_build`** (`crates/zlayer-api/src/handlers/build.rs`).
  New async helper that blocks until a registered build reaches a terminal
  state (`Complete` or `Failed`), polling every 200ms with a hard 1-hour
  ceiling. Returns the final `BuildStatus` for the caller to inspect, or
  `None` when the build id is unknown. The workflow `BuildProject` action
  uses it to turn the existing fire-and-forget `spawn_build` pipeline into
  a synchronous step — no handler duplication, no alternate build code
  path. Covered by two new unit tests: one asserts `Complete` propagates
  from a concurrent writer, one asserts unknown ids return `None`.
- **`WorkflowsState` extended with every handle workflow steps can reach:**
  `project_store`, `deployment_store`, `sync_store`, `build_manager`,
  `clone_root`, and (optional) `git_creds`. The old two-field constructor
  `WorkflowsState::new(store, task_store)` is replaced with the full
  seven-argument form; `serve.rs` hands them through from the existing
  `StorageBundle` + `BuildState` + the shared `{data_dir}/projects` clone
  root (same path `ProjectState` and `SyncState` already resolve, so every
  surface that pulls a project's git repo ends up in the same place).
  A chainable `with_git_creds(...)` method attaches a credential store
  when private repo auth is required for `BuildProject`.
- **`StoredProject::deploy_spec_path: Option<String>`** (`crates/zlayer-api/src/storage/mod.rs`).
  New optional field that tells workflow `DeployProject` which YAML inside
  the cloned working copy to parse and apply. Defaults to `None` (explicit
  opt-in) and is serde `#[serde(default)]` so existing project rows in
  `projects.db` deserialize without a schema migration — the
  `SqlxProjectStore` serializes the whole struct as `data_json` so only
  the JSON shape matters. `CreateProjectRequest` / `UpdateProjectRequest`
  both accept the new field; passing `""` on update clears it.
- **`pub(crate)` helper `ensure_project_clone`** (`crates/zlayer-api/src/handlers/projects.rs`).
  Extracted from the HTTP `pull_project` handler so the workflow
  `BuildProject` action uses the exact same clone / fast-forward / auth
  resolution code path — no divergence between manual `POST /pull` and
  workflow-triggered builds.
- **Manager UI `/notifiers` page**
  (`crates/zlayer-manager/src/app/pages/notifiers.rs`). DaisyUI table of
  configured notifiers with columns for Name, Type (badge colored per
  kind — slack/primary, discord/secondary, webhook/accent, smtp/info),
  Enabled (clickable badge toggles via PATCH), Updated, and Actions
  (Test / Edit / Delete). Establishes the **discriminated-union form
  pattern** that future pages (workflows, syncs) should follow when they
  pick up similarly shaped tagged unions: a single `<select>` drives the
  active variant, variant-specific fields render conditionally, and
  switching variants clears the other variants' inputs so stale state
  never leaks into the POST body. The final `WireNotifierConfig` is
  built via a `match` on the selected variant. A per-row Test modal
  shows a spinner while `POST /api/v1/notifiers/{id}/test` is in flight,
  then surfaces the daemon's `NotifierTestResult` as an
  `alert-success`/`alert-error` with the returned message — the daemon
  returns upstream failures as HTTP 200 + `success: false`, so the UI
  never confuses "reachable but misconfigured" with "daemon unreachable".
  - Five server fns in `app/server_fns.rs`: `manager_list_notifiers`,
    `manager_create_notifier`, `manager_update_notifier`,
    `manager_delete_notifier`, `manager_test_notifier` — all
    cookie/CSRF-forwarded through `raw_request` exactly like the
    Variables fns. `manager_create_notifier` always submits the
    daemon's `{name, kind, config}` tuple with `kind` derived from
    `WireNotifierConfig::kind()`, and immediately follows with a PATCH
    when the caller asked for `enabled: false` (the daemon's create
    handler ignores enabled and always produces `enabled: true`).
  - Wire types in `crates/zlayer-manager/src/wire/notifiers.rs`
    (`WireNotifier`, `WireNotifierConfig` with
    `#[serde(tag = "type", rename_all = "snake_case")]`,
    `NotifierTestResult`). Zero dependency on `zlayer-api` — the
    hydrate/WASM build can consume these the same way it consumes
    `WireVariable`. Tests in `tests/routes_200.rs` round-trip each
    variant (slack, discord, webhook with + without optional fields,
    smtp), exercise the `kind()` helper, and assert
    `NotifierTestResult` parses both success and failure shapes.
  - Route `/notifiers` mounted behind `<AuthGuard>` in
    `app/mod.rs`; sidebar entry with a Heroicons `bell` icon in the
    existing "CI/CD" section of `app/components/sidebar.rs`; page
    exported via `app/pages/mod.rs::Notifiers`.

### Changed
- **`BuildManager::spawn_build` is now `pub(crate)`** so the workflow
  `BuildProject` action can invoke it directly. No behavior change; the
  function was previously a private helper inside the build handler.
- **`serve.rs` constructs `BuildState` and the shared `projects` clone root
  earlier** so `ProjectState`, `SyncState`, and the new `WorkflowsState` all
  reference the same `Arc<BuildManager>` and the same on-disk clone root.
  Before this change `ProjectState` and `SyncState` defaulted to
  `std::env::temp_dir().join("zlayer-projects")`, which survived nothing
  across reboots; they now use `{data_dir}/projects`.

### Fixed
- Workflow runs that included `BuildProject`, `DeployProject`, or `ApplySync`
  steps previously reported `"ok"` with a `"would execute …"` output even
  though nothing actually ran. Admins relying on workflows to automate
  builds / deploys / syncs would see green step results and no error, but
  find their deployments stale and registries empty. Those three arms now
  execute the real action and propagate failures the way every other
  workflow step does.

## [0.10.99]

### Added
- **Persistent `SQLite` backend for audit log storage (`SqlxAuditStore`).**
  Audit entries (who did what, when, to which resource) now survive daemon
  restarts. The new store lives in `crates/zlayer-api/src/storage/audit.rs`
  alongside the existing `InMemoryAuditStore` (kept for unit tests); both back
  the same `AuditStorage` trait so callers are backend-agnostic. Re-exported
  from `storage::mod` as `SqlxAuditStore`.
  - Hand-rolled (not built on `SqlxJsonStore`) because the filter is a
    multi-field range query — `user_id`, `resource_kind`, and `[since, until]`
    on `occurred_at` — rather than the single-column-peel pattern the generic
    adapter is tuned for. `list()` builds its dynamic `WHERE` clause via
    `sqlx::QueryBuilder` with parameterised binds, so filter composition is
    injection-safe.
  - Append-only schema: `audit_log(id PK, user_id, resource_kind, resource_id,
    action, data_json, occurred_at)`. Three supporting indexes —
    `idx_audit_user (user_id, occurred_at DESC)`,
    `idx_audit_resource (resource_kind, occurred_at DESC)`, and
    `idx_audit_time (occurred_at DESC)` — so every common filter shape
    (per-user timeline, per-kind timeline, global recent) serves from an
    ordered index scan rather than an in-memory sort. The canonical
    `AuditEntry` is also stored as `data_json` so new optional fields
    (e.g. `ip`, `user_agent`, `details`) can be added without a schema
    migration.
  - `list()` semantics match `InMemoryAuditStore` exactly: `since`/`until` are
    both inclusive bounds on `occurred_at`, `user_id` and `resource_kind` are
    strict-equality filters, results are ordered by `occurred_at DESC`, and
    `limit` is applied at the SQL level (so a `limit = 5` over a million rows
    stops at 5 without materialising the rest).
  - Seven new `#[tokio::test]` cases cover no-filter reverse-chronological
    ordering, each single-field filter, the inclusive-window semantics (10
    entries spread over 10s, middle 3 captured), the limit cap, the full
    combined filter (user + kind + window with negative cases for each
    axis), and a round-trip through an on-disk database to confirm the
    RFC3339 timestamp, `details` JSON, `ip`, and `user_agent` all survive
    close-and-reopen.
- **Persistent `SQLite` backend for task storage (`SqlxTaskStore`).**
  Named runnable scripts (tasks) and their execution records (task runs) now
  survive daemon restarts. The new store lives in
  `crates/zlayer-api/src/storage/tasks.rs` alongside the existing
  `InMemoryTaskStore` (kept for unit tests); both back the same `TaskStorage`
  trait so callers are backend-agnostic. Re-exported from `storage::mod` as
  `SqlxTaskStore`.
  - Primary `tasks` table managed by `SqlxJsonStore` with two peeled columns:
    `name` (single-column `UNIQUE`, so duplicate task names surface as
    `StorageError::AlreadyExists` directly from the adapter) and `project_id`
    (nullable, indexed) so `list(Some(project_id))` goes through `list_where`
    rather than a full-table scan.
  - Secondary `task_runs` history table, hand-rolled on the shared pool via
    the crate-private `SqlxJsonStore::pool()` accessor, with a composite
    index on `(task_id, started_at DESC)` so `list_runs` serves its
    newest-first ordering as an ordered index scan rather than an in-memory
    sort pass. `record_run` upserts by run id so re-recording the same run
    (e.g. to patch `stdout`/`finished_at` after the process exits) updates
    in place rather than duplicating.
  - `delete` runs a transaction that first wipes the task's runs in
    `task_runs`, then the row in `tasks`, so a crash mid-delete cannot leave
    orphan run rows behind.
- **`SqlxJsonStore::pool()` accessor** (crate-private) on
  `crates/zlayer-api/src/storage/sqlx_json.rs`. Exposes the underlying
  `SqlitePool` to sibling stores inside `storage::*` so they can create
  secondary tables (e.g. `task_runs`) and run cross-table transactions on
  the same pool without opening a second connection. Not part of the public
  API.
- **Persistent `SQLite` backend for user group storage (`SqlxGroupStore`).**
  User groups and their memberships now survive daemon restarts. The new
  store lives in `crates/zlayer-api/src/storage/groups.rs` alongside the
  existing `InMemoryGroupStore` (kept for unit tests); both back the same
  `GroupStorage` trait so callers are backend-agnostic. Re-exported from
  `storage::mod` as `SqlxGroupStore`.
  - Primary `groups` table managed by `SqlxJsonStore` with a single-column
    `UNIQUE(name)` constraint — duplicate-name inserts surface as
    `StorageError::AlreadyExists` straight out of the adapter.
  - Secondary `group_members` link table keyed by a composite
    `PRIMARY KEY (group_id, user_id)`, hand-rolled on the shared pool via
    the crate-private `SqlxJsonStore::pool()` accessor. A secondary
    `idx_group_members_user` index on `(user_id)` powers
    `list_groups_for_user` without a full-table scan. Double-add of the
    same pair is idempotent via `INSERT OR IGNORE`, matching
    `InMemoryGroupStore` which deduplicates through its `HashSet`.
  - `delete` runs a transaction that first wipes the group's membership
    rows in `group_members`, then the row in `groups`, so a crash
    mid-delete cannot leave orphan membership rows behind.
  - `add_member`, `remove_member`, and `list_members` preflight-check
    group existence and return `StorageError::NotFound` for unknown group
    ids, matching the existing `InMemoryGroupStore` behaviour.
- **Persistent `SQLite` backend for workflow storage (`SqlxWorkflowStore`).**
  Workflow definitions and their execution history now survive daemon
  restarts. The new store lives in `crates/zlayer-api/src/storage/workflows.rs`
  alongside the existing `InMemoryWorkflowStore` (kept for unit tests); both
  back the same `WorkflowStorage` trait so callers are backend-agnostic.
  Re-exported from `storage::mod` as `SqlxWorkflowStore`.
  - Primary `workflows` table managed by `SqlxJsonStore` with a single-column
    `UNIQUE(name)` constraint — duplicate-name inserts surface as
    `StorageError::AlreadyExists` straight out of the adapter.
  - Secondary `workflow_runs` history table, hand-rolled on the shared pool
    via the crate-private `SqlxJsonStore::pool()` accessor, with a composite
    index on `(workflow_id, started_at DESC)` so `list_runs` is an ordered
    index scan rather than a fetch-plus-sort-in-memory pass.
  - `delete` runs a transaction that first wipes the workflow's run history
    in `workflow_runs`, then the row in `workflows`, so a crash mid-delete
    cannot leave orphan run rows. The `InMemoryWorkflowStore` backend was
    updated to cascade-delete run history too, matching the new contract.
- **Persistent `SQLite` backend for variable storage (`SqlxVariableStore`).**
  Plaintext template variables now survive daemon restarts. The new store
  lives in `crates/zlayer-api/src/storage/variables.rs` alongside the
  existing `InMemoryVariableStore` (kept for unit tests); both back the same
  `VariableStorage` trait so callers are backend-agnostic. Re-exported from
  `storage::mod` as `SqlxVariableStore`.
- **Compound `UNIQUE` support in `JsonTable`.** `JsonTable<T>` gained a
  `unique_constraints: &'static [&'static [&'static str]]` field; each inner
  slice becomes a `UNIQUE(col_a, col_b, ...)` clause in the generated
  `CREATE TABLE` statement. This enables multi-column uniqueness (e.g.
  `UNIQUE(name, scope)` for variables) that `IndexSpec::unique` — a
  single-column flag — cannot express. Compound-constraint violations still
  surface as `StorageError::AlreadyExists` via the existing
  `map_sqlx_err` path.
- **Three new `SqlxJsonStore` lookup methods:**
  `list_where(column, value)` for `column = ?` filtering,
  `list_where_null(column)` for `column IS NULL` filtering, and
  `list_where_opt(column, Option<&str>)` as the unified primitive that
  accepts either. All three reject unknown columns with `StorageError::Other`
  (not a raw `Database` string) by validating against the static index list,
  matching the `get_by_unique` contract. Results are ordered by `id` ASC.
  The variable store uses these to implement its `list(scope)` and
  `get_by_name(name, scope)` trait methods without full-table scans.
- **Scope-isolation tests for `SqlxVariableStore`.** `(name, scope)`
  uniqueness is enforced by the schema for rows with non-`NULL` scope and by
  an application-level pre-write `get_by_name` check for global rows
  (SQLite treats `NULL` as distinct in `UNIQUE`, so the schema alone can't
  cover the `scope = None` case). Persistent round-trip across database
  reopen is covered.

### Changed
- **Sync apply is now a real reconcile (breaking change to
  `SyncApplyResponse`).** `POST /api/v1/syncs/{id}/apply` no longer returns a
  dry-run diff — it actually upserts `DeploymentSpec` manifests into the
  deployment store and, when `sync.delete_missing == true`, deletes
  deployments that are no longer present in the manifest directory.
  - The response shape is now
    `{ results: [SyncResourceResult], applied_sha?, summary }` where each
    `SyncResourceResult` is `{ resource, kind, action, status, error? }`
    with `action` in `{create, update, delete, skip}` and `status` in
    `{ok, error}`. Per-resource errors are surfaced on the individual
    result rather than aborting the whole apply.
  - `SyncState` now carries an `Arc<dyn DeploymentStorage>` alongside the
    sync store and clone root. `SyncState::new` takes the deployment store
    as a second argument; `bin/zlayer::commands::serve` wires it from the
    same persistent `SqlxStorage` used by the deployment routes.
  - `CreateSyncRequest` gains an optional `delete_missing` field so syncs
    can be created with destructive reconcile enabled up front; the default
    is still `false` (the safer behaviour).
  - On successful apply the sync's `last_applied_sha` is refreshed from
    `zlayer_git::current_sha` on the scan directory and persisted, so
    `zlayer sync ls` reflects the commit actually applied.
  - Standalone `kind: job` and `kind: cron` YAMLs are recorded as
    `action: skip` with a message explaining that job/cron records should
    be scheduled via `DeploymentSpec` (no standalone `JobStorage` /
    `CronStorage` exists on the API yet). Unknown kinds are skipped with
    `"unknown kind: {kind}"`. This replaces silent no-ops with visible
    skips so the caller sees what was reconciled and what wasn't.
  - The core apply logic is extracted as
    `handlers::syncs::apply_sync_inner(..)` (crate-private) so the
    upcoming workflow `ApplySync` action (Fix 3) can invoke the same
    reconcile path without going back out through HTTP.
  - `bin/zlayer::commands::sync_cmd::apply` now tabulates
    `results` per-row with action/status/kind/resource/detail columns and
    prints the `summary` and `applied_sha` headers. The old
    `diff.to_create / to_update / to_delete` summary is gone.
  - Six new tests in `crates/zlayer-api/src/handlers/syncs.rs` cover
    create, update, `delete_missing = false` → skip, `delete_missing = true`
    → delete, unknown kind → skip, and `kind: job` → skip-with-message,
    all driven through `apply_sync_inner` against in-memory sync +
    deployment stores.
- Existing `JsonTable` construction sites (`NOTIFIERS_TABLE`, `SYNCS_TABLE`,
  plus in-module test fixtures) now populate `unique_constraints: &[]`.
  No functional change — just the new field default. Downstream callers
  outside the workspace should do the same.
- **`SyncStorage` trait error type harmonised from `Result<_, String>` to
  `Result<_, StorageError>`.** Every method on the trait
  (`list`/`get`/`create`/`delete`/`update`) now returns the same structured
  error type used by every other storage trait in the crate, so the sync
  store is no longer the odd one out. The in-memory implementation maps its
  duplicate-name and missing-id cases to `StorageError::AlreadyExists` and
  `StorageError::NotFound` respectively — and the new persistent backend
  does the same — so HTTP handlers can rely on a single conversion path.
  This is a BREAKING change at the trait level but internal to the
  workspace; the only caller (`crates/zlayer-api/src/handlers/syncs.rs`)
  was updated in the same patch to drop its `.map_err(ApiError::Internal)` /
  `.map_err(ApiError::Conflict)` noise in favour of the new
  `From<StorageError> for ApiError` conversion, which preserves HTTP status
  semantics (`NotFound` → 404, `AlreadyExists` → 409).
- The `'static` bound on `SyncStorage` was dropped to match the other
  storage traits — it was defensive, not actually required by any caller.

### Added
- **Persistent `SQLite` backend for sync storage (`SqlxSyncStore`).**
  Sync records (git-backed resource-set pointers) now survive daemon
  restarts. The new store lives in `crates/zlayer-api/src/storage/syncs.rs`
  alongside the existing `InMemorySyncStore` (kept for unit tests); both
  back the same `SyncStorage` trait so callers are backend-agnostic.
  Re-exported from `storage::mod` and `zlayer_api::storage` as
  `SqlxSyncStore`.
  - Schema: `name` is a single-column `UNIQUE` peeled column (global name
    uniqueness enforced at the schema level); `project_id` is a nullable
    secondary index for fast project-scoped lookups.
  - Preflight name check on `create` yields a clean `StorageError::
    AlreadyExists` even before the adapter's upsert machinery runs; the
    schema-level UNIQUE is the backstop.
- **`StoredSync.delete_missing: bool` field**, defaulted to `false` via
  `#[serde(default)]` so existing serialised blobs without the field
  deserialise as "skip deletes" (the safer default). This is the
  per-record toggle that the upcoming real sync-apply implementation (Fix
  5) reads to decide whether resources present on the API but missing
  from the manifest should be deleted. Fresh syncs default to `false`;
  `StoredSync::new(...)` was updated to set the field explicitly.
- **`From<StorageError> for ApiError`** in `crates/zlayer-api/src/error.rs`.
  Maps `NotFound` → `ApiError::NotFound`, `AlreadyExists` → `Conflict`,
  `Serialization`/`Other`/`Database`/`Io` → `Internal` with descriptive
  prefixes. Callers can now just `.await?` on any `StorageError`-returning
  store method and get the correct HTTP status.
- **Persistent `SQLite` backend for permission storage (`SqlxPermissionStore`).**
  Resource-level permission grants now survive daemon restarts. The new store
  lives in `crates/zlayer-api/src/storage/permissions.rs` alongside the
  existing `InMemoryPermissionStore` (kept for unit tests); both back the same
  `PermissionStorage` trait so callers are backend-agnostic. Re-exported from
  `storage::mod` as `SqlxPermissionStore`.
  - Hand-rolled schema (NOT built on `SqlxJsonStore`) because `check()`
    requires SQL-side wildcard matching (`resource_id IS NULL`) and a
    `MAX(level)` aggregate across matching rows — neither is expressible
    through the generic JSON-table adapter. All five query columns
    (`subject_kind`, `subject_id`, `resource_kind`, `resource_id`, `level`)
    are first-class indexed SQLite columns. The full record is additionally
    serialized into `data_json` so `StoredPermission` can grow new fields
    without schema migrations.
  - Three covering indexes: `idx_perm_subject (subject_kind, subject_id)`
    for `list_for_subject`, `idx_perm_resource (resource_kind, resource_id)`
    for `list_for_resource`, and `idx_perm_check (subject_kind, subject_id,
    resource_kind, resource_id)` so the hot-path `check()` aggregate runs as
    an index scan instead of a table scan.
  - `grant()` is an upsert on the `id` primary key (same as
    `InMemoryPermissionStore` which keys by `id` in its `HashMap`) — two
    distinct ids with the same (subject, resource) coexist, and `check()`
    resolves them via `MAX(level)`. Deliberately no unique constraint on
    (subject, resource); re-granting with the same id replaces every field
    on the existing row.
  - `check()` is a single `SELECT MAX(level) WHERE subject_kind=? AND
    subject_id=? AND resource_kind=? AND (resource_id IS NULL OR
    resource_id=?)` that folds wildcard and exact-id grants in one round
    trip. `SubjectKind` serializes as `"user"`/`"group"` TEXT (matching the
    `Display` impl and `serde` wire form); `PermissionLevel` is stored as a
    small INTEGER (None=0, Read=1, Execute=2, Write=3) so comparisons work
    at the SQL layer.

### Fixed
- Cleaned up two `clippy::doc_markdown` hits and one `clippy::single_match_else`
  in `storage/sqlx_json.rs` introduced by the variable-store patch — the
  workspace now passes `cargo clippy --workspace --all-targets --
  -D warnings` again.
- **`zlayer serve` wires every persistent API store through a single
  `StorageBundle`** (`bin/zlayer/src/commands/serve.rs`). The bundle opens
  11 `SQLite` databases under `--data-dir` — `users.db`, `environments.db`,
  `projects.db`, `variables.db`, `tasks.db` (+ task runs), `workflows.db`
  (+ workflow runs), `notifiers.db`, `groups.db` (+ group memberships),
  `permissions.db`, `audit.db`, `syncs.db` — and feeds each handler its
  corresponding store. Prior to this change `zlayer serve` constructed
  `InMemory*Store::new()` for every resource class, so notifier configs,
  variables, tasks, workflows, groups, permissions, audit entries, and
  syncs were wiped on every daemon restart. Users, environments, and
  projects had no routes mounted at all. All of that now lives on disk
  and survives restart. The deployment database (`deployments.db`) stays
  in `init_daemon` because the S3 `SqliteReplicator` is bound to that
  exact path and the deployment-restore path depends on the concrete
  `Arc<SqlxStorage>` type — documented inline on `StorageBundle`. A
  `StorageBundle::open(None)` in-memory fallback is preserved for tests
  and exercises the same sqlx schema / query path as the file-backed
  variant (just with `:memory:`). Two new `#[tokio::test]` cases cover
  a full restart round-trip (write one record per store, drop the
  bundle, reopen at the same path, assert every record is still there)
  and the in-memory fallback. `--data-dir` defaults to `~/.zlayer` on
  macOS and `/var/lib/zlayer` (root) or `~/.zlayer` (user) on Linux.
- **`ApiConfig` grows a `user_store` handle** (`crates/zlayer-api/src/config.rs`).
  The three router builders that derive `AuthState` internally
  (`build_router_with_storage`, `build_router_with_services`,
  `build_router_with_deployment_state`) now propagate it, so
  `/api/v1/users` and `/auth/bootstrap`/`/auth/login` finally have a
  backing store when the daemon wires one in. The old behaviour
  (`user_store: None` hardcoded in the three builders) was the reason
  the Manager UI saw 500s on user management despite the store being
  implemented. `ApiConfig` drops its `#[derive(Debug)]` in favour of a
  manual impl that elides the `jwt_secret` and reports the two store
  handles as `bool`s — trait objects can't be derived-Debug, and we
  didn't want to slap a `Debug` supertrait on every storage trait.
- **Re-exports for the seven `Sqlx*Store` types that were hidden inside
  `zlayer_api::storage`** (`crates/zlayer-api/src/lib.rs`): `SqlxAuditStore`,
  `SqlxGroupStore`, `SqlxNotifierStore`, `SqlxPermissionStore`,
  `SqlxTaskStore`, `SqlxVariableStore`, `SqlxWorkflowStore`. Consumers that
  wanted to open one of these from outside the crate previously had to
  reach into `storage::...` by full path; now they're flat under
  `zlayer_api::` like the other three Sqlx stores. No API change beyond
  the re-exports.

## [0.10.98]

### Added
- **Persistent `SQLite` backend for notifier storage (`SqlxNotifierStore`).**
  Notifier configurations (Slack, Discord, webhook, SMTP) now survive daemon
  restarts. The new store lives in
  `crates/zlayer-api/src/storage/notifiers.rs` alongside the existing
  `InMemoryNotifierStore` (kept for unit tests); both back the same
  `NotifierStorage` trait so callers are backend-agnostic.
- **Generic `SqlxJsonStore<T>` adapter**
  (`crates/zlayer-api/src/storage/sqlx_json.rs`). A reusable blob-and-indexes
  persistence engine for resources with the "opaque JSON body + zero or more
  peeled-out unique/indexed columns" shape. Callers supply a `JsonTable<T>`
  describing the table name and a static list of `IndexSpec<T>` extractor
  functions; the adapter handles schema creation (`CREATE TABLE`, secondary
  indexes, table-level `UNIQUE(...)`), upserts with `created_at` preservation,
  `get`/`list`/`delete`/`count`, and `get_by_unique` lookups. This is the
  foundation for the remaining persistent stores (variables, tasks, workflows,
  groups, syncs) that will follow in subsequent patches.
- Two new `StorageError` variants: `AlreadyExists` (surfaced on `SQLite`
  2067 / 1555 constraint violations so callers can distinguish 409 from 500)
  and `Other` (generic miscellaneous error, e.g. unknown column in
  `get_by_unique`).

### Changed
- `NotifierStorage` implementations are now re-exported from
  `storage::mod` as `{InMemoryNotifierStore, NotifierStorage,
  SqlxNotifierStore}` (previously only the in-memory variant was exposed).
  No call-site changes required; the new type is additive.

## [0.10.97]

### Added
- **SMTP notifier sending is now fully implemented.** The
  `POST /api/v1/notifiers/{id}/test` endpoint no longer returns
  `501 Not Implemented` for `NotifierConfig::Smtp`; it connects to the
  configured SMTP server, authenticates, and delivers a real email via
  [`lettre`](https://docs.rs/lettre/0.11/). Upstream failures (bad
  credentials, unreachable host, invalid `from`/`to` addresses) are
  surfaced as HTTP 200 with `success: false` and a descriptive message,
  matching the convention already used by the Slack/Discord/Webhook arms.
  - Internals: a new `send_smtp_message(&SmtpParams, subject, body)`
    helper in `crates/zlayer-api/src/handlers/notifiers.rs` handles
    mailbox parsing, message construction, and transport setup, so any
    future caller that needs to send email can reuse it.
  - Transport features: `AsyncSmtpTransport::<Tokio1Executor>` with
    `lettre`'s `relay()` constructor (implicit TLS on submission ports,
    or STARTTLS as auto-negotiated). No OpenSSL linkage — TLS goes
    through `rustls`, matching the rest of the workspace.
  - New workspace dependency: `lettre = "0.11"` with
    `default-features = false` and features `smtp-transport`,
    `tokio1-rustls-tls`, `builder`, `hostname`. See the inline comment
    in `Cargo.toml` for the rationale.
  - Tests: four new `#[tokio::test]` cases cover the empty-recipient,
    invalid-`from`, unreachable-host, and end-to-end-via-`send_test_notification`
    failure paths, all without requiring a live SMTP server.
  - Refactor: the Slack/Discord/Webhook arms of `send_test_notification`
    have been extracted into `send_slack_test`, `send_discord_test`, and
    `send_webhook_test` helpers. Behaviour is unchanged; the dispatcher
    is now a thin `match` over `NotifierConfig`.

## [0.10.96]

### Changed
- **`zlayer-git` now uses `gix` (gitoxide) instead of shelling out to the
  `git` CLI.** All public operations (`clone_repo`, `fetch`, `pull_ff`,
  `rev_parse`, `current_sha`, `checkout`, `ls_remote`) are now implemented
  in-process via the pure-Rust `gix = "0.81"` crate. Blocking gix work is
  dispatched through `tokio::task::spawn_blocking` so the async API is
  preserved byte-for-byte and none of the 9 callers in `zlayer-api` had to
  change.
  - HTTPS PAT authentication is installed via
    `gix::remote::Connection::with_credentials`, returning an `Outcome`
    that carries the username/token directly. Tokens never touch the
    filesystem and never appear on a command line.
  - SSH authentication writes the PEM key to a mode-`0600` `TempDir` and
    sets `GIT_SSH_COMMAND` for the duration of the operation, guarded by
    an internal `Mutex` to serialise concurrent SSH operations.
  - `ls_remote` uses a throwaway bare repo (`gix::init_bare`) as a scratch
    context for the remote listing, then inspects `ref_map.remote_refs`.
  - `checkout` and `pull_ff` rebuild the worktree by calling
    `gix::worktree::state::checkout` with the target tree's index, then
    delete worktree files that existed previously but are absent from the
    new tree so that going backwards in history no longer leaves stale
    files behind.
  - New tests `ls_remote_against_local_bare` and
    `gix_native_clone_reads_gix_init_bare` exercise the gix-native code
    paths directly.
  - Runtime dependency: `gix = "0.81"` (workspace dep) with features
    `blocking-network-client`, `blocking-http-transport-reqwest-rust-tls`,
    `credentials`, `worktree-mutation`, `revision`, `sha1`. The `tempfile`
    runtime dependency is retained in `zlayer-git` because the SSH path
    still needs an on-disk key with a stable lifetime.

## [0.10.95]

### Added
- **Groups, permissions, and audit** (Phase 8.2): resource-level access
  control and audit logging for the REST API and CLI.
  - **Groups**: user group CRUD and membership management.
    - `GroupsState` handler state backed by `Arc<dyn GroupStorage>`.
    - Seven REST endpoints:
      - `GET    /api/v1/groups` -- list (any auth)
      - `POST   /api/v1/groups` -- create (admin)
      - `GET    /api/v1/groups/{id}` -- get (any auth)
      - `PATCH  /api/v1/groups/{id}` -- update name/description (admin)
      - `DELETE /api/v1/groups/{id}` -- delete (admin)
      - `POST   /api/v1/groups/{id}/members` -- add member (admin)
      - `DELETE /api/v1/groups/{id}/members/{user_id}` -- remove member (admin)
    - `zlayer group` CLI subcommands: `list`, `create`, `delete`,
      `member add`, `member remove`.
    - Daemon client methods: `list_groups`, `create_group`, `delete_group`,
      `add_group_member`, `remove_group_member`.
  - **Permissions**: resource-level permission grant/revoke/list.
    - `PermissionsState` handler state backed by `Arc<dyn PermissionStorage>`
      and `Arc<dyn GroupStorage>` (validates group existence on grant).
    - Three REST endpoints:
      - `GET    /api/v1/permissions?user={id}|group={id}` -- list (any auth)
      - `POST   /api/v1/permissions` -- grant (admin)
      - `DELETE /api/v1/permissions/{id}` -- revoke (admin)
    - `zlayer permission` CLI subcommands: `list`, `grant`, `revoke`.
    - Daemon client methods: `list_permissions`, `grant_permission`,
      `revoke_permission`.
  - **Audit**: audit log query endpoint and recording middleware.
    - `AuditState` handler state backed by `Arc<dyn AuditStorage>`.
    - One REST endpoint:
      - `GET /api/v1/audit?user=&resource_kind=&since=&until=&limit=`
        -- list (admin)
    - `zlayer audit tail` CLI subcommand with `--user`, `--resource`,
      `--limit`, `--output` options.
    - Daemon client method: `list_audit`.
    - `audit_middleware`: Axum middleware that records an `AuditEntry` for
      every mutating request (POST/PUT/PATCH/DELETE) that succeeds (2xx).
      Extracts actor from `AuthActor` extension, derives resource kind and
      id from the URL path.
  - OpenAPI documentation for all new endpoints, schemas, and tags
    (Groups, Permissions, Audit).

## [0.10.94]

### Added
- **Notifiers resource** (Phase 7d): configurable notification channels that
  fire alerts to Slack, Discord, generic webhooks, or SMTP endpoints.
  - `StoredNotifier` type in `zlayer-api::storage`: UUID id, name, kind,
    config, enabled flag, timestamps.
  - `NotifierKind` enum: `Slack`, `Discord`, `Webhook`, `Smtp`.
  - `NotifierConfig` tagged enum: channel-specific settings (webhook URL,
    SMTP host/port/credentials/recipients, custom HTTP method/headers).
  - `NotifierStorage` trait + `InMemoryNotifierStore` implementation.
  - Six REST endpoints:
    - `GET    /api/v1/notifiers` -- list (any auth)
    - `POST   /api/v1/notifiers` -- create (admin)
    - `GET    /api/v1/notifiers/{id}` -- get (any auth)
    - `PATCH  /api/v1/notifiers/{id}` -- update (admin)
    - `DELETE /api/v1/notifiers/{id}` -- delete (admin)
    - `POST   /api/v1/notifiers/{id}/test` -- send test notification (admin)
  - Test endpoint actually fires HTTP requests to Slack/Discord/Webhook
    URLs via `reqwest`. SMTP returns 501 (deferred).
  - `zlayer notifier` CLI subcommands: `list`, `create`, `test`, `delete`
    -- dispatched to the daemon via Unix socket.
  - Daemon client notifier methods: `list_notifiers`, `create_notifier`,
    `test_notifier`, `delete_notifier`.
  - OpenAPI documentation for all notifier endpoints and schemas.

## [0.10.93]

### Added
- **Workflows resource** (Phase 7c): named DAGs of steps that compose tasks,
  project builds, deploys, and sync applies.
  - `StoredWorkflow` type in `zlayer-api::storage`: UUID id, name, ordered
    steps list, optional project_id, timestamps.
  - `WorkflowStep` struct: step name, action, optional `on_failure` handler.
  - `WorkflowAction` tagged enum: `RunTask`, `BuildProject`, `DeployProject`,
    `ApplySync`.
  - `WorkflowRun` type: recorded execution with per-step results and overall
    status.
  - `WorkflowRunStatus` enum: `Pending`, `Running`, `Completed`, `Failed`.
  - `StepResult` type: step name, status (`ok`/`failed`/`skipped`), output.
  - `WorkflowStorage` trait + `InMemoryWorkflowStore` implementation.
  - Six REST endpoints:
    - `GET    /api/v1/workflows` -- list (any auth)
    - `POST   /api/v1/workflows` -- create (admin)
    - `GET    /api/v1/workflows/{id}` -- get (any auth)
    - `DELETE /api/v1/workflows/{id}` -- delete (admin)
    - `POST   /api/v1/workflows/{id}/run` -- execute sequentially (admin)
    - `GET    /api/v1/workflows/{id}/runs` -- list past runs (any auth)
  - Sequential execution: `RunTask` steps execute tasks via `sh -c`;
    `BuildProject`, `DeployProject`, `ApplySync` log but do not execute
    real operations (v1 limitation).
  - On-failure handling: if a step fails and defines `on_failure`, the
    referenced task runs before the workflow aborts; remaining steps are
    marked as `skipped`.
  - `zlayer workflow` CLI subcommands: `list`, `create`, `run`, `logs`,
    `delete` -- dispatched to the daemon via Unix socket.
  - Daemon client workflow methods: `list_workflows`, `create_workflow`,
    `run_workflow`, `list_workflow_runs`, `delete_workflow`.
  - OpenAPI documentation for all workflow endpoints and schemas.

## [0.10.92]

### Added
- **Tasks resource** (Phase 7b): named runnable scripts that can be executed
  on demand with output streaming.
  - `StoredTask` type in `zlayer-api::storage`: UUID id, name, kind (bash),
    body, project_id (optional), timestamps.
  - `TaskKind` enum (`Bash`).
  - `TaskRun` type: recorded execution with exit code, stdout, stderr,
    start/end timestamps.
  - `TaskStorage` trait + `InMemoryTaskStore` implementation.
  - Six REST endpoints:
    - `GET    /api/v1/tasks[?project_id=...]` -- list (any auth)
    - `POST   /api/v1/tasks` -- create (admin)
    - `GET    /api/v1/tasks/{id}` -- get (any auth)
    - `DELETE /api/v1/tasks/{id}` -- delete (admin)
    - `POST   /api/v1/tasks/{id}/run` -- execute synchronously (admin)
    - `GET    /api/v1/tasks/{id}/runs` -- list past runs (any auth)
  - `zlayer task` CLI subcommands: `list`, `create`, `run`, `logs`, `delete` --
    dispatched to the daemon via Unix socket.
  - Daemon client task methods: `list_tasks`, `create_task`, `get_task`,
    `run_task`, `list_task_runs`, `delete_task`.
  - OpenAPI documentation for all task endpoints and schemas.
  - `ApiError::NotImplemented` variant for 501 responses on non-Unix
    platforms.

## [0.10.91]

### Added
- **Variables resource** (Phase 7a): plaintext shared key-value pairs for
  template substitution in deployment specs. Variables are NOT encrypted
  (unlike secrets) and are fully visible in API responses.
  - `StoredVariable` type in `zlayer-api::storage`: UUID id, name, value,
    scope (project id or global), timestamps.
  - `VariableStorage` trait + `InMemoryVariableStore` implementation.
  - Five REST endpoints:
    - `GET    /api/v1/variables?scope={project_id}` -- list (any auth)
    - `POST   /api/v1/variables` -- create (admin)
    - `GET    /api/v1/variables/{id}` -- get (any auth)
    - `PATCH  /api/v1/variables/{id}` -- update (admin)
    - `DELETE /api/v1/variables/{id}` -- delete (admin)
  - `zlayer variable` CLI subcommands: `list`, `set`, `get`, `unset` --
    dispatched to the daemon via Unix socket.
  - Daemon client variable methods: `list_variables`, `create_variable`,
    `get_variable`, `update_variable`, `delete_variable`.
  - OpenAPI documentation for all variable endpoints and schemas.

## [0.10.90]

### Added
- **GitOps sync module** (`zlayer_git::sync`): `scan_resources()` walks a
  directory for YAML files and parses each into a `SyncResource` (kind +
  name + raw content).  `compute_diff()` compares local resources against
  remote names to produce a `SyncDiff` (to_create / to_update / to_delete).
- **Sync REST API** (`handlers/syncs.rs`): five endpoints for sync CRUD +
  diff + apply:
  - `GET  /api/v1/syncs` -- list all syncs
  - `POST /api/v1/syncs` -- create a sync
  - `GET  /api/v1/syncs/{id}/diff` -- compute diff
  - `POST /api/v1/syncs/{id}/apply` -- dry-run apply (v1)
  - `DELETE /api/v1/syncs/{id}` -- delete a sync
- **In-memory sync store** (`InMemorySyncStore`): `SyncStorage` trait +
  `Arc<RwLock<HashMap>>` implementation for v1 persistence.
- **`StoredSync` type** in `zlayer-api::storage`: UUID id, name,
  project_id, git_path, auto_apply, last_applied_sha, timestamps.
- **`zlayer sync` CLI subcommands**: `ls`, `create`, `diff`, `apply`,
  `delete` -- dispatched to the daemon via Unix socket.
- **Daemon client sync methods**: `list_syncs`, `create_sync`,
  `diff_sync`, `apply_sync`, `delete_sync`.
- **OpenAPI documentation** for all sync endpoints and schemas.

## [0.10.89]

### Added
- **Per-project git poller** (`GitPoller` in `zlayer-api::poller`).
  A background task that periodically checks `git ls-remote` for new
  commits on projects with `poll_interval_secs` set.  When new commits
  are detected the local working copy is fast-forward pulled; when
  `auto_deploy` is also true, the intent to rebuild/redeploy is logged
  (actual build trigger integration comes in a later phase).
- **`auto_deploy` and `poll_interval_secs` fields on `StoredProject`**
  (`#[serde(default)]` for backward-compatible deserialization of
  existing records).  Both fields are exposed on `CreateProjectRequest`
  and `UpdateProjectRequest`.
- **`git ls-remote` helper** (`zlayer_git::ls_remote`) queries the
  remote for a branch's HEAD SHA without cloning or fetching.
- **`zlayer project auto-deploy <id> --enabled true|false`** CLI
  subcommand to toggle the auto-deploy flag via PATCH.
- **`zlayer project poll-interval <id> --seconds N`** CLI subcommand
  to set (or clear with `--seconds 0`) the polling interval via PATCH.

## [0.10.88]

### Added
- **Git push webhook receiver** (`POST /webhooks/{provider}/{project_id}`)
  -- unauthenticated endpoint that verifies HMAC signatures (GitHub,
  Gitea, Forgejo) or constant-time token comparison (GitLab) against a
  per-project webhook secret stored in `PersistentSecretsStore` under
  scope `"webhook_secrets"`. On success, triggers the same clone /
  fast-forward-pull logic as `POST /api/v1/projects/{id}/pull`. Returns
  `{ "status": "ok", "sha": "..." }`.
- **`GET /api/v1/projects/{id}/webhook`** (auth-required) returns the
  webhook URL template and HMAC secret. Generates the secret on first
  call if it does not exist.
- **`POST /api/v1/projects/{id}/webhook/rotate`** (admin-only)
  regenerates the webhook secret, invalidating the old one.
- **`WebhookState`** struct and `build_project_webhook_routes()` /
  `build_webhook_receiver_routes()` router builders in `zlayer-api`.
- **`DaemonClient::get_project_webhook()` and `rotate_project_webhook()`**
  methods for the CLI to talk to the new endpoints.
- **`zlayer project webhook show <id>` / `zlayer project webhook rotate <id>`**
  CLI subcommands.
- **`WebhookResponse` and `WebhookInfoResponse` schemas** registered in
  the OpenAPI doc under a new "Webhooks" tag.

### Changed
- `zlayer-api` now depends on `hmac` (workspace dep) for HMAC-SHA256
  signature verification.

## [0.10.87]

### Added
- **`POST /api/v1/projects/{id}/pull` endpoint** (admin-only) that wires
  the `zlayer-git` crate into the Projects handler. Clones into
  `{clone_root}/{project_id}` on first call, then fast-forward pulls on
  subsequent calls. Resolves authentication from the project's
  `git_credential_id` via `GitCredentialStore` (PAT or SSH key), falling
  back to anonymous when unset or the store is not configured. Returns
  `{ project_id, git_url, branch, sha, path }`.
- **`ProjectState::with_git(store, git_creds, clone_root)`** constructor on
  `zlayer-api`'s project state. The existing `ProjectState::new(store)`
  constructor still works and leaves `git_creds` unset / defaults
  `clone_root` to `$TMPDIR/zlayer-projects`, so callers that don't need
  pulls are unchanged.
- **`ProjectPullResponse` schema** registered in the OpenAPI doc.
- **`DaemonClient::project_pull(&id)` method** posting to the new endpoint
  and returning the parsed JSON response.
- **`zlayer project pull <id>` CLI command** that triggers a pull and
  prints the repo, branch, SHA, and working-copy path.

### Changed
- `zlayer-api` now depends on `zlayer-git` so the Projects handler can
  clone and pull directly without going through a separate service.

## [0.10.86]

### Added
- **`zlayer project` CLI command group** with full CRUD: `ls`, `create`,
  `show`, `update`, `delete`, plus deployment linking (`link-deployment`,
  `unlink-deployment`, `list-deployments`). 8 new Clap variants total.
- **`zlayer credential` CLI command group** with two nested subcommands:
  `registry` (ls, add, delete) and `git` (ls, add, delete). 6 new Clap
  variants total. Passwords/tokens are prompted interactively via
  `dialoguer` when omitted from the command line.
- **14 `DaemonClient` methods** for projects and credentials, using
  `serde_json::Value` to avoid pulling server-only handler types into the
  CLI binary.

## [0.10.85]

### Added
- **`AuthSource::SecretStore` variant** in `zlayer-core` auth resolver.
  Allows per-registry auth configs to reference a stored credential by id.
  The sync resolver returns `Anonymous` with a warning; callers needing
  the credential must use the new async resolver.
- **`resolve_registry_auth_async()` free function** in `zlayer-api::auth`.
  Resolves all `AuthSource` variants including `SecretStore`, performing
  an async lookup against a `RegistryCredentialStore<S>`.
- **`AuthResolver::source_for_registry()` and public `resolve_source()`**
  accessors on the sync resolver, enabling external async resolution
  without duplicating per-registry lookup logic.

## [0.10.84]

### Added
- **Project CRUD endpoints** at `/api/v1/projects`. List, create, fetch,
  update (PATCH), and delete projects. Mutating endpoints require `admin`.
  Deletion cascade-removes deployment links. Sub-routes for deployment
  linking: `GET/POST /{id}/deployments`, `DELETE /{id}/deployments/{name}`.
  8 endpoints total.
- **Credential management endpoints** at `/api/v1/credentials`. Registry
  credentials (list, create, delete) and git credentials (list, create,
  delete) share a combined handler. All mutation requires `admin`.
  6 endpoints total. Secrets are stored encrypted; only metadata is
  returned in responses.
- **`build_project_routes(project_state)` and
  `build_credential_routes(credential_state)` router helpers** for
  composing project and credential routes into existing routers.
- `ProjectState` and `CredentialState` re-exported from `lib.rs`.
- OpenAPI schemas for `StoredProject`, `BuildKind`, project request types,
  and credential request/response types registered in the API doc.

## [0.10.83]

### Added
- **Environment CRUD endpoints** at `/api/v1/environments`. List, create,
  fetch, rename/re-describe (PATCH), and delete deployment/runtime
  environments. Project-scoped or global. List filter: `?project={id}` for
  one project, `?project=*` for everything (currently returns globals only
  and surfaces a 503 when project rows exist that the storage trait cannot
  enumerate — see follow-up below), or no param for globals only.
  Mutating endpoints require the `admin` role; deletion refuses if the
  environment still owns secrets.
- **Environment-aware secrets routing.** `POST/GET/DELETE /api/v1/secrets`
  now accept `?environment={env_id}` to resolve the secret scope from the
  environment store (`env:{id}` for globals, `project:{pid}:env:{id}` for
  project-scoped). Body `scope` and `?scope=` continue to work for legacy
  / API-key-style clients. The two forms are mutually exclusive — sending
  both returns `400`. New helper `env_scope(&StoredEnvironment) -> String`
  centralises the scope-string convention.
- **`POST /api/v1/secrets/bulk-import?environment={id}`** parses a
  dotenv-style payload (`KEY=value` per line, with `#` comments and
  optional surrounding quotes) and writes each entry under the env's
  scope. Returns `{ created, updated, errors }`. Admin-only.
- **`GET /api/v1/secrets/{name}?reveal=true`** returns the plaintext
  value alongside metadata. Admin-only.
- **`build_environment_routes(env_state, secrets_state) -> Router<()>`**
  helper plus reworked `build_router_with_secrets` /
  `build_router_with_services_and_secrets` /
  `build_router_with_internal_and_secrets` builders that now take an
  `Arc<dyn EnvironmentStorage>` and mount `/api/v1/environments` alongside
  `/api/v1/secrets`. `SecretsState::with_environments(...)` is the new
  constructor for env-aware secrets state.
- **Extended `SecretRef` parser** in `zlayer-secrets` with environment-scoped
  references: `$S::env/name` (global environment) and
  `$S:project:env/name` (project-scoped). Legacy `$S:name` and
  `$S:name/field` forms are preserved. The leading `::` disambiguates from
  the legacy JSON-field extraction syntax (`$S:name/field`).
- **CLI `zlayer env` command group** — `zlayer env ls [--project ID]`,
  `zlayer env create <name> [--project ID]`, `zlayer env show <id>`,
  `zlayer env update <id>`, `zlayer env delete <id>`. Full environment CRUD
  from the terminal.
- **CLI `zlayer secret` extended with `--env` flag** — all existing secret
  subcommands (list/get/set/unset) now accept `--env <env-id-or-name>` to
  scope operations to an environment. The `--env` flag resolves by name when
  a non-UUID is passed. Import and export also support `--env`. All existing
  scope-based behaviour (without `--env`) is preserved.

### Changed
- Secrets handlers (`create_secret`, `list_secrets`, `get_secret_metadata`,
  `delete_secret`) now use the `AuthActor` extractor (admin-gated for
  mutations) instead of `AuthUser`. The previous implementation scoped
  every secret to the caller's user id; new code defaults to a literal
  `"default"` scope when no `environment` / `scope` is provided.
- `SecretMetadataResponse` gained an optional `value` field, populated
  only on the `?reveal=true` admin path. The field is omitted from JSON
  when unset, so existing clients see no change.

### Follow-ups
- `?reveal=true` is admin-only but does not yet require a re-authentication
  challenge within the last 60 seconds. A future iteration should add that
  guard.
- `?project=*` listing of environments cannot enumerate distinct project
  ids through the current `EnvironmentStorage` trait. The handler returns
  `503 Service Unavailable` (with the count of unenumerable rows) when
  project-scoped environments exist; once the trait grows a proper
  `list_all` accessor we will switch to true cross-project listing.

## [0.10.82]

### Added
- **User accounts with email/password login.** ZLayer now has a real user
  system instead of raw API keys. First-run setup via
  `POST /auth/bootstrap` creates the initial admin; subsequent users via
  `POST /api/v1/users` (admin-gated). Passwords are hashed with Argon2id in
  the existing encrypted `zlayer-secrets` credential store. New
  `storage/users.rs` with `UserStorage` trait, `SqlxUserStore`
  (SQLite/sqlx), and `InMemoryUserStore`.
- **Cookie + CSRF browser sessions.** `POST /auth/login` returns an HttpOnly
  `zlayer_session` JWT cookie plus a JS-readable `zlayer_csrf` double-submit
  token. A new `crates/zlayer-api/src/middleware/csrf.rs` enforces the
  double-submit on mutating cookie-authed requests; Bearer-authed API
  clients (including local CLI over the Unix-socket auto-bearer path) pass
  through untouched. Standard production flow — HttpOnly / Secure / SameSite=Lax /
  constant-time token compare via `subtle`.
- **New auth endpoints** on `zlayer-api`: `POST /auth/bootstrap`,
  `POST /auth/login`, `POST /auth/logout`, `GET /auth/me`,
  `GET /auth/csrf`. Existing `POST /auth/token` (Bearer-JWT exchange) is
  preserved for API clients and for CLI session persistence.
- **Users CRUD** at `/api/v1/users`: list, create, get, update (PATCH),
  delete, and `POST /{id}/password`. Self-service password change when the
  caller provides the current password; admins can reset for any user.
- **`AuthActor` extractor** accepts both `Authorization: Bearer …` and the
  `zlayer_session` cookie, so every endpoint works for both API clients and
  browsers. `SessionAuthUser` is also available when cookie-only is required.
- **CLI auth and user command groups.** `zlayer auth bootstrap | login |
  logout | whoami` and `zlayer user ls | create | set-role | set-password |
  delete`. Interactive password prompts via `dialoguer` when flags are
  omitted. Session persisted to `~/.zlayer/session.json` (mode 0600 on Unix)
  with atomic temp-file-plus-rename writes.
- **OpenAPI coverage.** All new endpoints and request/response schemas are
  registered in the generated spec served at `/swagger-ui`.

### Changed
- **`AuthState` gained `user_store: Option<Arc<dyn UserStorage>>` and
  `cookie_secure: bool` fields.** All construction sites were updated to
  default both (`None` and `false`). Production deployments should set
  `cookie_secure = true` behind TLS.
- **`Claims` now carries an optional `email` field** (serde `default` for
  back-compat with tokens minted before this release). `create_token_with_email`
  is the new canonical minting function; the existing `create_token` forwards
  to it with `email = None` so callers don't need to change.
- **CLI `zlayer auth` command surface replaced.** The previous remote-server
  API-key flow (`zlayer auth login <URL> --api-key … --api-secret …`) is
  gone. Email/password is the new primary flow against the local daemon;
  remote-server + `--url` support can return as a follow-up.

### Security
- Argon2id hashing for user passwords (reused from the existing encrypted
  credential store; no downgrade).
- HttpOnly session cookies; CSRF double-submit with constant-time compare
  (`subtle`); short-circuit on Bearer so API clients aren't CSRF-gated.
- Session file mode 0600 on Unix; atomic write via temp file + rename.

## [0.10.81]

### Fixed
- **`ZImagefile`/Dockerfile `WORKDIR` now materialises the directory in the
  built image's rootfs**, matching Docker's WORKDIR semantics. Previously,
  the builder only ran `buildah config --workingdir` (metadata-only), so
  images built from a `ZImagefile` declaring e.g. `workdir: /workspace`
  shipped without `/workspace` in the rootfs. Containers started with
  `cwd=/workspace` then failed at init time with `chdir(/workspace): ENOENT`,
  surfaced as the opaque upstream message "failed to unix syscall" from
  youki's init process. `crates/zlayer-builder/src/buildah/mod.rs` now emits
  a `buildah run -- mkdir -p <dir>` alongside the `--workingdir` config.
- **`zlayer build` now hard-errors with actionable remediation** when
  `/var/lib/zlayer/{cache,registry}` isn't writable by the current UID,
  instead of silently dropping the local registry and producing an image the
  daemon can't find (which then falls through to `docker.io/library/*` with a
  cryptic 401).

### Changed
- **`zlayer daemon install` chowns the build-facing data dirs**
  (`registry`, `cache`, `bundles`) to `$SUDO_USER:zlayer` (mode 2775) instead
  of only `chgrp zlayer`. The installing user can run `zlayer build`
  immediately in their current shell — no `newgrp zlayer` or re-login
  required. Group membership is still provisioned so additional users added
  later can share the same dirs (after their own re-login).
- **`install.sh` and `install.ps1` warn when a stale `zlayer` binary earlier
  on `PATH` shadows the one just installed**, which otherwise produces
  mystifying "image built but daemon can't find it" symptoms (the stale
  binary skips the local-registry import step that the current binary
  performs).

## [0.10.77]

### Changed
- **`--deployment` is now optional on `zlayer logs`, `zlayer stop`, `zlayer exec`,
  `zlayer job trigger`, `zlayer job status`, and `zlayer cron status`.**
  A new shared resolver at `bin/zlayer/src/commands/resolver.rs` auto-picks the
  deployment when the service (or deployment) name is unambiguous. When multiple
  candidates match and stdout is a TTY, an interactive `dialoguer::Select` prompt
  disambiguates; when non-TTY, the command errors with the list of candidates and
  a hint to pass `--deployment` explicitly. Example: `zlayer logs my-api` now
  works directly whenever `my-api` appears in exactly one deployment.
- **Removed duplicated deployment-detection in `commands/exec.rs`.** The old
  `auto_detect_deployment` helper is gone; `exec` now uses the shared resolver
  (which additionally validates the service name and supports the TTY prompt path,
  neither of which `exec` did before).
- **`CLAUDE.md` aligned with the current tree.** Removed stale references to
  `bin/runtime/` and `bin/zlayer-build/` (both consolidated into `bin/zlayer` long
  ago), documented `bin/zlayer-desktop`, and corrected the `cargo run` examples.
  Also updated the CHANGELOG guidance to use real version numbers (never
  `[Unreleased]`).

## [0.10.76]

### Added
- **`zlayer` system group provisioned during `daemon install` (Linux).**
  `zlayer daemon install` now creates a `zlayer` system group (via `groupadd
  --system`), adds `$SUDO_USER` to it (via `usermod -aG`), and chgrps +
  chmods the shared build-facing data directories — `registry`, `cache`, and
  `bundles` — so unprivileged users can run `zlayer build` against the
  system-wide data dir without sudo. Secrets, raft, and live container
  runtime state (containers/rootfs/volumes) stay root-only. Applied before
  the `systemctl daemon-reload` call so group setup still takes effect on
  WSL distros with systemd disabled. Membership in `zlayer` is
  root-equivalent on the host (same trust model as the `docker` group). No
  change on macOS (single-user data dir) or Windows.

### Fixed
- **`zlayer-manager` dashboard failed with `error sending request for url
  (http://0.0.0.0:3669/...)`.** Two compounding issues. First, `zlayer serve`
  was baking the bind address directly into the `ZLAYER_API_URL` env var it
  injects into every container, producing `http://0.0.0.0:3669` when the
  default wildcard bind was used — wildcard IPs can't be dialed.
  `bin/zlayer/src/commands/serve.rs` now substitutes `127.0.0.1` / `[::1]`
  for wildcard binds when constructing the per-container URL. Second, even
  with a valid TCP URL the manager can't reach the host's loopback from
  inside an overlay-networked container. `crates/zlayer-manager/src/api_client.rs`
  now supports HTTP-over-Unix-Domain-Socket via `ZLayerClient::new_unix`,
  backed by a `hyper_util::client::legacy::Client` + `tower::Service<Uri>`
  UnixConnector (pattern copied from `bin/zlayer/src/daemon_client.rs`).
  `get_api_client()` in `server_fns.rs` now prefers `ZLAYER_SOCKET` when
  set + file exists, falling back to `ZLAYER_API_URL` TCP. Works regardless
  of container network mode. All ~40 existing public API methods were
  refactored to route through a transport-aware `execute()` helper; the
  reqwest-backed TCP path is preserved for external/remote manager
  deployments.
- **CI "Install system dependencies" step hung indefinitely.** `sudo apt-get
  install -y` was blocking on `needrestart`'s whiptail prompt on Ubuntu
  22.04+ runners despite `-y`. All four Linux install steps in
  `.forgejo/workflows/ci.yaml` now run with `DEBIAN_FRONTEND=noninteractive`,
  `NEEDRESTART_MODE=a`, `NEEDRESTART_SUSPEND=1`, `sudo -E`, and
  `Dpkg::Options::--force-confnew` so no prompt can stall the job.
- **`zlayer build` silently skipped local-registry import on permission
  errors.** When the builder's local-registry import step hit EACCES —
  typically because an unprivileged user ran `zlayer build` against the
  root-owned `/var/lib/zlayer/registry` — it logged a warning and returned
  success. The daemon then couldn't find the just-built image locally and
  fell through to Docker Hub, producing confusing 401s for images that were
  never supposed to leave the box. The import step now returns a hard error
  that names the registry path and the underlying cause.
- **`zlayer-manager` image build failed copying `hash.txt`.** The 0.10.75
  fix copied from `target/hash.txt`, but cargo-leptos 0.3.x writes `hash.txt`
  next to the compiled binary (`target/release/hash.txt`). Both
  `ZImagefile.zlayer-manager` and `Dockerfile.zlayer-manager` now copy from
  the correct path, verified locally by running `cargo leptos build
  --release` and inspecting the output.
- **`cargo install cargo-leptos` in zlayer-manager/zlayer-web images broke
  when `core2 0.4.0` was yanked.** Fresh semver resolution picked the
  yanked version transitively via `libflate`. Both Dockerfiles and
  ZImagefiles now use `cargo install cargo-leptos --locked`, which reuses
  the lockfile shipped with cargo-leptos and avoids the yank entirely.

## [0.10.75]

### Added
- **Interactive deploy TUI.** `zlayer deploy` and `zlayer up` now render the
  existing ratatui-based deploy TUI by default on a TTY. The TUI animates the
  deployment plan, infra status panel, per-service lifecycle (Registering →
  Scaling → Running or Failed), and a scrollable log pane. SSE events from
  the daemon are translated into structured `DeployEvent`s so the panels
  populate in real time. Ctrl+C inside the TUI detaches cleanly (the daemon
  keeps running). `--no-tui` (already a global flag) falls back to the plain
  logger; dry-run and non-TTY stdout (CI/piped) also stay plain.

### Fixed
- **zlayer-manager container panic at startup.** `cargo-leptos` emits
  `hash.txt` to the workspace target directory when `hash-files = true`, but
  the runtime images didn't copy it, so Leptos 0.8.17 panicked at startup
  with `failed to read hash file` before serving the first request. Both
  `ZImagefile.zlayer-manager` and `Dockerfile.zlayer-manager` now copy
  `hash.txt` into `/app/hash.txt` alongside the binary.

## [0.10.74]

### Added
- **`zlayer volume` CLI commands.** New subcommand group for volume
  management: `ls` (list volumes with name, path, and size) and `rm`
  (remove a volume, with `--force` for non-empty volumes). Includes new
  REST API endpoints (`GET /api/v1/volumes`, `DELETE /api/v1/volumes/{name}`)
  backed by filesystem-level volume directory enumeration, plus
  `DaemonClient` methods and route registration in the daemon server.

## [0.10.73]

### Added
- **`zlayer network` CLI commands.** New subcommand group for network
  management: `ls`, `inspect`, `create`, `rm` for network CRUD, plus
  `status`, `peers`, and `dns` for overlay network introspection. All
  commands communicate with the daemon via the existing REST API endpoints.

### Fixed
- **zlayer-manager white screen in production.** Container images were missing
  `LEPTOS_OUTPUT_NAME`, `LEPTOS_SITE_PKG_DIR`, and `LEPTOS_HASH_FILES` runtime
  env vars, so Leptos SSR generated unhashed script paths that didn't match the
  hashed files built by cargo-leptos. Also added `assets-dir` to `Leptos.toml`
  so the logo and other static assets are copied into the site directory.
- **zlayer-web same missing env vars.** Applied the same runtime env var fix to
  the zlayer-web container images.

### Changed
- **zlayer-manager default port 9120 → 6677.** Port 9120 conflicted with
  Komodo. Updated CLI default, Leptos config, container images, deployment
  spec generator, and documentation.

## [0.10.72]

### Fixed
- **`zlayer status` reports "not running" when daemon runs as root via
  systemd.** The Unix socket at `/var/run/zlayer.sock` was created with
  mode `0o660` (owner+group only). When the daemon runs as root, the
  socket is owned by `root:root`, so non-root users cannot stat it and
  `zlayer status` thinks nothing is there. Changed to `0o666`; access
  control is already handled by the local auth token.
- **`zlayer daemon status` reports "not installed" on SELinux/Fedora
  Atomic systems.** The `detect_service_level()` function tried to stat
  `/etc/systemd/system/zlayer.service` to decide system vs user service,
  but on SELinux-confined systems non-root users cannot stat files in
  that directory. The fallback returned `User`, causing all `systemctl`
  calls to use `--user` (wrong namespace). Removed the dead
  `ServiceLevel::User` code path entirely — on Linux, zlayer is always
  installed as a system service.

## [0.10.71]

### Fixed
- **`zlayer deploy` keeps serving stale `:latest` images after a new
  release is pushed.** The end-to-end pull path had three independent
  cache bugs that all had to be fixed together. First, the
  `zlayer-registry` manifest cache was keyed by tag (`manifest:<image>`)
  in a persistent SQLite store with no TTL and no revalidation, so any
  cached mutable-tag manifest was served forever across daemon restarts.
  `pull_manifest` now revalidates mutable tags (`:latest`, `:dev`,
  `:edge`, `:main`, `:master`, empty/missing tag) via
  `HEAD /v2/{repo}/manifests/{tag}` and compares the upstream digest
  against a new `manifest-digest:<image>` sidecar entry; on mismatch the
  stale cache is invalidated before the refetch. Registry-unreachable
  errors fall back to the cached copy with a warning so offline deploys
  still work. Pinned tags and digest refs keep their fast-path behavior.
  Second, `YoukiRuntime::pull_image` hard-coded `PullPolicy::IfNotPresent`,
  dropping any `pull_policy: always` the user set in their spec before it
  reached the puller; `pull_image_with_policy` and `pull_image_layers` now
  thread the spec's policy all the way through to the registry client via
  the new `ImagePuller::pull_image_with_policy(..., force_refresh)` entry
  point, which invalidates the manifest cache when `force_refresh` is
  true. Third, `DockerRuntime::pull_image_with_policy` trusted
  `docker inspect_image` alone to short-circuit `IfNotPresent` pulls,
  never consulting the registry; it now compares the local image's
  `repo_digests` entry against the upstream digest returned by
  bollard's `inspect_registry_image` (the Docker distribution API) and
  re-pulls on mismatch.
- **`zlayer build` / `zlayer-build` reuse stale base images.**
  `buildah from` was invoked with no `--pull` flag, so any upstream
  republish of the base image was invisible to subsequent local builds
  until `buildah rmi` was run manually. Builds now default to
  `--pull=newer`, matching modern build-tool conventions: fast when
  nothing has changed, correct when the registry has a newer copy.
  Controlled via the new `--pull=<newer|always|never>` and `--no-pull`
  flags on `zlayer build`.

### Added
- **`zlayer image ls`, `zlayer image rm <image>`, `zlayer system prune`
  subcommands.** First-class image management from the CLI via new
  `/api/v1/images`, `/api/v1/images/{image}`, and `/api/v1/system/prune`
  REST endpoints on the daemon, backed by new
  `Runtime::list_images`, `Runtime::remove_image`, and
  `Runtime::prune_images` trait methods. Implemented for the Docker
  runtime (via bollard's image-management APIs) and the Youki runtime
  (walks the persistent `zlayer-registry` cache to remove manifest
  entries and their referenced layer blobs, and prunes orphaned
  content-addressed blobs). Other runtimes inherit a default
  "unsupported" error. Users can now force a fresh pull of a stale
  `:latest` image with `zlayer image rm <image>` + redeploy instead of
  manually wiping `{data_dir}/cache/blobs.redb`.

## [0.10.70]

### Fixed
- **Deploy "Stabilization timed out" no longer masks the real container
  failure.** When a container's init process crashed during startup (bad
  image, missing libs, failed mount), the overlay attach code would run
  against the now-dead PID and emit misleading `RTNETLINK answers: No such
  process` / `File exists` / `Invalid "netns" value` errors. The user then
  saw a generic `Stabilization timed out: N/N replicas, healthy=false`
  with no pointer to the root cause. The agent now (a) checks the
  container's state between `start_container` and overlay attach, (b)
  returns the container's log tail when the init already exited, and
  (c) includes each failing service's recent log lines in the
  stabilization timeout error itself.
- **Veth leak from failed overlay attaches.** Each failed
  `attach_to_interface` used to leave a `veth-<pid>` pair (one end named
  `eth0`) orphaned in the host network namespace. The next deploy's
  `ip link add ... peer name eth0` then failed with RTNETLINK "File
  exists", cascading across redeploys. The overlay manager now (a) creates
  the container-side veth with a unique name (`vc-<pid>`) and renames it
  to `eth0` only after moving into the container's netns, (b) deletes
  the pair on any attach-path failure, and (c) sweeps orphan veth pairs
  whose owning PID is no longer alive before each new attach.
- **`zlayer-manager` and `zlayer-web` images fail at runtime with
  `version 'GLIBC_2.38' not found`.** The cargo-chef builder
  (`lukemathwalker/cargo-chef:latest-rust-1.90`) runs on Debian trixie
  (glibc 2.41), but the runtime stage was pinned to `debian:bookworm-slim`
  (glibc 2.36). Binaries referencing GLIBC_2.38+ symbols would not start.
  Both Dockerfile and ZImagefile runtime stages now use `debian:trixie-slim`
  to match the builder glibc.

## [0.10.69]

### Fixed
- **`zlayer daemon status` and `zlayer status` report "not installed" / "not running"
  after installing with sudo.** The daemon commands used `geteuid()` to decide
  between system-level and user-level systemd paths. Since `install.sh` runs
  `sudo zlayer daemon install` (system-level), but users query without sudo
  (user-level), the status commands looked in the wrong place. Now probes the
  filesystem for the actual unit file location and data directory, falling back
  to euid only during fresh installs.

## [0.10.66]

### Fixed
- **Container creation "failed to prepare rootfs" on Linux.** libcontainer 0.5.7
  used `is_file()` to decide bind mount destination type, which returned false
  for Unix sockets — causing the ZLayer API socket mount to create a directory
  instead of a file, failing with EINVAL. Upgraded to libcontainer git rev
  `a68a38c4` (youki-dev/youki#3484) which uses `!is_dir()` instead.
- **Improved container creation error reporting.** libcontainer errors now use
  Debug formatting to preserve the full error chain, so mount failures show
  the actual syscall error (e.g., EINVAL, EPERM) instead of just
  "failed to prepare rootfs".

### Changed
- **Socket bind mount uses `typ("bind")`** instead of `typ("none")` in the OCI
  spec, ensuring libcontainer's bind mount code path handles the source
  correctly.
- **Set `rootfsPropagation` to `"private"`** in OCI spec (matches Docker default).

### Added
- **`scripts/install-dev.sh`** — builds from source and installs like `install.sh`
  but reports `0.0.0-dev`. Run `install.sh` to go back to a release build.

## [0.10.65]

### Fixed
- **SELinux 203/EXEC on Fedora/RHEL.** Binaries installed under
  `/var/lib/zlayer/bin` inherited `var_lib_t`, which systemd's `init_t` cannot
  exec as a service entrypoint, so `systemctl start zlayer` failed with
  `status=203/EXEC`. `install.sh` now relabels the binary to `bin_t` via
  `semanage fcontext` + `restorecon` (persistent) with `chcon` as a fallback
  for systems without `policycoreutils-python-utils` (e.g., Fedora Silverblue).
- **Daemon startup failure now shows current error, not stale logs.** Daemon log
  files are truncated before each start so failure diagnostics only contain
  output from the current attempt. Switched from `RotationStrategy::Never` to
  `Daily` rotation with 7-day retention. Added `--since=-2min` to the
  journalctl fallback query on Linux. Previously, `zlayer daemon install/start`
  could print week-old tracing entries from previous attempts.
- **Systemd unit upgraded to `Type=notify`** with `sd_notify(READY=1)` via the
  pure-Rust `sd-notify` crate, matching Docker/containerd/CRI-O. `systemctl
  start zlayer` now blocks until the daemon is truly ready (all init phases
  complete, API socket bound) instead of returning immediately.
- **Fixed daemon.json / socket bind race condition.** `daemon.json` was written
  ~300 lines before the API socket bound, so the CLI could think the daemon was
  ready while the socket wasn't listening yet.

### Changed
- **Install script now auto-configures PATH.** When the install directory is not
  already on PATH, the user is prompted (Y/n) and the script writes
  `/etc/profile.d/zlayer.sh` (Linux) or `/etc/paths.d/zlayer` (macOS).
- **Install script reports installed vs target version.** Shows both versions
  before downloading and prompts before reinstalling the same version.

### Added
- **Networks management page** in ZLayer Manager UI at `/networks`: full CRUD
  interface for network access-control policies. Displays stats row (total
  networks, members, rules, active policies), networks table with View/Delete
  actions, detail modal showing CIDRs as badges, members table with kind badges,
  and access rules table with allow/deny badges. Create modal accepts name,
  description, and comma/newline-separated CIDRs. Delete confirmation modal
  included. API client methods (`list_networks`, `get_network`,
  `create_network`, `delete_network`) and Leptos server functions added. Sidebar
  updated with "Networks" link in the Network section.
- **Network policy enforcement in reverse proxy**: the proxy now evaluates
  `NetworkPolicySpec` access rules against incoming requests. When a source IP
  belongs to a network with defined policies, access is allowed or denied based
  on service/deployment/port rules (deny takes priority, default deny when
  governed). The `NetworkPolicyChecker` is shared between the API layer and
  the proxy so that policy changes via the Networks API take effect immediately.
- **Networks API** (`/api/v1/networks`): CRUD endpoints for network
  access-control groups. Supports creating, listing, getting, updating, and
  deleting `NetworkPolicySpec` objects that define membership (users, groups,
  nodes, CIDRs) and service access rules. Includes `ip_matches_network()`
  helper for CIDR-based access matching.
- **`ServiceNetworkSpec` rename**: the per-service overlay/join-policy config
  formerly named `NetworkSpec` is now `ServiceNetworkSpec` to avoid collision
  with the new standalone `NetworkPolicySpec` type.
- **Reverse Proxy management page** in ZLayer Manager UI at `/proxy`: displays
  proxy stats (total routes, healthy/total backends, TLS certificates, active
  streams), routes table with expandable backend details and health badges, TLS
  certificates table with expiry warnings for certificates expiring within 7
  days, and stream proxies table with TCP/UDP protocol badges. Sidebar updated
  with "Reverse Proxy" and "SSH Tunnels" links in the Network section.
- **Proxy management API endpoints** (`GET /api/v1/proxy/{routes,backends,tls,streams}`):
  read-only REST endpoints for inspecting reverse proxy state. Includes L7
  route listing, load-balancer backend groups with health status, TLS
  certificate inventory with expiry/renewal info, and L4 TCP/UDP stream
  proxies. New getters added to `ServiceRegistry::list_routes()`,
  `LoadBalancer::{list_service_names, group_snapshot}`,
  `CertManager::list_cached_domains()`, and
  `StreamRegistry::{list_tcp_services, list_udp_services}`.
- **Proxy management API client and server functions** in ZLayer Manager:
  adds `ProxyRoute`, `ProxyBackendGroup`, `TlsCertificate`, and `StreamProxy`
  types to the API client with methods `list_proxy_routes()`,
  `list_proxy_backends()`, `list_tls_certificates()`, and
  `list_stream_proxies()`. Corresponding Leptos server functions
  (`get_proxy_routes`, `get_proxy_backends`, `get_tls_certificates`,
  `get_stream_proxies`) provide data access for the proxy management UI page.
- **Service endpoint display** in deployment detail modal: each service row now
  shows its configured endpoints (protocol, port, URL) in an expandable sub-table.
  Protocol badges use DaisyUI color coding (HTTP=info, HTTPS=success,
  WebSocket=secondary, gRPC=accent, others=ghost). Services with no endpoints
  show "No endpoints configured".
- `ServiceEndpoint` type in Manager server functions for passing endpoint data
  from the API to the UI.
- `get_services()` now fetches per-service details to include endpoint
  information alongside the service summary data.

## [2026-04-03]

### Added
- **Nodes management page** in ZLayer Manager: replaces the "Coming Soon" stub
  with a live cluster node table sourced from the Raft cluster state. Shows
  stats row (total nodes, healthy count, leader, cluster health), a table with
  node ID, address, role (leader/voter/learner badge), overlay IP, status
  (ready/draining/dead badge), CPU, and memory. Includes a "Generate Join
  Token" card when a leader is present.
- `ClusterNodeSummary` in `/api/v1/cluster/nodes` now returns enriched data:
  `advertise_addr`, `overlay_ip`, `cpu_total`, `cpu_used`, `memory_total`,
  `memory_used`, `registered_at`, `last_heartbeat`, `role`, and `mode`.
- `list_cluster_nodes()` method on `ZLayerClient` API client for calling
  `GET /api/v1/cluster/nodes`.
- Dashboard `get_system_stats()` now reports real node counts and aggregate
  CPU/memory usage from cluster state instead of hardcoded zeros.

### Changed
- Overlay API endpoints (`/api/v1/overlay/status`, `/peers`, `/ip-alloc`, `/dns`)
  now return real data from the `OverlayManager` and `DnsServer` instead of stub
  503 responses. `OverlayApiState` holds `Option<Arc<RwLock<OverlayManager>>>` and
  `Option<Arc<DnsServer>>`; the daemon's `serve` command wires them in from
  `DaemonState`. When the overlay is unavailable (host networking mode), endpoints
  return appropriate "unavailable" responses.
- Added getters on `OverlayManager`: `deployment()`, `global_interface()`,
  `overlay_port()`, `has_global_transport()`, `service_transport_count()`,
  `overlay_cidr()`, `ip_alloc_stats()`.

### Added
- Theme toggle button in Manager navbar: toggles between "dark" and "zlayer"
  (light) DaisyUI themes with localStorage persistence across page refreshes.
  Sun icon in dark mode, moon icon in light mode.
- Structured logging types: `LogEntry`, `LogStream`, `LogSource`, `LogQuery` in
  `zlayer-observability::logs` — unified type for all log sources (containers,
  jobs, builds, daemon) with timestamps and stream identification.
- `FileLogWriter` / `MemoryLogWriter` for writing structured JSONL log entries
  to disk or in-memory ring buffer.
- `FileLogReader` / `apply_query()` in `zlayer-observability::log_reader` for
  reading JSONL files with filtering (stream, source, time range, tail limit)
  and legacy `stdout.log`/`stderr.log` backward compatibility.
- `LogOutputConfig` / `LogDestination` types for configurable log destination
  (disk/memory), max size, and retention.
- `LogsConfig` in `zlayer-spec` with `logs: Option<LogsConfig>` on `ServiceSpec`
  so deployment specs can configure per-service log output.
- `max_files: Option<usize>` on `FileLoggingConfig` — old rotated log files
  beyond this limit are cleaned up at startup (default: 7).
- Daemon log rotation via `tracing-appender` with daily rotation, replacing
  the unbounded `dup2`-to-file approach.

### Changed
- Default WireGuard overlay port moved from `zlayer-overlay` to `zlayer-core`
  (`DEFAULT_WG_PORT`) for cross-platform availability (Windows thin CLI).
- `Runtime` trait: `container_logs()` returns `Vec<LogEntry>` (was `String`),
  `get_logs()` returns `Vec<LogEntry>` (was `Vec<String>`). All 4 runtimes
  (youki, docker, macOS sandbox, WASM) updated.
- `ServiceManager::get_service_logs()` returns `Vec<LogEntry>` with populated
  service/deployment fields.
- Daemon systemd unit uses `StandardOutput=journal` instead of appending to
  `/var/log/zlayer/daemon.log`. The daemon manages its own files via
  `tracing-appender`; journald captures pre-init crashes.
- `rotate_daemon_log()` renames the current day's log before every start so
  failure output is always fresh.
- `wait_for_daemon_ready()` finds the newest `daemon.*` tracing-appender file
  instead of hardcoded `daemon.log`, with journalctl fallback.
- Log rotation loop now handles `.jsonl` files alongside `.log`, and cleans up
  the `executions/` subdirectory.
- Raft bootstrap checks metrics before calling `initialize()` to suppress
  noisy openraft ERROR log on already-initialized nodes.
- `install.sh` cleanup now removes stale `zl-*` network interfaces, WireGuard
  UAPI sockets, and kills stale port-holding processes.

## [2026-04-02]

### Changed
- Default WireGuard overlay port changed from 51820 to 51420 to avoid
  conflicts with system WireGuard VPNs.
- Replaced all hardcoded `51820` port literals with `DEFAULT_WG_PORT` constant
  and `node_config.overlay_port` configuration throughout the codebase.
- Daemon startup WireGuard port cleanup: replaced passive polling with active
  process identification and termination. Reads overlay port from
  `node_config.json` instead of hardcoding, identifies the port holder via
  `ss`/`lsof`, and sends SIGTERM/SIGKILL to stale zlayer/boringtun processes.
- `OverlayManager` now accepts a configurable overlay port via
  `with_overlay_port()` builder method instead of hardcoding.
- Raft bootstrap on restart now checks metrics before calling `initialize()`,
  suppressing the noisy openraft ERROR log on already-initialized nodes.
- `install.sh`: expanded cleanup on upgrade — removes stale `zl-*` network
  interfaces, WireGuard UAPI sockets, and kills stale port-holding processes.

## [2026-04-01]

### Added
- New `zlayer-docker` crate: Docker CLI, Compose, and API socket compatibility layer.
  - `compose` module: Parse `docker-compose.yaml` files and convert to ZLayer `DeploymentSpec`.
    Handles ports (short/long syntax), volumes (bind/named/tmpfs), environment (map/list),
    depends_on (with conditions), healthchecks, deploy resources/replicas, and more.
  - `cli` module: Docker-compatible CLI subcommands (`zlayer docker run/ps/stop/build/compose/etc.`)
    with full argument parsing mirroring Docker CLI flags.
  - `socket` module: Docker Engine API v1.43 socket emulation over Unix domain socket.
    Stub endpoints for containers, images, volumes, networks, and system info.
  - 104 unit tests + 5 integration tests covering compose parsing, conversion, and round-trips.
  - Example `docker-compose.yaml` in `examples/` (nginx + API + postgres + redis stack).
  - Wired into `bin/zlayer` as `zlayer docker` subcommand (feature-gated: `docker-compat`).
- `zlayer daemon install --docker-socket`: opt-in Docker API socket at `/var/run/docker.sock`
  and Docker CLI shim at `/usr/local/bin/docker` → `zlayer docker`.
- `zlayer serve --docker-socket`: spawns Docker API socket server alongside the daemon.

## [2026-03-31]

### Fixed
- `install.sh` falling back to `~/.local/bin` which breaks `daemon install` on
  immutable distros (Fedora Atomic/Silverblue/uBlue). Now write-probes
  `/usr/local/bin` (k3s pattern), falls back to `/opt/bin`. Binary always lands
  in a system path accessible to systemd.
- `pick_system_binary_path()` last-resort fallback returning `/opt/zlayer/bin/zlayer`
  without creating the directory, causing ENOENT on `std::fs::copy`.
- Sandbox `copy_dir_recursive` failing on symlinks (common in macOS Homebrew images),
  breaking secondary tag creation (`:latest`). Symlinks are now preserved instead of
  falling through to `tokio::fs::copy`.
- Crate publishing failing for all crates: `zlayer-wsl` had a hardcoded `version = "0.0.0-dev"`
  instead of `version.workspace = true`, causing workspace resolution to fail after the
  release sed version bump.
- Sandbox backend push failure for multi-tag images ("rootfs not found"): secondary
  tags now get on-disk directories via `tag_image()` before the push phase runs.
- Sandbox backend tar archive failure on macOS ("No such file or directory"): tar
  builder now preserves symlinks instead of following them, fixing broken Homebrew
  symlinks and producing correct OCI layers.
- Build workflow `upload-temp-packages` output: `package_base_url` is now set
  unconditionally so re-runs don't pass an empty artifact URL to the release workflow.

## [2026-03-30]

### Added
- `host` field on `EndpointSpec` for host-based/subdomain proxy routing
  (e.g. `host: "api.example.com"` or `host: "*.example.com"`), wired into
  `RouteEntry::from_endpoint()` so specs can declare host patterns directly.
- GPU model affinity: `model` field on `GpuSpec` pins workloads to nodes with
  a matching GPU model (substring match, e.g. `model: "A100"`).
- GPU device index tracking: scheduler now allocates specific GPU indices per
  container, preventing device overlap when multiple containers share a node.
  `PlacementDecision` carries `gpu_indices` for downstream device injection.
- GPU environment variable injection: containers automatically receive
  `NVIDIA_VISIBLE_DEVICES`/`CUDA_VISIBLE_DEVICES` (NVIDIA),
  `ROCR_VISIBLE_DEVICES`/`HIP_VISIBLE_DEVICES` (AMD), or
  `ZE_AFFINITY_MASK` (Intel) based on vendor and allocated indices.
- Gang scheduling (`scheduling: gang` on `GpuSpec`): all-or-nothing placement
  for distributed GPU jobs — if any replica cannot be placed, all are rolled back.
- Distributed job coordination (`distributed` on `GpuSpec`): injects
  `MASTER_ADDR`, `MASTER_PORT`, `WORLD_SIZE`, `RANK`, `LOCAL_RANK`, and
  backend-specific env vars (`NCCL_SOCKET_IFNAME`/`GLOO_SOCKET_IFNAME`).
- GPU spread scheduling (`scheduling: spread`): distributes GPU workloads across
  nodes instead of bin-packing them onto the fewest nodes.
- GPU sharing modes (`sharing` on `GpuSpec`): `mps` for NVIDIA Multi-Process
  Service (up to 8 containers per GPU) and `time-slice` for round-robin sharing
  (up to 4 containers per GPU). Fractional GPU tracking in scheduler.
- `MpsDaemonManager` in `zlayer-agent` for reference-counted NVIDIA MPS daemon
  lifecycle management (auto-start on first MPS container, auto-stop on last).
- GPU utilization in heartbeat: `GpuUtilizationReport` struct with per-GPU
  utilization %, memory, temperature, and power reported via Raft heartbeat.
- GPU metrics collection module (`gpu_metrics`): portable metrics via
  `nvidia-smi` (NVIDIA), sysfs (AMD/Intel) — no hard driver dependencies.
- GPU health monitoring: detects thermal throttling, ECC errors, and
  unresponsive GPUs via `check_gpu_health()`.
- GPU metrics in `zlayer-observability`: utilization (percent), memory used/total
  (bytes), temperature (celsius), and power draw (watts). Each metric is a
  Prometheus `GaugeVec` with `gpu_index` and `node` labels.
- CDI (Container Device Interface) support: discovers specs from `/etc/cdi/`
  and `/var/run/cdi/`, resolves fully-qualified device names, merges container
  edits, and can generate NVIDIA specs via `nvidia-ctk`.
- macOS sandbox backend now supports `push_image`, `tag_image`, `manifest_create`,
  `manifest_add`, and `manifest_push` operations. Previously, pipeline builds on
  macOS succeeded but push/manifest operations always failed with "Operation 'push'
  is not supported by this backend". The sandbox backend now tars the rootfs, builds
  OCI manifests, and pushes via the registry client (requires the `cache` feature,
  which the `zlayer` CLI already enables). Multi-platform manifest lists are stored
  on disk and pushed as OCI image indexes.
- `ImagePuller::push_image_index_to_registry` method for pushing OCI image indexes
  (manifest lists) to remote registries.

## [2026-03-29]

### Fixed
- `zlayer-paths` crate now publishable (removed `publish = false`) and added to
  CI release publish order so crates depending on it can resolve it from the registry.
- `daemon install` no longer fails with "Text file busy" (ETXTBSY) when the old
  binary is still running via systemd. The install function now stops the service
  and unlinks the destination before copying.
- `admin_password` file permissions are now enforced to 0o644 on every daemon
  startup, not just on initial creation. Previously, files created with the old
  0o600 permissions stayed unreadable to non-root tools (E2E tests, CLI) forever.
- `DockerConfigAuth` now respects the `DOCKER_CONFIG` environment variable
  (standard Docker convention) for locating `config.json`.

## [2026-03-28]

### Fixed
- Daemon startup failures now produce diagnostic output instead of
  "no log output found at /var/log/zlayer/daemon.log". The systemd unit
  template now redirects stdout/stderr to the daemon log file via
  `StandardOutput=append:` and `StandardError=append:` directives. The log
  directory is pre-created before starting the service. On timeout,
  `wait_for_daemon_ready()` falls back to querying journalctl and provides
  a diagnostic hint.

## [2026-03-27]

### Fixed
- Install scripts (`install.sh`, `install.py`) now install `libseccomp` runtime
  library, verify cgroups v2, and create `/var/lib/zlayer/` state directories.
  Previously, fresh installs would fail to start containers because the bundled
  libcontainer runtime requires libseccomp at runtime but nothing installed it.
- Container, job, cron, and build API routes were missing the `AuthState`
  extension because it was applied inside `build_router_with_deployment_state()`
  before routes were nested in `serve.rs`. The `Extension(auth_state)` layer is
  now re-applied after all `.nest()` calls so every route receives auth context.
- `install.sh`: daemon install output was silently discarded (`>/dev/null 2>&1`).
  Now shows output so users can see what happened. Linux installs use `sudo`,
  macOS installs do not. Removed redundant post-install status checks that
  duplicated the daemon's own readiness reporting.
- Linux `daemon install`: `systemctl daemon-reload` and `systemctl enable` exit
  codes were silently discarded (`let _ = ...`). They now propagate errors and
  bail with the stderr message on failure.
- Non-deterministic sandbox builder cache hash. `compute_dockerfile_hash()` used
  `format!("{:?}", dockerfile)` which produced different hashes across runs due to
  `HashMap` randomized iteration order. Replaced with deterministic per-instruction
  `cache_key()` hashing. Also fixed `SandboxBackend::build_image()` not forwarding
  `options.source_hash` to `SandboxImageBuilder`, so pipeline-provided hashes were
  being silently ignored.

### Added
- Content-based cache invalidation for the sandbox builder. `SandboxImageConfig`
  now carries a `source_hash` field (SHA-256 of the Dockerfile/ZImagefile). When
  a cached image exists with a matching hash, the rebuild is skipped entirely.
  Pipeline builds (`build_single_image`) also compute a file hash up front and
  short-circuit before even creating an `ImageBuilder` when the output image is
  unchanged. `BuildOptions` and `ImageBuilder` gain a `source_hash` setter.
  `build_toolchain_as_image()` and `build_base_image()` in `macos_image_resolver`
  now embed source hashes in their cached configs.
- Multi-version builds in `scripts/build-macos-images.sh` — e.g.
  `./scripts/build-macos-images.sh node 18 20 22 24` builds Node 18, 20, 22, 24.
  Partial versions auto-resolve to latest patch via upstream APIs. Environment
  variable overrides (`GO_VERSION`, `NODE_VERSION`, `PYTHON_VERSION`, etc.) take
  precedence over API resolution. Output directories are now versioned
  (`$BUILD_DIR/golang-1.23.6/rootfs/`). OCI config.json includes version labels.
- `BuildBackend` trait in `zlayer-builder::backend` providing a pluggable
  abstraction over container build tooling. Includes `BuildahBackend` (wraps
  buildah CLI) and `SandboxBackend` (macOS Seatbelt, cfg-gated). Added
  `detect_backend()` for runtime auto-detection with `ZLAYER_BACKEND` env
  override support. Added `BuildError::NotSupported` variant for unsupported
  backend operations.
- `ImageBuilder::with_backend()` constructor for creating a builder with an
  explicit `BuildBackend`. `ImageBuilder::new()` now auto-detects the backend
  via `detect_backend()`. `with_executor()` wraps the executor in a
  `BuildahBackend` for trait-based dispatch.
- `PipelineExecutor::with_backend()` constructor for creating a pipeline
  executor with an explicit `BuildBackend`. Push, manifest, and per-image
  build operations delegate to the backend when set.

### Refactored
- Refactored package installer in `macos_image_resolver` to use a `ResolvedPackage`
  enum (`HomebrewBottle`, `DirectRelease`, `Tap`, `UvPython`) instead of
  shoehorning everything into `BrewFormulaInfo`. Renamed `fetch_formula_info()`
  to `resolve_package()`, `install_single_bottle()` to `install_package()`, and
  `fetch_and_extract_bottle()` to `install_with_deps()`. Added
  `install_direct_release()` for downloading and extracting forge release assets,
  `install_uv_python()` for Python provisioning via uv, and `DiscoveryResponse`
  for structured RepoSourceSyncer discovery. Dependency BFS walk now only recurses
  for `HomebrewBottle` variants. Added `"golang"` -> `("go", false)` to
  `map_single_package_hardcoded()`.
- Extracted the buildah build orchestration loop (stage walking, container
  creation, instruction execution, COPY --from resolution, cache tracking,
  commit, cleanup) from `ImageBuilder::build()` into
  `BuildahBackend::build_image()`. `ImageBuilder::build()` is now a thin
  coordinator: parse Dockerfile/ZImagefile/template, handle WASM early return,
  delegate to the backend. `LayerCacheTracker` moved to `backend/buildah.rs`.
  Removed inline `build_with_sandbox()`, `resolve_stages()`,
  `resolve_base_image()`, `create_container()`, `commit_container()`,
  `tag_image()`, `push_image()`, and `generate_build_id()` from `builder.rs`.
- Pipeline executor (`pipeline/executor.rs`) now always uses
  `ImageBuilder::with_backend()` instead of falling back to
  `ImageBuilder::with_executor()`, wrapping a bare executor in a
  `BuildahBackend` when no explicit backend is provided.

### Changed
- CLI `pipeline` command now uses `detect_backend()` + `PipelineExecutor::with_backend()`
  instead of directly creating a `BuildahExecutor`, enabling automatic sandbox
  fallback on macOS.
- Register container lifecycle routes (`/api/v1/containers`) in the daemon API
  server, enabling direct container creation, inspection, logs, exec, and stats
  endpoints independent of the deployment/service abstraction.

### Fixed
- macOS sandbox runtime default data directory changed from `~/.local/share/zlayer`
  to `~/.zlayer` to match the builder and other components.
- macOS images CI workflow (`macos-images.yml`): replaced `uses: ./` action
  reference (which tried to download from GitHub and hung) with building zlayer
  from source and running the pipeline directly.

### Changed
- `map_linux_packages()` in `macos_image_resolver` is now async and fetches
  package mappings from RepoSources (`zachhandley.github.io/RepoSources/maps/`)
  with a 7-day local cache at `{data_dir}/cache/package-maps/{distro}.json`.
  Resolution order: cached/fetched RepoSources map, name transformation
  heuristics (strip `-dev`, `lib` prefix, version digits), then hardcoded
  fallback. The original hardcoded mapping is preserved as `map_single_package_hardcoded()`.
- Homebrew bottle install success log now includes the formula version
  (`versions.stable`) from the Homebrew API.

- Refactored `fetch_and_extract_bottle()` in `macos_image_resolver` to resolve
  the full Homebrew dependency tree (BFS) before installing a formula. Split into
  three functions: `fetch_formula_info()`, `install_single_bottle()`, and the
  public `fetch_and_extract_bottle()` entry point. Added `dependencies` and
  `versions` fields to `BrewFormulaInfo`. Formulas already present in the rootfs
  Cellar are skipped, and individual dependency failures are logged as warnings
  without aborting the overall installation.
- Refactored `sandbox_builder::setup_base_image()` to use the 3-tier macOS image
  resolution from `macos_image_resolver`. Old inline toolchain provisioning block
  replaced with Tier 1 (local cache), Tier 2 (GHCR pull), Tier 3 (local build)
  pipeline. Non-rewritable images retain existing cache/pull logic.
- `sandbox_builder::load_base_image_config()` now checks for rewritten macOS image
  `config.json` before falling through to the existing `image_config.json` / registry
  pull path.
- Toolchain env injection in `build()` now guards against duplicating env vars that
  are already present in the config (e.g., when loaded from a cached image config).
- `sandbox_builder::execute_run()` now intercepts Linux package manager install
  commands (`apt-get install`, `apk add`, `yum install`, `dnf install`) and fetches
  Homebrew bottles into the sandbox rootfs before the translated (no-op) command runs.

### Added
- `extract_package_install_packages()` helper: parses package names from apt-get,
  apk, yum, and dnf install commands, handling flags, sudo, and `&&`-compound
  commands.
- `find_after_subcommand()` helper: locates the argument tail after a subcommand
  keyword, skipping leading flags.
- 3-tier macOS image resolution system (`macos_image_resolver`) — rewrites Docker Hub
  image references to macOS-native equivalents. Tier 1: pulls pre-built sandbox images
  from `ghcr.io/blackleafdigital/zlayer`. Tier 2: builds toolchain images locally via
  `macos_toolchain`. Tier 3: creates minimal base rootfs for distro images. Includes
  GHCR auth resolution (GHCR_TOKEN, GITHUB_TOKEN, Docker config), Homebrew bottle
  fetching for installing packages into sandbox rootfs, and Linux-to-brew package
  name mapping.
- GraalVM CE toolchain resolver for macOS sandbox builds — resolves exact versions
  (`21.0.5`), partial/major versions (`21` -> latest 21.x.y), and `latest` by scanning
  GitHub releases from `graalvm/graalvm-ce-builds`. Downloads macOS tarballs and
  extracts with `--strip-components=3` (same JDK structure as Adoptium). Sets both
  `JAVA_HOME` and `GRAALVM_HOME` environment variables. Detects base images containing
  "graalvm" in the name (e.g., `graalvm-ce`, `graalvm/graalvm-ce`).
- Swift toolchain resolver for macOS sandbox builds — provisions Swift from the host
  system's Xcode Command Line Tools rather than downloading. Locates the toolchain via
  `xcrun --find swiftc`, copies it into the cache keyed by the detected version, and
  symlinks into the build rootfs at `/usr/local/swift`. Detects `swift` base images.
- Java (Adoptium/Temurin) toolchain resolver for macOS sandbox builds — resolves `latest`
  (fetches most recent LTS from Adoptium API), exact feature versions (`21`, `17`, `8`),
  and dotted versions (`21.0.5` strips to major `21`). Uses the Adoptium binary API which
  redirects to `.tar.gz` downloads. Extracts with `--strip-components=3` to handle the
  macOS `jdk-X/Contents/Home/` tarball structure. Detects `eclipse-temurin`, `amazoncorretto`,
  and `openjdk` base images.
- Bun toolchain resolver for macOS sandbox builds — resolves exact versions (`1.2.3`),
  partial versions (`1` -> latest 1.x.y), and `latest` by scanning GitHub releases from
  `oven-sh/bun`. Downloads `bun-darwin-{arch}.zip` assets and extracts the binary into
  the `bin/` directory structure for proper PATH integration.
- Python toolchain resolver for macOS sandbox builds — uses standalone builds from
  `astral-sh/python-build-standalone` on GitHub. Resolves exact versions (`3.12.1`),
  partial versions (`3.12` -> latest 3.12.x), and `latest` by scanning GitHub release
  assets. Downloads `install_only_stripped` tarballs for the host architecture.
- Rust toolchain resolver for macOS sandbox builds — resolves exact versions (e.g. `1.82.0`),
  partial versions (`1.82` -> `1.82.0`), and `latest` (fetches stable channel TOML). Downloads
  standalone installer tarballs from `static.rust-lang.org`, extracts, and runs `install.sh`
  with `--prefix` to provision rustc + cargo into the build cache.

### Changed
- Sandbox builder: macOS toolchain provisioning now symlinks cached toolchains into the
  build rootfs instead of copying. The cache at `~/.zlayer/toolchains/{lang}-{version}-{arch}/`
  is the immutable source; each build gets a symlink to it, saving disk space and build time.

### Fixed
- Sandbox builder: macOS toolchain provisioning (`macos_toolchain.rs`) — auto-detects
  language from base image (e.g. `golang:1.23-alpine` → Go 1.23), downloads self-contained
  macOS binaries from official APIs (go.dev, nodejs.org, etc.), and provisions them directly
  into the build rootfs. No brew, no global state, parallel-build safe. Supports Go + Node
  initially, with version resolution for any published version.
- Sandbox builder: `macos_compat.rs` package manager commands (`apk add`, `apt-get install`)
  are now no-ops — toolchains are provisioned in rootfs, so Linux package installs are skipped
- Sandbox builder: PATH augmentation now always includes rootfs-prefixed paths and Homebrew
  paths, even when the base image already defines its own PATH (e.g. golang images)
- Sandbox builder: broadened Seatbelt mach-lookup to allow all services during build-time,
  fixing brew/ruby/git failures caused by restricted XPC access
- Sandbox builder: RunFailed errors now include stderr output for better diagnostics
- action.yml: added curl timeouts (`--connect-timeout 30 --max-time 120`) to prevent
  indefinite hangs when downloading ZLayer binaries from GitHub releases
- install.sh: added curl timeouts to binary download
- Sandbox builder: per-build HOME directory (`{image_dir}/home/`) instead of hardcoded
  `/root` — fixes `Read-only file system @ dir_s_mkdir - /root` errors from brew/git
- Sandbox builder: base image config (ENV, WORKDIR, USER, ENTRYPOINT, CMD, Shell,
  Healthcheck) is now loaded from OCI image metadata and merged into the build config —
  previously all base image config was silently discarded
- Sandbox builder: base image config is cached alongside rootfs as `image_config.json`
- Registry: `ImageConfig` now parses Shell and Healthcheck fields from OCI image config

### Changed
- Moved `docker/` directory to `images/` and updated all references across the codebase
  (ZPipeline.yaml, CLAUDE.md, Dockerfiles, ZImagefiles, pipeline module docs, builder README)
- Buildah "not found" warning suppressed on macOS (now debug-level); non-macOS unchanged
- Sandbox builder: broadened Seatbelt FS rules for build-time (allows RUN commands to
  write to absolute host paths since there is no chroot on macOS)

### Added
- macOS-native base images defined as ZImagefiles (`images/macos/`):
  `zlayer/base`, `zlayer/golang`, `zlayer/rust`, `zlayer/node`, `zlayer/python` (with uv),
  `zlayer/deno`, `zlayer/bun` — containing macOS Mach-O binaries for sandbox builds
- macOS base image ZPipeline (`images/macos/ZPipeline.yaml`) orchestrating all 7 images
  with dependency ordering, tagged for `ghcr.io/blackleafdigital/zlayer/`
- macOS image builder script (`scripts/build-macos-images.sh`) for bootstrapping native
  rootfs images with architecture detection (arm64/x86_64)
- Registry client: two-pass platform resolver on macOS — prefers `darwin/{arch}` for
  native ZLayer sandbox images, falls back to `linux/{arch}` for Docker Hub images
- Sandbox builder: ELF binary detection on macOS — when a RUN command fails with exit
  126/127, checks if the binary is a Linux ELF and produces a clear error message
  suggesting zlayer/ base images instead of Alpine/Debian
- Sandbox builder: PATH includes rootfs bin dirs + Homebrew paths (`/opt/homebrew/bin`)
  so that macOS-native binaries installed in the image are found
- Sandbox builder: end-to-end integration tests (`sandbox_build_e2e.rs`) covering
  manifest pull, layer pull, simple build, and ENV/WORKDIR build verification
- Sandbox builder: registry image pull via `zlayer-registry` (ImagePuller + LayerUnpacker)
  when the `cache` feature is enabled, instead of requiring pre-pulled base images
- Sandbox builder: multi-stage build support -- all stages are built sequentially and
  `COPY --from=stage` resolves files from previously-built stage rootfs directories
- Sandbox builder: ARG/ENV variable substitution (`${VAR}`, `${VAR:-default}`, `$VAR`)
  applied to RUN commands, COPY/ADD sources and destinations, ENV values, WORKDIR,
  LABEL values, and USER instructions
- Sandbox builder: ADD URL sources -- downloads via `reqwest` with automatic archive
  extraction for `.tar`, `.tar.gz`, `.tgz`, `.tar.bz2`, `.tar.xz`, and `.zip` files
- Sandbox builder: ADD local archive auto-extraction for tar/gzip/bzip2/xz/zip formats
- Sandbox builder: SHELL instruction support -- custom shell stored in config and used
  for subsequent RUN shell-form commands
- Sandbox builder: HEALTHCHECK instruction -- stores command, interval, timeout,
  start_period, and retries in `SandboxImageConfig`
- Sandbox builder: USER instruction now sets the USER environment variable for RUN
  commands and resolves usernames from the rootfs `/etc/passwd`
- Sandbox builder: COPY/ADD `--chown` and `--chmod` flags applied after file operations
  using `nix::unistd::chown` and `std::os::unix::fs::PermissionsExt`

### Fixed
- Dockerfile COPY parser: fixed source/destination extraction to use the external
  parser's separated `destination` field instead of incorrectly splitting the
  `sources` vector (which already excluded the destination)
- Sandbox builder: COPY/ADD destination path handling now correctly distinguishes
  file targets from directory targets (`.`, trailing `/`, multiple sources)

## [2026-03-17]

### Added
- macOS-native image builder (`SandboxImageBuilder`) that uses the Seatbelt sandbox
  instead of buildah, enabling `zlayer build` on macOS without requiring buildah or
  a Linux VM. Supports FROM, RUN, COPY, ADD, ENV, WORKDIR, ENTRYPOINT, CMD, EXPOSE,
  ARG, LABEL, USER, VOLUME, and STOPSIGNAL instructions (single-stage builds).
- `ImageBuilder` automatically falls back to the sandbox builder on macOS when buildah
  is not installed, instead of failing with a "buildah not found" error.
- `examples/zlayer-web.zlayer.yml` deployment spec for the Leptos web frontend

### Fixed
- Daemon now regenerates the admin password if the `admin_password` file was
  deleted while the credential store still has the (Argon2id-hashed) entry,
  preventing permanently unrecoverable admin credentials
- `zlayer daemon install` and `zlayer daemon start` now write a `spawner.pid`
  marker file before launching the daemon via launchctl/systemctl, so the new
  daemon's `cleanup_stale_daemon()` skips killing the spawning CLI process
  (previously caused SIGTERM / exit 143)
- `cleanup_stale_daemon()` now reads `{data_dir}/spawner.pid` as a fallback when
  `ZLAYER_SPAWNER_PID` env var is not set (e.g., when launched via launchd/systemd),
  preventing the daemon from killing the `zlayer daemon install/start` CLI process
- GitHub Actions E2E workflow aligned with Forgejo: added `CARGO_BUILD_JOBS: "4"`,
  `--test-threads=1` for Youki tests, and post-sudo permission fix to resolve
  SQLite "database is locked" contention and test timeouts
- `daemon install` placed `--data-dir` after the `serve` subcommand, but it's a
  top-level CLI arg — clap rejected it, causing the daemon to crash-loop on start
  (both macOS launchd and Linux systemd)

### Previously added
- Containers automatically receive `ZLAYER_API_URL`, `ZLAYER_TOKEN`, and
  `ZLAYER_SOCKET` environment variables for authenticated API access
- Unix socket bind-mounted into containers for local auth bypass (Linux/Docker)
- macOS sandbox containers get socket path in writable dirs for local access
- `zlayer token show` command to retrieve admin API credentials
- Admin password persisted to `{data_dir}/admin_password` (mode 0600) on first bootstrap

### Fixed
- Overlay networking now auto-starts reliably on daemon restart by waiting for
  the WireGuard UDP port (51820) to be freed after killing the old daemon
  (fixes `errno=48` / EADDRINUSE on restart).
- Stale network interface cleanup uses `ifconfig` on macOS instead of Linux-only
  `ip` command, properly destroying orphaned utun devices with matching WireGuard
  sockets.
- Raft data directory now uses platform-specific data dir instead of hardcoded
  `/var/lib/zlayer/raft` (fixes "Permission denied" on macOS without root).
- Raft bootstrap "already bootstrapped" message downgraded to `info!` (expected
  on every restart with persistent storage).
- Overlay warning messages no longer reference Linux-only concepts (veth pairs)
  on macOS. macOS hint now directs users to `sudo` or `zlayer daemon install`.

## [0.9.990]

### Added
- Resource-aware node join: `NodeInfo` now tracks CPU cores, memory, disk, GPU resources, and
  node status (ready/draining/dead). Join flow (`POST /api/v1/cluster/join`) includes hardware
  resource information from the joining node.
- Cross-platform system resource detection (`bin/zlayer/src/resources.rs`): detects CPU, memory,
  disk, and GPU resources using Linux `/proc/meminfo`, macOS `sysctl`, and disk via `statvfs`.
- Leader self-registration: the leader node registers itself with real hardware specs on bootstrap
  instead of placeholder values.
- Worker heartbeat endpoint (`POST /api/v1/cluster/heartbeat`): worker nodes report resource
  usage every 5 seconds. Dead-node detection loop marks stale nodes as "dead" after 30 seconds
  of missed heartbeats.
- Distributed scheduling: `Scheduler::build_node_states()` converts Raft cluster state into a
  placement-ready node list. `Scheduler::compute_placement()` feeds it through the existing
  bin-packing/dedicated/exclusive placement algorithms.
- Remote container dispatch: `Scheduler::execute_distributed_scaling()` dispatches container
  creation to remote nodes via internal API calls. `apply_scaling()` now chooses distributed
  multi-node or local-only scheduling based on cluster size.
- Service assignment tracking: service-to-node assignments are replicated through Raft via
  `UpdateServiceAssignment` entries.
- Container rescheduling on node death: when the dead-node detection loop marks a node as "dead",
  the scheduler's `handle_node_death()` method automatically reschedules affected services to
  remaining live nodes using the placement algorithm.
- `Scheduler::with_raft()` constructor: accepts a pre-existing `Arc<RaftCoordinator>` so the
  daemon can share its Raft coordinator with the scheduler.
- `PlacementState::remove_node()` and `PlacementState::remove_service_from_node()`: cleanup
  methods for clearing stale container placements when a node dies.
- Node recovery: when a previously "dead" node resumes sending heartbeats, the heartbeat handler
  automatically proposes `UpdateNodeStatus { status: "ready" }` to recover it.
- Internal authentication token is now generated during daemon init and shared between the
  scheduler and API InternalState, ensuring consistent auth across the system.
- `Runtime::get_container_port_override()` trait method: allows runtimes to report a
  dynamically assigned port for containers that share the host network. Default returns
  `None` (no override). macOS sandbox runtime returns the assigned port.
- `Container.port_override` field: stores the runtime-assigned port for proxy backend
  address construction.
- Seatbelt profiles now include the dynamically assigned port in their network bind rules.
- Stabilization and endpoint reporting (Phase 11):
  - `wait_for_stabilization()` in daemon.rs: polls ServiceManager for replica count convergence and health checks with configurable timeout
  - `ServiceHealthSummary` / `StabilizationResult` types for structured stabilization outcomes
  - `DeploymentDetails` response now includes `service_health` array with per-service replica counts, health status, and endpoint URLs
  - `create_deployment` handler orchestrates when wired: parses spec, stores as Deploying, spawns async task to register services / setup overlay / configure proxy / scale, updates to Running or Failed
  - `delete_deployment` handler tears down services (scale to 0, remove) before deleting from storage
  - `DeploymentState.with_orchestration()` constructor wires ServiceManager, OverlayManager, and ProxyManager into the handler
  - `build_router_with_deployment_state()` router builder accepts pre-built DeploymentState for orchestration
  - Enhanced CLI deploy output: success shows per-service endpoints and replica counts from daemon, failure shows per-service status with health info
  - CLI poll loop uses live `service_health` data from daemon instead of spec-based estimates
  - Serve command wires orchestration handles into DeploymentState so API deployments are fully orchestrated
- Raft distributed scheduler integration (Phase 10):
  - `RaftCoordinator` integration in daemon init with `PersistentRaftStorage` (SQLite-backed)
  - `RaftService` background Raft RPC server for cluster communication
  - `add_member()` for dynamic cluster membership changes
  - `POST /api/v1/cluster/join` and `GET /api/v1/cluster/nodes` API endpoints with `ClusterApiState`
  - Join token validation with HMAC-based authentication
  - CIDR-aware IP allocation via `IpAllocator` for overlay addresses (collision-safe)
- Secrets wiring into container environment (Phase 8):
  - `BundleBuilder.with_secrets_provider()` / `with_deployment_scope()`: thread secrets provider through OCI bundle creation
  - `$S:secret-name` env vars resolved at container start time via `resolve_env_with_secrets`
  - Falls back to `$E:` (host env) resolution when no secrets provider is configured
  - `CredentialStore`: Argon2id-based API key authentication built on `PersistentSecretsStore`
  - `CredentialStore.validate()`, `create_api_key()`, `ensure_admin()` for credential lifecycle
  - Blanket `SecretsProvider` / `SecretsStore` impls for `Arc<T>` enabling shared ownership
  - Auth handler (`/auth/token`) validates against `CredentialStore` instead of hardcoded dev credentials
  - `AuthState.credential_store` / `ApiConfig.credential_store` for injecting credential store into API
  - Admin credential bootstrapped on first daemon start (random password logged once)
  - Unix socket connections auto-injected with admin JWT via `run_dual_with_local_auth()`
  - Local CLI-to-daemon IPC requires no explicit authentication
- TCP/UDP L4 proxy wiring (Phase 6):
  - `TcpStreamService.serve()`: standalone accept loop for TCP proxying without Pingora infrastructure
  - `UdpStreamService.serve()`: accepts externally-bound `UdpSocket` for UDP proxying
  - `ProxyManager` now handles TCP/UDP protocols in `ensure_ports_for_service()`: binds listeners, spawns stream proxy tasks
  - `ProxyManager.set_stream_registry()` / `with_stream_registry()`: wires L4 stream registry
  - `StreamService` health-aware backend selection: `BackendHealth` enum (Healthy/Unhealthy/Unknown), skips unhealthy backends in round-robin
  - `StreamRegistry.spawn_health_checker()`: background TCP connect probe (every 5s, 2s timeout)
  - Daemon wires `StreamRegistry` into both `ProxyManager` and `ServiceManager`
  - Health checker task integrated into daemon lifecycle (started on init, aborted on shutdown)
- Public vs Internal endpoint enforcement (Phase 7):
  - Public endpoints (`expose: public`) bind to `0.0.0.0` (all interfaces)
  - Internal endpoints (`expose: internal`) bind to the overlay IP, or `127.0.0.1` if no overlay is available
  - Defense-in-depth: proxy rejects non-overlay sources (outside `10.200.0.0/16`) for internal routes with HTTP 403
  - `OverlayManager.node_ip()` getter exposes the node's global overlay IP
  - Removed duplicate `ExposeType` enum from `zlayer-tunnel`; now imports from `zlayer-spec`
- Container logging (Phase 5):
  - Structured log paths: container stdout/stderr written to `/var/log/zlayer/{deployment}/{service}/{container_id}.{stdout,stderr}.log`
  - `YoukiConfig.log_base_dir` / `YoukiConfig.deployment_name`: configurable log directory hierarchy
  - Bundle symlinks: `{bundle}/logs/{stdout,stderr}.log` symlink to structured paths for backward compatibility
  - Daemon auto-configures Youki runtime with log directory and deployment name on Linux
  - Structured log rotation: hourly task rotates oversized files in `/var/log/zlayer/` (keeps last 10%)
  - Old log cleanup removes `.log` files older than 7 days from the structured log directory
- GPU inventory detection module (`zlayer-agent::gpu_detector`) that scans sysfs PCI devices for GPUs
  - Auto-detects vendor (NVIDIA, AMD, Intel) via PCI vendor IDs
  - Reads VRAM from PCI BAR regions, AMD `mem_info_vram_total`, or nvidia-smi
  - Reads GPU model names from DRM subsystem or nvidia-smi
  - Resolves device paths (`/dev/nvidiaN`, `/dev/dri/cardN`, `/dev/dri/renderDN`)
- GPU fields on `NodeResources` in scheduler placement: `gpu_total`, `gpu_used`, `gpu_models`, `gpu_memory_mb`, `gpu_vendor`, and `gpu_available()` helper
- `GpuInfoSummary` struct and `gpus` field on `NodeInfo` in Raft cluster state for distributed GPU awareness
- GPU detection runs at node init and node join, with results logged and displayed
- `zlayer exec` command: batch command execution in running containers via the daemon API
- `zlayer ps` command: lists deployments, services, and containers via the daemon API
  - Supports `--deployment` filter, `--containers` flag for replica detail, `--format` (table/json/yaml)
  - Container listing API endpoint: `GET /api/v1/deployments/{name}/services/{service}/containers`
- Deploy TUI with ratatui-based interactive progress display for `zlayer deploy` and `zlayer up`
  - Phase-aware layout: deploying, running dashboard, and shutdown views
  - Infrastructure phase progress indicators (spinners, checkmarks)
  - Per-service deployment status with replica counts and health indicators
  - Scrollable log pane capturing warnings and errors
  - `--no-tui` flag for CI/non-interactive environments with clean PlainDeployLogger fallback
- `restore_deployments()`: automatic deployment recovery on daemon restart from SQLite storage
- `DaemonClient`: HTTP-over-Unix-socket client with auto-start daemon and exponential backoff retry
- `load_or_init_node_config()`: node configuration auto-initialization with WireGuard key generation on first run
- Overlay tunnel setup on node join: configures WireGuard peers, TUN interface, and persists bootstrap state for daemon reload
- Shared stabilization module: `wait_for_stabilization()` moved to `zlayer-agent` crate for reuse across daemon and API
- Deployment delete cleanup: `delete_deployment` handler tears down overlay, proxy routes, and service replicas before removing storage
- DNS handle lifecycle: `DnsHandle` kept alive in `DaemonState` for runtime record management
- Glob-based spec file discovery: `zlayer deploy` now finds `*.zlayer.yml` and `*.zlayer.yaml` files
- Pipeline file auto-discovery: accepts `ZPipeline.yaml` or `zlayer-pipeline.yaml` (no longer requires `-f` flag)
- Per-crate log filtering: default verbosity reduced to WARN for internal crates, use `-v` for old behavior

### Changed
- zlayer-consensus: replaced serde_json round-trip in state machine apply with direct
  `EntryPayload` matching (~10x faster apply). Switched to consistent bincode serialization
  for snapshots. Removed unnecessary serde_json dependency.
- Replaced kernel WireGuard with boringtun (Cloudflare's Rust userspace WireGuard implementation) for overlay networking
  - No longer requires WireGuard kernel module (`modprobe wireguard` / kernel 5.6+)
  - No longer requires `wireguard-tools` package (`wg` binary)
  - Uses TUN device via `/dev/net/tun` (universally available on Linux)
  - Still requires CAP_NET_ADMIN or root for TUN device creation
  - Renamed `WireGuardManager` to `OverlayTransport`
  - All configuration now done via UAPI protocol (no external binary dependencies)
- `zlayer manager init` now creates `manager.zlayer.yml` instead of `.zlayer.yml`
- Deploy output uses event-driven architecture (DeployEvent channel) instead of mixed println/tracing
- API server default port changed from 8080 to 3669
- Overlay network failure is now fatal when services require networking (use `--host-network` to bypass)

### Fixed
- zlayer-consensus: `handle_full_snapshot` was a no-op (snapshot data was received but never
  installed into the state machine). Now correctly deserializes and installs snapshot state.
- zlayer-consensus: `snapshot_logs_since_last` and `enable_prevote` config settings were silently
  ignored during Raft node initialization. Both are now applied to the openraft `Config`.
- Overlay container attachment: added platform guard (`#[cfg(not(target_os = "linux"))]`) to
  `OverlayManager::attach_container()` so non-Linux platforms skip veth/nsenter commands and
  return the node's overlay IP directly, eliminating noisy error logs on macOS.
- macOS sandbox networking: multiple replicas of the same service no longer conflict on ports.
  Each sandbox container is assigned a unique dynamic port (via OS port-0 allocation), passed
  to the process as `PORT` / `ZLAYER_PORT` environment variables. The proxy routes to each
  replica's unique `127.0.0.1:{assigned_port}` backend address instead of all replicas sharing
  the same port. Port guard listeners prevent TOCTOU races between `create_container()` and
  `start_container()`.
- Proxy backend registration: containers are now registered with the load balancer on startup (previously backends were never added, causing "No healthy backends" errors)
- Health check target address: TCP and HTTP health checks now connect to the container's overlay IP instead of 127.0.0.1
- Error reporting now includes service names and error details in deploy output
- Youki runtime reads image CMD, ENTRYPOINT, Env, WorkingDir, and User from OCI image config
- Docker runtime cleans up stale containers before re-deploy
- Integration tests un-ignored in `youki_e2e.rs`
