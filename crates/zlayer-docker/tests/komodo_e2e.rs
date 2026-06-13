#![cfg(unix)]
//! Komodo ↔ `ZLayer` docker-shim end-to-end test.
//!
//! Topology (all containers are host-docker siblings on the default
//! bridge, addressed by their bridge IPs):
//!
//! ```text
//!  test process ──spawns──> zlayer serve (--docker-socket)
//!       │                        │ shim socket: <sock-dir>/docker.sock
//!       │                        │
//!       │   docker (host) ── mongo ── komodo core (:9120)
//!       │                                │
//!       └──HTTP──> core API              │ Noise handshake (shared keys volume)
//!                                        ▼
//!                              komodo periphery (:8120)
//!                                DOCKER_HOST=unix:///zlayer-sock/docker.sock
//!                                (the shim socket, bind-mounted in)
//! ```
//!
//! Periphery believes the shim is dockerd: bollard's
//! `connect_with_defaults()` and the in-container docker CLI both honor
//! `DOCKER_HOST`. Every Komodo operation (server stats, container list,
//! deploy, logs) therefore exercises the shim's Docker Engine API
//! surface, including the two historical Komodo breakers: the missing
//! `POST /auth` route and stdcopy-framed logs mislabelled
//! `vnd.docker.raw-stream`.
//!
//! # Running
//!
//! Needs root (the zlayer daemon runs real containers), a working host
//! docker, and network access for the image pulls:
//!
//! ```bash
//! cargo build --package zlayer
//! sudo -E ZLAYER_BIN=target/debug/zlayer \
//!   cargo test --package zlayer-docker --test komodo_e2e -- --ignored --nocapture
//! ```
//!
//! Env knobs:
//! * `ZLAYER_BIN` — zlayer binary (default `target/debug/zlayer`).
//! * `ZLAYER_E2E_SOCK_DIR` — where the shim socket is created
//!   (default: a per-run dir under /tmp).
//! * `ZLAYER_E2E_SOCK_MOUNT_SRC` — the `docker run -v` SOURCE that
//!   exposes that socket dir to the periphery container. Defaults to
//!   the sock dir itself (host bind mount). CI sets this to a named
//!   docker volume that is also mounted into the CI job container.
//! * `ZLAYER_E2E_KOMODO_TAG` — komodo image tag (default `2.2.0`,
//!   current latest looked up from ghcr.io on 2026-06-12).
//! * `ZLAYER_E2E_MONGO_IMAGE` — mongo image (default the GHCR test
//!   mirror, see mirror-test-images.yml).
//! * `ZLAYER_E2E_KEEP` — set to skip teardown for debugging.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const CORE_PORT: u16 = 9120;
const PERIPHERY_PORT: u16 = 8120;

/// Names of every docker resource the test creates, so teardown (and a
/// stale previous run) can be swept by prefix.
const PREFIX: &str = "zlayer-komodo-e2e";

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn komodo_tag() -> String {
    env_or("ZLAYER_E2E_KOMODO_TAG", "2.2.0")
}

fn mongo_image() -> String {
    env_or(
        "ZLAYER_E2E_MONGO_IMAGE",
        "ghcr.io/blackleafdigital/zlayer-test-mongo:latest",
    )
}

/// The image the Komodo deployment runs through the shim. The GHCR
/// mirror exists so CI never depends on Docker Hub anonymous limits.
const DEPLOY_IMAGE: &str = "ghcr.io/blackleafdigital/zlayer-test-nginx:1.29-alpine";

fn zlayer_bin() -> PathBuf {
    if let Ok(p) = std::env::var("ZLAYER_BIN") {
        return PathBuf::from(p);
    }
    // Walk up from the test binary (target/debug/deps/...) to target/debug.
    let exe = std::env::current_exe().expect("current_exe");
    let debug_dir = exe
        .parent() // deps
        .and_then(|p| p.parent()) // debug
        .expect("target/debug");
    debug_dir.join("zlayer")
}

fn run(cmd: &mut Command) -> (bool, String, String) {
    let out = cmd.output().unwrap_or_else(|e| {
        panic!("failed to spawn {:?}: {e}", cmd.get_program());
    });
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Host container engine CLI for the Komodo sibling containers. Any
/// docker-CLI-compatible engine works (docker, podman via podman-docker);
/// override with `ZLAYER_E2E_ENGINE` on hosts where `docker` resolves to
/// something that isn't a real engine client (e.g. zlayer's own shim
/// wrapper).
fn docker(args: &[&str]) -> (bool, String, String) {
    let engine = env_or("ZLAYER_E2E_ENGINE", "docker");
    run(Command::new(engine).args(args))
}

fn docker_ok(args: &[&str]) -> String {
    let (ok, stdout, stderr) = docker(args);
    assert!(ok, "docker {args:?} failed:\n{stdout}\n{stderr}");
    stdout
}

/// Poll `f` until it returns Some, or panic after `timeout`.
async fn wait_for<T>(what: &str, timeout: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(
            Instant::now() < deadline,
            "timed out after {timeout:?} waiting for: {what}"
        );
        tokio::time::sleep(Duration::from_millis(750)).await;
    }
}

/// Best-effort sweep of every container/volume from this or a previous
/// run, so a crashed run doesn't wedge the next one.
fn sweep() {
    let (_, names, _) = docker(&["ps", "-aq", "--filter", &format!("name={PREFIX}")]);
    for id in names.split_whitespace() {
        let _ = docker(&["rm", "-f", id]);
    }
    let _ = docker(&["volume", "rm", "-f", &format!("{PREFIX}-keys")]);
}

/// Kills the daemon child + docker resources on drop unless
/// `ZLAYER_E2E_KEEP` is set.
struct Guard {
    daemon: Option<Child>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if std::env::var("ZLAYER_E2E_KEEP").is_ok() {
            eprintln!("ZLAYER_E2E_KEEP set: leaving daemon + containers running");
            return;
        }
        if let Some(child) = self.daemon.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        sweep();
    }
}

fn container_ip(name: &str) -> Option<String> {
    let (ok, out, _) = docker(&[
        "inspect",
        "-f",
        "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}",
        name,
    ]);
    let ip = out.trim().to_string();
    (ok && !ip.is_empty()).then_some(ip)
}

/// Minimal Komodo core API client. Komodo's API is "typed JSON over
/// four POST endpoints": /auth, /read, /write, /execute, each taking
/// `{"type": "...", "params": {...}}`.
struct Komodo {
    base: String,
    jwt: String,
    http: reqwest::Client,
}

impl Komodo {
    async fn login(base: &str, username: &str, password: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client");
        // mogh_auth_server: POST /auth/login with a typed request; the
        // response is JwtOrTwoFactor — `{"type":"Jwt","data":{"jwt":"…"}}`
        // for plain local logins.
        let resp = http
            .post(format!("{base}/auth/login"))
            .json(&serde_json::json!({
                "type": "LoginLocalUser",
                "params": { "username": username, "password": password },
            }))
            .send()
            .await
            .expect("core /auth/login reachable");
        let status = resp.status();
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        assert!(
            status.is_success(),
            "LoginLocalUser failed ({status}): {body}"
        );
        let jwt = body["data"]["jwt"].as_str().unwrap_or_default().to_string();
        assert!(!jwt.is_empty(), "no jwt in login response: {body}");
        Self {
            base: base.to_string(),
            jwt,
            http,
        }
    }

    async fn call(&self, endpoint: &str, ty: &str, params: serde_json::Value) -> serde_json::Value {
        let resp = self
            .http
            .post(format!("{}/{endpoint}", self.base))
            .header("authorization", format!("Bearer {}", self.jwt))
            .json(&serde_json::json!({ "type": ty, "params": params }))
            .send()
            .await
            .unwrap_or_else(|e| panic!("core /{endpoint} {ty}: {e}"));
        let status = resp.status();
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        assert!(
            status.is_success(),
            "core /{endpoint} {ty} failed ({status}): {body}"
        );
        body
    }
}

#[tokio::test]
#[ignore = "live e2e: needs root, host docker, and network (run with --ignored)"]
#[allow(clippy::too_many_lines)] // one linear e2e scenario; splitting hides the flow
async fn komodo_connects_through_zlayer_docker_shim() {
    let tag = komodo_tag();
    let core_image = format!("ghcr.io/moghtech/komodo-core:{tag}");
    let periphery_image = format!("ghcr.io/moghtech/komodo-periphery:{tag}");

    sweep();

    // ------------------------------------------------------------------
    // 1. zlayer daemon + docker shim socket
    // ------------------------------------------------------------------
    let run_id = std::process::id();
    let base_dir = PathBuf::from(format!("/tmp/{PREFIX}-{run_id}"));
    let sock_dir =
        std::env::var("ZLAYER_E2E_SOCK_DIR").map_or_else(|_| base_dir.join("sock"), PathBuf::from);
    let data_dir = base_dir.join("data");
    std::fs::create_dir_all(&sock_dir).expect("sock dir");
    std::fs::create_dir_all(&data_dir).expect("data dir");
    let shim_sock = sock_dir.join("docker.sock");
    let cli_sock = base_dir.join("cli.sock");

    let bin = zlayer_bin();
    assert!(
        bin.exists(),
        "zlayer binary not found at {} (build with `cargo build -p zlayer` or set ZLAYER_BIN)",
        bin.display()
    );

    let daemon_log = std::fs::File::create(base_dir.join("daemon.log")).expect("daemon log");
    #[allow(clippy::zombie_processes)] // owned by Guard, killed+reaped in Drop
    let daemon = Command::new(&bin)
        .args([
            "serve",
            "--bind",
            "127.0.0.1:36691",
            "--socket",
            cli_sock.to_str().unwrap(),
            "--deployment-name",
            "komodo-e2e",
            "--wg-port",
            "51499",
            "--dns-port",
            "15399",
            "--docker-socket",
            "--docker-socket-path",
            shim_sock.to_str().unwrap(),
        ])
        .env("ZLAYER_DATA_DIR", &data_dir)
        .stdout(Stdio::from(daemon_log.try_clone().expect("clone log")))
        .stderr(Stdio::from(daemon_log))
        .spawn()
        .expect("spawn zlayer serve");
    let guard = Guard {
        daemon: Some(daemon),
    };

    // All direct shim interactions go over curl --unix-socket: this keeps
    // the test independent of which docker-flavored CLI the host ships
    // (real docker, podman-docker, or zlayer's own `docker` shim wrapper,
    // none of which agree on `-H unix://` handling).
    let shim_curl = |method: &str, path: &str, body: Option<&str>| -> (String, String) {
        let mut cmd = Command::new("curl");
        cmd.args([
            "-s",
            "-o",
            "/dev/stderr", // body to stderr, status code to stdout
            "-w",
            "%{http_code}",
            "-X",
            method,
            "--unix-socket",
            shim_sock.to_str().unwrap(),
        ]);
        if let Some(b) = body {
            cmd.args(["-H", "Content-Type: application/json", "-d", b]);
        }
        cmd.arg(format!("http://localhost{path}"));
        let (_, code, body_out) = run(&mut cmd);
        (code.trim().to_string(), body_out)
    };

    wait_for(
        "shim socket to answer /version",
        Duration::from_secs(180),
        || {
            let (code, body) = shim_curl("GET", "/v1.47/version", None);
            (code == "200" && body.contains("\"ApiVersion\":\"1.47\"")).then_some(())
        },
    )
    .await;
    eprintln!("shim up at {}", shim_sock.display());

    // Unknown routes must be Docker-shaped JSON 404s, not bare bodies —
    // bollard surfaces a bare 404 as a JSON parse error, which is how
    // Komodo originally broke against the shim.
    {
        let (code, body) = shim_curl("GET", "/v1.47/definitely-not-a-route", None);
        assert_eq!(code, "404", "unknown route status: {body}");
        let parsed: serde_json::Value = serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("404 body not JSON ({e}): {body:?}"));
        assert_eq!(parsed["message"], "page not found", "404 body: {body}");
    }

    // POST /auth (docker login) with bogus credentials: the failure must be
    // a clean Docker-shaped credential rejection, not a 404 or parse error.
    {
        let (code, body) = shim_curl(
            "POST",
            "/v1.47/auth",
            Some(
                r#"{"username":"zlayer-e2e-bogus","password":"definitely-not-a-token","serveraddress":"ghcr.io"}"#,
            ),
        );
        assert_eq!(code, "401", "bogus /auth login should be 401: {body}");
        let parsed: serde_json::Value = serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("/auth body not JSON ({e}): {body:?}"));
        let msg = parsed["message"].as_str().unwrap_or_default();
        assert!(
            msg.contains("incorrect username or password") || msg.contains("failed"),
            "expected a credential rejection from /auth, got: {body}"
        );
    }

    // ------------------------------------------------------------------
    // 2. Komodo: mongo + core + periphery (host-docker siblings)
    // ------------------------------------------------------------------
    docker_ok(&["volume", "create", &format!("{PREFIX}-keys")]);

    docker_ok(&[
        "run",
        "-d",
        "--name",
        &format!("{PREFIX}-mongo"),
        "-e",
        "MONGO_INITDB_ROOT_USERNAME=admin",
        "-e",
        "MONGO_INITDB_ROOT_PASSWORD=admin",
        &mongo_image(),
        "--quiet",
        "--wiredTigerCacheSizeGB",
        "0.25",
    ]);
    let mongo_ip = wait_for("mongo container IP", Duration::from_secs(60), || {
        container_ip(&format!("{PREFIX}-mongo"))
    })
    .await;

    // Periphery FIRST: it generates the Noise keypair into the shared
    // keys volume (`file:` keys are created when missing); core then
    // reads periphery.pub from the same volume.
    let sock_mount_src = env_or(
        "ZLAYER_E2E_SOCK_MOUNT_SRC",
        sock_dir.to_str().expect("sock dir utf8"),
    );
    docker_ok(&[
        "run",
        "-d",
        "--name",
        &format!("{PREFIX}-periphery"),
        "-v",
        &format!("{PREFIX}-keys:/config/keys"),
        "-v",
        &format!("{sock_mount_src}:/zlayer-sock"),
        "-e",
        "DOCKER_HOST=unix:///zlayer-sock/docker.sock",
        "-e",
        "PERIPHERY_SSL_ENABLED=true",
        &periphery_image,
    ]);
    let periphery_ip = wait_for("periphery container IP", Duration::from_secs(60), || {
        container_ip(&format!("{PREFIX}-periphery"))
    })
    .await;

    // Wait for the keypair to land in the shared volume before booting
    // core (core resolves KOMODO_PERIPHERY_PUBLIC_KEY at startup).
    wait_for(
        "periphery.pub in keys volume",
        Duration::from_secs(60),
        || {
            let (ok, out, _) = docker(&[
                "exec",
                &format!("{PREFIX}-periphery"),
                "sh",
                "-c",
                "test -s /config/keys/periphery.pub && echo yes",
            ]);
            (ok && out.contains("yes")).then_some(())
        },
    )
    .await;

    docker_ok(&[
        "run",
        "-d",
        "--name",
        &format!("{PREFIX}-core"),
        "--init",
        "-v",
        &format!("{PREFIX}-keys:/config/keys"),
        "-e",
        &format!("KOMODO_DATABASE_ADDRESS={mongo_ip}:27017"),
        "-e",
        "KOMODO_DATABASE_USERNAME=admin",
        "-e",
        "KOMODO_DATABASE_PASSWORD=admin",
        "-e",
        "KOMODO_LOCAL_AUTH=true",
        "-e",
        "KOMODO_INIT_ADMIN_USERNAME=admin",
        "-e",
        "KOMODO_INIT_ADMIN_PASSWORD=zlayer-e2e",
        "-e",
        "KOMODO_PERIPHERY_PUBLIC_KEY=file:/config/keys/periphery.pub",
        "-e",
        "KOMODO_JWT_SECRET=zlayer-e2e-jwt-secret",
        "-e",
        "KOMODO_WEBHOOK_SECRET=zlayer-e2e-webhook-secret",
        "-e",
        "KOMODO_DISABLE_INIT_RESOURCES=true",
        &core_image,
    ]);
    let core_ip = wait_for("core container IP", Duration::from_secs(60), || {
        container_ip(&format!("{PREFIX}-core"))
    })
    .await;
    let core_base = format!("http://{core_ip}:{CORE_PORT}");

    let http_probe = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    {
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            if let Ok(resp) = http_probe.get(format!("{core_base}/")).send().await {
                if resp.status().as_u16() < 500 {
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "komodo core never answered at {core_base} — check `docker logs {PREFIX}-core`"
            );
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
    eprintln!("komodo core up at {core_base}");

    // ------------------------------------------------------------------
    // 3. Drive Komodo: server -> reachable, containers list, deploy, logs
    // ------------------------------------------------------------------
    let komodo = Komodo::login(&core_base, "admin", "zlayer-e2e").await;

    komodo
        .call(
            "write",
            "CreateServer",
            serde_json::json!({
                "name": "zlayer-shim",
                "config": {
                    "address": format!("https://{periphery_ip}:{PERIPHERY_PORT}"),
                    "enabled": true,
                },
            }),
        )
        .await;

    // The server must reach a healthy state: core <-> periphery handshake
    // works AND periphery can talk to its "docker" (the zlayer shim).
    {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let servers = komodo
                .call("read", "ListServers", serde_json::json!({}))
                .await;
            let state = servers
                .as_array()
                .and_then(|arr| arr.iter().find(|s| s["name"] == "zlayer-shim"))
                .and_then(|s| s["info"]["state"].as_str())
                .unwrap_or("missing")
                .to_string();
            if state.eq_ignore_ascii_case("ok") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "server never became Ok (last state: {state}) — check `docker logs {PREFIX}-periphery`"
            );
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }
    eprintln!("komodo server state: Ok (periphery is talking to the shim)");

    // Container listing goes bollard -> shim /containers/json.
    let containers = komodo
        .call(
            "read",
            "ListDockerContainers",
            serde_json::json!({ "server": "zlayer-shim" }),
        )
        .await;
    assert!(
        containers.is_array(),
        "ListDockerContainers did not return an array: {containers}"
    );

    // Deploy nginx through the shim and read its logs back via Komodo.
    komodo
        .call(
            "write",
            "CreateDeployment",
            serde_json::json!({
                "name": format!("{PREFIX}-nginx"),
                "config": {
                    "server_id": "zlayer-shim",
                    "image": { "type": "Image", "params": { "image": DEPLOY_IMAGE } },
                },
            }),
        )
        .await;
    let update = komodo
        .call(
            "execute",
            "Deploy",
            serde_json::json!({ "deployment": format!("{PREFIX}-nginx") }),
        )
        .await;
    let update_id = update["_id"]["$oid"]
        .as_str()
        .or_else(|| update["_id"].as_str())
        .unwrap_or_default()
        .to_string();
    assert!(
        !update_id.is_empty(),
        "Deploy returned no update id: {update}"
    );

    {
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            let u = komodo
                .call("read", "GetUpdate", serde_json::json!({ "id": update_id }))
                .await;
            let status = u["status"].as_str().unwrap_or("");
            if status.eq_ignore_ascii_case("complete") {
                assert!(
                    u["success"].as_bool().unwrap_or(false),
                    "Deploy completed but failed: {u}"
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Deploy never completed (status {status}): {u}"
            );
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }
    eprintln!("deploy through the shim succeeded");

    // Log fetch: periphery runs `docker logs` against the shim. The CLI
    // inspects (Config.Tty=false) and demultiplexes the stdcopy frames;
    // the returned text must be clean — a leaked 0x01 means the framing
    // or content-type regressed.
    let log = wait_for_logs(&komodo).await;
    let combined = format!(
        "{}{}",
        log["stdout"].as_str().unwrap_or(""),
        log["stderr"].as_str().unwrap_or("")
    );
    assert!(
        !combined.contains('\u{1}') && !combined.contains('\u{2}'),
        "stdcopy frame bytes leaked into Komodo's container log: {combined:?}"
    );
    assert!(
        !combined.to_lowercase().contains("failed to parse"),
        "Komodo failed to parse the shim's log stream: {combined}"
    );
    eprintln!(
        "container logs via komodo are clean ({} bytes)",
        combined.len()
    );

    // `guard` drops here: kills the daemon child and sweeps the
    // containers/volume (unless ZLAYER_E2E_KEEP is set).
    drop(guard);
}

/// Fetch the deployment's container log via Komodo, retrying until the
/// container has produced output (nginx logs its startup lines).
async fn wait_for_logs(komodo: &Komodo) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let log = komodo
            .call(
                "read",
                "GetContainerLog",
                serde_json::json!({
                    "server": "zlayer-shim",
                    "container": format!("{PREFIX}-nginx"),
                    "tail": 100,
                }),
            )
            .await;
        let len =
            log["stdout"].as_str().map_or(0, str::len) + log["stderr"].as_str().map_or(0, str::len);
        if len > 0 {
            return log;
        }
        assert!(
            Instant::now() < deadline,
            "no container log output via Komodo: {log}"
        );
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}
