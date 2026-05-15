//! API server

use std::net::SocketAddr;
use std::path::Path;
use tokio::net::TcpListener;
#[cfg(unix)]
use tokio::net::UnixListener;
use tracing::info;
#[cfg(test)]
use zlayer_paths::ZLayerDirs;

use axum::Router;

use crate::config::{ApiConfig, ApiTlsConfig};
use crate::router::build_router;

/// Persist the local-admin bearer token to disk so a local `DaemonClient` can
/// read it on connect.
///
/// - **Linux / macOS**: informational — the daemon's UDS middleware already
///   injects the bearer into UDS-originated requests. Other tooling (e.g. a
///   `zlayer` invocation that elects to skip the socket) can still use this
///   file to authenticate against the loopback TCP listener.
/// - **Windows**: load-bearing. The TCP listener has no peer-credential
///   bypass, so `DaemonClient` on Windows reads this file to authenticate.
///
/// The stored value is the raw JWT — the `Bearer ` prefix is stripped before
/// write so consumers can inject it into the `Authorization` header however
/// they like.
///
/// Failures are logged as warnings and do NOT abort server start-up. Dev
/// environments without write permission to `%ProgramData%` or `/var/lib`
/// should still be able to run the daemon.
fn persist_admin_bearer(bearer: &str) {
    let bearer_path = zlayer_paths::default_admin_bearer_path();
    if let Some(parent) = bearer_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!(
                error = %e,
                path = %parent.display(),
                "failed to create admin bearer parent dir"
            );
        }
    }
    // Strip the "Bearer " prefix; store only the raw JWT.
    let raw = bearer.strip_prefix("Bearer ").unwrap_or(bearer);
    match std::fs::write(&bearer_path, raw) {
        Ok(()) => {
            tracing::info!(
                path = %bearer_path.display(),
                "persisted admin bearer for local CLI"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Err(e) =
                    std::fs::set_permissions(&bearer_path, std::fs::Permissions::from_mode(0o600))
                {
                    tracing::warn!(
                        error = %e,
                        "failed to chmod 0o600 on admin bearer file"
                    );
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %bearer_path.display(),
                "failed to persist admin bearer"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// TLS plumbing
//
// Wave 2 of the join-token hardening wires `ApiTlsConfig` into the TCP
// listener so the verifying-key endpoint cannot be trivially MitM-ed.
// Mirrors the proxy's rustls + tokio-rustls stack (see
// `crates/zlayer-proxy/src/tls.rs` and `acme.rs`) — uses `axum-server`
// 0.8 with its `tls-rustls` feature, which is a thin tokio-rustls wrapper
// that integrates with axum's `Router` make-service. No second TLS stack.
// ---------------------------------------------------------------------------

/// Load a static cert + key pair from disk into a `RustlsConfig`.
///
/// PEM-format only. The `axum-server` helper internally calls
/// `ServerConfig::builder().with_no_client_auth().with_single_cert(...)`,
/// which requires a default `rustls::CryptoProvider`. We install one
/// idempotently before the call so unrelated test ordering can't break it.
///
/// # Errors
///
/// Returns an error if either file cannot be read or the PEM cannot be
/// parsed as a valid certificate chain + private key pair.
async fn load_static_rustls_config(
    cert_path: &Path,
    key_path: &Path,
) -> anyhow::Result<axum_server::tls_rustls::RustlsConfig> {
    install_default_crypto_provider();
    axum_server::tls_rustls::RustlsConfig::from_pem_file(cert_path, key_path)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "Failed to load TLS cert+key from {} / {}: {e}",
                cert_path.display(),
                key_path.display(),
            )
        })
}

/// Build an `axum-server` `RustlsConfig` from the proxy's `CertManager`.
///
/// Wraps `CertManager::build_server_config`, which itself wraps a fresh
/// `SniCertResolver` populated from the manager's current cert cache.
/// Hot-reload is a Wave 2.4 concern — for the join-token verifying-key
/// listener, restarting the daemon on cert renewal is acceptable since
/// renewals happen every ~60-90 days.
///
/// # Errors
///
/// Returns an error if any cached certificate fails to load into the
/// SNI resolver (malformed PEM, key/cert mismatch, etc.).
async fn load_managed_rustls_config(
    cert_manager: &zlayer_proxy::CertManager,
) -> anyhow::Result<axum_server::tls_rustls::RustlsConfig> {
    install_default_crypto_provider();
    let server_config = cert_manager
        .build_server_config()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to build TLS config from cert manager: {e}"))?;
    Ok(axum_server::tls_rustls::RustlsConfig::from_config(
        server_config,
    ))
}

/// Idempotently install a default `rustls::CryptoProvider`.
///
/// rustls 0.23 with both `aws-lc-rs` and `ring` features compiled in
/// refuses to pick a default automatically and panics if a builder is
/// called without one installed. The workspace pin enables `aws-lc-rs`
/// as the rustls default feature, so we install that. Safe to call
/// repeatedly — the second call returns `Err` which we discard.
fn install_default_crypto_provider() {
    use std::sync::Once;
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        // Discarded `Err` means a provider is already installed — fine.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

/// Serve a TCP listener with optional rustls TLS termination.
///
/// Used by every TCP-listening entry point in this module so the
/// `Option<ApiTlsConfig>` branch lives in exactly one place. UDS
/// listeners stay plaintext — they live on the local filesystem with
/// 0o660 perms, so wrapping them in TLS would buy nothing.
///
/// `shutdown` is awaited concurrently with the accept loop. For plain
/// HTTP it integrates with `axum::serve::with_graceful_shutdown`; for
/// TLS it spawns a small task that flips the `axum_server::Handle`'s
/// graceful-shutdown signal when the future resolves.
///
/// # Errors
///
/// Returns an error if loading the TLS config fails or the underlying
/// HTTP/HTTPS server returns an error during operation.
async fn serve_tcp_with_optional_tls(
    listener: TcpListener,
    router: Router,
    tls: Option<&ApiTlsConfig>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let make_service = router.into_make_service_with_connect_info::<SocketAddr>();

    match tls {
        None => {
            axum::serve(listener, make_service)
                .with_graceful_shutdown(shutdown)
                .await?;
            Ok(())
        }
        Some(tls_config) => {
            let rustls_config = match tls_config {
                ApiTlsConfig::Static {
                    cert_path,
                    key_path,
                } => load_static_rustls_config(cert_path, key_path).await?,
                ApiTlsConfig::Managed { cert_manager } => {
                    load_managed_rustls_config(cert_manager).await?
                }
            };

            // Convert tokio listener -> std listener so `axum-server` can
            // own it. `into_std()` sets non-blocking mode implicitly.
            let std_listener = listener.into_std()?;

            let handle = axum_server::Handle::new();
            let handle_for_shutdown = handle.clone();
            tokio::spawn(async move {
                shutdown.await;
                handle_for_shutdown.graceful_shutdown(None);
            });

            axum_server::from_tcp_rustls(std_listener, rustls_config)?
                .handle(handle)
                .serve(make_service)
                .await
                .map_err(anyhow::Error::from)
        }
    }
}

/// Pre-bound TCP and Unix listeners ready to be handed to [`serve_bound`].
///
/// Created by [`bind_dual_with_local_auth`] so that the Unix socket file
/// exists on disk before any container restore or other logic that needs to
/// bind-mount it.
#[cfg(unix)]
pub struct BoundListeners {
    /// Already-bound TCP listener.
    pub tcp: TcpListener,
    /// Local address the TCP listener is bound to.
    pub tcp_local_addr: SocketAddr,
    /// Already-bound Unix domain socket listener.
    pub unix: UnixListener,
    /// `Bearer <token>` value injected into Unix-socket requests that lack
    /// an `Authorization` header.
    pub local_bearer: String,
    /// Filesystem path of the Unix socket (kept for cleanup on shutdown).
    pub unix_path: std::path::PathBuf,
    /// Optional TLS configuration applied to the TCP listener at
    /// [`serve_bound`] time. `None` (the default) keeps today's plaintext
    /// HTTP behaviour. The UDS listener is always plaintext — it's gated
    /// by 0o660 perms + local-bearer injection, so TLS would buy nothing.
    /// Populated via [`BoundListeners::with_tls`] by Wave 2.4 daemon
    /// wiring; left `None` by [`bind_dual_with_local_auth`] so existing
    /// callers stay source-compatible.
    pub tls: Option<ApiTlsConfig>,
}

#[cfg(unix)]
impl BoundListeners {
    /// Attach an optional TLS configuration to the bound TCP listener.
    ///
    /// Returns `self` for chained construction:
    /// `bind_dual_with_local_auth(...).await?.with_tls(Some(cfg))`.
    #[must_use]
    pub fn with_tls(mut self, tls: Option<ApiTlsConfig>) -> Self {
        self.tls = tls;
        self
    }
}

/// Bind TCP and Unix listeners and mint the local admin JWT token.
///
/// This is the "early" half of what [`ApiServer::run_dual_with_local_auth`]
/// used to do in one shot.  Call it **before** any logic that requires the
/// Unix socket file to exist on disk (e.g. container rootfs restore that
/// bind-mounts the socket), then pass the returned [`BoundListeners`] to
/// [`serve_bound`] once the router is ready.
///
/// # Errors
///
/// Returns an error if binding fails or the admin token cannot be minted.
#[cfg(unix)]
pub async fn bind_dual_with_local_auth(
    tcp_addr: SocketAddr,
    unix_path: impl AsRef<Path>,
    jwt_secret: &str,
) -> anyhow::Result<BoundListeners> {
    use std::os::unix::fs::PermissionsExt;

    let unix_path = unix_path.as_ref().to_path_buf();

    // Clean up stale socket file if it exists
    let _ = std::fs::remove_file(&unix_path);

    // Ensure parent directory exists
    if let Some(parent) = unix_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Mint a long-lived (24h) admin token for local socket connections
    let local_token = crate::auth::create_token(
        jwt_secret,
        "local-admin",
        std::time::Duration::from_secs(86400),
        vec!["admin".to_string()],
    )
    .map_err(|e| anyhow::anyhow!("Failed to mint local admin token: {e}"))?;

    let local_bearer = format!("Bearer {local_token}");

    // Bind TCP listener
    let tcp = TcpListener::bind(tcp_addr).await?;
    let tcp_local_addr = tcp.local_addr()?;

    // Bind Unix listener
    let unix = UnixListener::bind(&unix_path)?;

    // Socket perms 0o660: only root and members of the socket's group can
    // connect. The UDS middleware injects an admin Bearer for any request
    // lacking `Authorization`, so anyone who can `connect()` to this socket
    // gains daemon admin. Keeping the perms tight is the access-control.
    // Non-root operators that need CLI access should be added to the
    // daemon's primary group.
    std::fs::set_permissions(&unix_path, std::fs::Permissions::from_mode(0o660))?;

    #[cfg(unix)]
    {
        use nix::unistd::{chown, Gid, Group};
        match Group::from_name("zlayer") {
            Ok(Some(grp)) => {
                let gid = Gid::from_raw(grp.gid.as_raw());
                if let Err(e) = chown(&unix_path, None, Some(gid)) {
                    tracing::debug!(
                        error = %e,
                        group = "zlayer",
                        socket = %unix_path.display(),
                        "failed to chown socket to zlayer group; falling back to default group ownership"
                    );
                } else {
                    tracing::info!(
                        socket = %unix_path.display(),
                        "socket chowned to root:zlayer 0o660"
                    );
                }
            }
            Ok(None) => {
                tracing::debug!(
                    socket = %unix_path.display(),
                    "group 'zlayer' not present on this system; skipping socket chown"
                );
            }
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    "failed to look up 'zlayer' group; skipping socket chown"
                );
            }
        }
    }

    info!(
        tcp = %tcp_local_addr,
        unix = %unix_path.display(),
        "Bound TCP and Unix listeners (with local auth token)"
    );

    Ok(BoundListeners {
        tcp,
        tcp_local_addr,
        unix,
        local_bearer,
        unix_path,
        tls: None,
    })
}

/// Serve the given router on pre-bound TCP and Unix listeners.
///
/// This is the "late" half of what [`ApiServer::run_dual_with_local_auth`]
/// used to do.  The Unix listener automatically gets a middleware layer that
/// injects the admin bearer token from [`BoundListeners::local_bearer`] into
/// requests that lack an `Authorization` header.
///
/// # Errors
///
/// Returns an error if either server encounters an error during operation.
///
/// # Panics
///
/// Panics if the bearer token header value cannot be parsed (should not
/// happen with valid JWT tokens).
#[cfg(unix)]
pub async fn serve_bound(
    listeners: BoundListeners,
    router: Router,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    use axum::http::header::AUTHORIZATION;
    use axum::middleware;

    let bearer = listeners.local_bearer;
    let unix_path = listeners.unix_path;
    let tls = listeners.tls;

    persist_admin_bearer(&bearer);

    info!(
        tcp = %listeners.tcp_local_addr,
        unix = %unix_path.display(),
        tls = tls.is_some(),
        "Starting API server on TCP and Unix socket (with local auth bypass)"
    );

    // Use a watch channel to fan out the shutdown signal to both servers
    let (shutdown_tx, mut shutdown_rx_tcp) = tokio::sync::watch::channel(false);
    let mut shutdown_rx_unix = shutdown_tx.subscribe();

    // Spawn a task that awaits the caller's shutdown future, then signals both servers
    tokio::spawn(async move {
        shutdown.await;
        let _ = shutdown_tx.send(true);
    });

    // Clone the router so each server gets its own copy
    let tcp_router = router.clone();

    // Add local auth injection middleware to the Unix router
    let unix_router = router.layer(middleware::from_fn(
        move |mut request: axum::http::Request<axum::body::Body>, next: middleware::Next| {
            let bearer_clone = bearer.clone();
            async move {
                // Only inject auth if the request doesn't already have one
                if !request.headers().contains_key(AUTHORIZATION) {
                    request.headers_mut().insert(
                        AUTHORIZATION,
                        bearer_clone.parse().expect("valid header value"),
                    );
                }
                next.run(request).await
            }
        },
    ));

    // TCP server: branch on tls inside the helper. Plaintext today,
    // wrapped with rustls when Wave 2.4 sets `BoundListeners::tls`.
    let tcp_shutdown = async move {
        let _ = shutdown_rx_tcp.wait_for(|&v| v).await;
    };
    let tcp_future =
        serve_tcp_with_optional_tls(listeners.tcp, tcp_router, tls.as_ref(), tcp_shutdown);

    // Unix socket server without ConnectInfo (no TCP addresses on UDS).
    // UDS stays plaintext regardless of `tls` — TLS over a 0o660 local
    // socket buys nothing.
    let unix_server = axum::serve(listeners.unix, unix_router.into_make_service())
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx_unix.wait_for(|&v| v).await;
        });

    // Run both servers concurrently, stop when either errors or both finish
    let result = tokio::try_join!(tcp_future, async {
        unix_server.await.map_err(anyhow::Error::from)
    });

    // Clean up the Unix socket file
    let _ = std::fs::remove_file(&unix_path);

    info!("API server (dual) shut down");

    result.map(|_| ())
}

// ---------------------------------------------------------------------------
// Windows variants (TCP-only)
//
// Unix domain sockets are not supported by the ZLayer daemon on Windows yet,
// so the Windows flavour of `bind_dual_with_local_auth` + `serve_bound`
// binds TCP only. The local-auth bearer is still minted so that subsequent
// phases (F-7b) can wire it into a named-pipe or loopback-local-auth path
// without changing the call sites here.
// ---------------------------------------------------------------------------

/// Pre-bound TCP listener ready to be handed to the Windows [`serve_bound`].
#[cfg(windows)]
pub struct BoundListeners {
    /// Already-bound TCP listener.
    pub tcp: TcpListener,
    /// Local address the TCP listener is bound to.
    pub tcp_local_addr: SocketAddr,
    /// `Bearer <token>` value reserved for future loopback/local-auth paths.
    /// Not currently injected into any request — see Phase F-7b.
    pub local_bearer: String,
    /// Optional TLS configuration applied to the TCP listener at
    /// [`serve_bound`] time. `None` (the default) keeps today's plaintext
    /// HTTP behaviour. Populated via [`BoundListeners::with_tls`] by
    /// Wave 2.4 daemon wiring.
    pub tls: Option<ApiTlsConfig>,
}

#[cfg(windows)]
impl BoundListeners {
    /// Attach an optional TLS configuration to the bound TCP listener.
    ///
    /// Returns `self` for chained construction:
    /// `bind_dual_with_local_auth(...).await?.with_tls(Some(cfg))`.
    #[must_use]
    pub fn with_tls(mut self, tls: Option<ApiTlsConfig>) -> Self {
        self.tls = tls;
        self
    }
}

/// Bind a TCP listener and mint the local admin JWT token (Windows).
///
/// This is the Windows counterpart of the Unix `bind_dual_with_local_auth`.
/// It binds only the TCP listener — the `ZLayer` daemon does not yet expose a
/// named-pipe transport on Windows (tracked as Phase F-7b). The `unix_path`
/// argument is accepted to keep the call-site signature identical, but
/// treated as opaque: if it looks like a `tcp://host:port` URL the parsed
/// value is logged for diagnostics; otherwise it is ignored.
///
/// # Errors
///
/// Returns an error if binding fails or the admin token cannot be minted.
#[cfg(windows)]
pub async fn bind_dual_with_local_auth(
    tcp_addr: SocketAddr,
    unix_path: impl AsRef<Path>,
    jwt_secret: &str,
) -> anyhow::Result<BoundListeners> {
    // Mint a long-lived (24h) admin token. This is not yet injected into any
    // request path on Windows (the Unix version hangs it off the Unix-socket
    // router) but we still mint it so the value can be used by F-7b.
    let local_token = crate::auth::create_token(
        jwt_secret,
        "local-admin",
        std::time::Duration::from_secs(86400),
        vec!["admin".to_string()],
    )
    .map_err(|e| anyhow::anyhow!("Failed to mint local admin token: {e}"))?;

    let local_bearer = format!("Bearer {local_token}");

    let tcp = TcpListener::bind(tcp_addr).await?;
    let tcp_local_addr = tcp.local_addr()?;

    info!(
        tcp = %tcp_local_addr,
        hint = %unix_path.as_ref().display(),
        "Bound TCP listener (Windows: no Unix domain socket in this phase)"
    );

    Ok(BoundListeners {
        tcp,
        tcp_local_addr,
        local_bearer,
        tls: None,
    })
}

/// Serve the given router on a pre-bound TCP listener (Windows).
///
/// Windows counterpart to the Unix `serve_bound`. Runs a single axum server
/// on [`BoundListeners::tcp`] until `shutdown` resolves.
///
/// # Errors
///
/// Returns an error if the server encounters an error during operation.
#[cfg(windows)]
pub async fn serve_bound(
    listeners: BoundListeners,
    router: Router,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    persist_admin_bearer(&listeners.local_bearer);
    info!(
        tcp = %listeners.tcp_local_addr,
        tls = listeners.tls.is_some(),
        "Starting API server on TCP (Windows)"
    );

    serve_tcp_with_optional_tls(listeners.tcp, router, listeners.tls.as_ref(), shutdown).await?;

    info!("API server (TCP) shut down");
    Ok(())
}

/// API server
pub struct ApiServer {
    config: ApiConfig,
}

impl ApiServer {
    /// Create a new API server
    #[must_use]
    pub fn new(config: ApiConfig) -> Self {
        Self { config }
    }

    /// Get the bind address
    #[must_use]
    pub fn bind_addr(&self) -> SocketAddr {
        self.config.bind
    }

    /// Run the server.
    ///
    /// # Errors
    ///
    /// Returns an error if binding to the address or serving fails.
    pub async fn run(self) -> anyhow::Result<()> {
        let addr = self.config.bind;
        let tls = self.config.tls.clone();
        let router = build_router(&self.config);

        info!(
            bind = %addr,
            swagger = self.config.swagger_enabled,
            rate_limit = self.config.rate_limit.enabled,
            tls = tls.is_some(),
            "Starting API server"
        );

        let listener = TcpListener::bind(addr).await?;

        // No shutdown future is supplied here — `run` is the one-shot
        // serve-forever entry point. `std::future::pending()` parks the
        // graceful-shutdown branch indefinitely so the server only exits
        // on an underlying I/O error.
        serve_tcp_with_optional_tls(listener, router, tls.as_ref(), std::future::pending::<()>())
            .await
    }

    /// Run the server with graceful shutdown.
    ///
    /// # Errors
    ///
    /// Returns an error if binding to the address or serving fails.
    pub async fn run_with_shutdown(
        self,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> anyhow::Result<()> {
        let addr = self.config.bind;
        let tls = self.config.tls.clone();
        let router = build_router(&self.config);

        info!(
            bind = %addr,
            swagger = self.config.swagger_enabled,
            tls = tls.is_some(),
            "Starting API server with graceful shutdown"
        );

        let listener = TcpListener::bind(addr).await?;

        serve_tcp_with_optional_tls(listener, router, tls.as_ref(), shutdown).await?;

        info!("API server shut down");
        Ok(())
    }

    /// Run the server on both TCP and Unix socket simultaneously with graceful shutdown.
    ///
    /// This serves the same router on a TCP listener (for external/network access) and
    /// a Unix domain socket (for local CLI-to-daemon IPC) concurrently. Both listeners
    /// are shut down gracefully when the provided shutdown future completes.
    ///
    /// The TCP listener preserves `ConnectInfo<SocketAddr>` for handlers that need
    /// client address information. The Unix listener uses plain `into_make_service()`
    /// since Unix sockets don't have TCP socket addresses.
    ///
    /// # Arguments
    /// * `tcp_addr` - TCP address to bind (e.g. `0.0.0.0:3669`)
    /// * `unix_path` - Path for the Unix domain socket file (e.g. `/run/zlayer/api.sock`)
    /// * `router` - Pre-built axum Router to serve on both listeners
    /// * `shutdown` - Future that completes when shutdown is requested
    ///
    /// # Errors
    ///
    /// Returns an error if binding to the TCP address or Unix socket fails,
    /// or if either server encounters an error during operation.
    #[cfg(unix)]
    pub async fn run_dual(
        tcp_addr: SocketAddr,
        unix_path: impl AsRef<Path>,
        router: Router,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let unix_path = unix_path.as_ref().to_path_buf();

        // Clean up stale socket file if it exists
        let _ = std::fs::remove_file(&unix_path);

        // Ensure parent directory exists
        if let Some(parent) = unix_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Bind TCP listener
        let tcp_listener = TcpListener::bind(tcp_addr).await?;
        let tcp_local_addr = tcp_listener.local_addr()?;

        // Bind Unix listener
        let unix_listener = UnixListener::bind(&unix_path)?;

        // Socket perms 0o660: only root and members of the socket's group can
        // connect. The UDS middleware injects an admin Bearer for any request
        // lacking `Authorization`, so anyone who can `connect()` to this socket
        // gains daemon admin. Keeping the perms tight is the access-control.
        // Non-root operators that need CLI access should be added to the
        // daemon's primary group.
        std::fs::set_permissions(&unix_path, std::fs::Permissions::from_mode(0o660))?;

        #[cfg(unix)]
        {
            use nix::unistd::{chown, Gid, Group};
            match Group::from_name("zlayer") {
                Ok(Some(grp)) => {
                    let gid = Gid::from_raw(grp.gid.as_raw());
                    if let Err(e) = chown(&unix_path, None, Some(gid)) {
                        tracing::debug!(
                            error = %e,
                            group = "zlayer",
                            socket = %unix_path.display(),
                            "failed to chown socket to zlayer group; falling back to default group ownership"
                        );
                    } else {
                        tracing::info!(
                            socket = %unix_path.display(),
                            "socket chowned to root:zlayer 0o660"
                        );
                    }
                }
                Ok(None) => {
                    tracing::debug!(
                        socket = %unix_path.display(),
                        "group 'zlayer' not present on this system; skipping socket chown"
                    );
                }
                Err(e) => {
                    tracing::debug!(
                        error = %e,
                        "failed to look up 'zlayer' group; skipping socket chown"
                    );
                }
            }
        }

        info!(
            tcp = %tcp_local_addr,
            unix = %unix_path.display(),
            "Starting API server on TCP and Unix socket"
        );

        // Use a watch channel to fan out the shutdown signal to both servers
        let (shutdown_tx, mut shutdown_rx_tcp) = tokio::sync::watch::channel(false);
        let mut shutdown_rx_unix = shutdown_tx.subscribe();

        // Spawn a task that awaits the caller's shutdown future, then signals both servers
        tokio::spawn(async move {
            shutdown.await;
            let _ = shutdown_tx.send(true);
        });

        // Clone the router so each server gets its own copy
        let tcp_router = router.clone();
        let unix_router = router.clone();

        // TCP server: plaintext only — `run_dual` takes no `ApiConfig` and
        // no explicit TLS source. Callers that need TLS use
        // `ApiServer::run_with_shutdown` or `serve_bound` with a
        // `BoundListeners::with_tls(Some(...))`-decorated listener.
        let tcp_shutdown = async move {
            let _ = shutdown_rx_tcp.wait_for(|&v| v).await;
        };
        let tcp_future = serve_tcp_with_optional_tls(tcp_listener, tcp_router, None, tcp_shutdown);

        // Unix socket server without ConnectInfo (no TCP addresses on UDS)
        let unix_server = axum::serve(unix_listener, unix_router.into_make_service())
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx_unix.wait_for(|&v| v).await;
            });

        // Run both servers concurrently, stop when either errors or both finish
        let result = tokio::try_join!(tcp_future, async {
            unix_server.await.map_err(anyhow::Error::from)
        });

        // Clean up the Unix socket file
        let _ = std::fs::remove_file(&unix_path);

        info!("API server (dual) shut down");

        result.map(|_| ())
    }

    /// Run dual TCP + Unix socket server with automatic admin auth on the Unix socket.
    ///
    /// Convenience wrapper that calls [`bind_dual_with_local_auth`] followed by
    /// [`serve_bound`].  Existing callers that don't need to split bind from
    /// serve can continue using this method unchanged.
    ///
    /// # Arguments
    /// * `tcp_addr` - TCP address to bind
    /// * `unix_path` - Path for the Unix domain socket file
    /// * `router` - Pre-built axum Router
    /// * `jwt_secret` - JWT secret used to mint the local admin token
    /// * `shutdown` - Future that completes when shutdown is requested
    ///
    /// # Errors
    ///
    /// Returns an error if binding to the TCP address or Unix socket fails,
    /// token creation fails, or either server encounters an error.
    ///
    /// # Panics
    ///
    /// Panics if the bearer token header value cannot be parsed (should not
    /// happen with valid JWT tokens).
    #[cfg(unix)]
    pub async fn run_dual_with_local_auth(
        tcp_addr: SocketAddr,
        unix_path: impl AsRef<Path>,
        router: Router,
        jwt_secret: &str,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> anyhow::Result<()> {
        let listeners = bind_dual_with_local_auth(tcp_addr, unix_path, jwt_secret).await?;
        serve_bound(listeners, router, shutdown).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_creation() {
        let config = ApiConfig::default();
        let server = ApiServer::new(config);
        assert_eq!(server.bind_addr(), "0.0.0.0:3669".parse().unwrap());
    }

    #[test]
    fn test_server_custom_bind() {
        let config = ApiConfig {
            bind: "127.0.0.1:9090".parse().unwrap(),
            ..Default::default()
        };
        let server = ApiServer::new(config);
        assert_eq!(server.bind_addr(), "127.0.0.1:9090".parse().unwrap());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_run_dual_starts_and_shuts_down() {
        use axum::routing::get;
        use std::time::Duration;

        let tmp_dir = ZLayerDirs::system_default()
            .scratch_dir("test-run-dual-starts-and-shuts-down-")
            .unwrap();
        let sock_path = tmp_dir.path().join("test.sock");

        let router = Router::new().route("/health", get(|| async { "ok" }));

        // Use port 0 so the OS picks a free port
        let tcp_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

        // Signal shutdown after a brief delay
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let shutdown = async move {
            let _ = rx.await;
        };

        let sock_path_clone = sock_path.clone();
        let handle = tokio::spawn(async move {
            ApiServer::run_dual(tcp_addr, sock_path_clone, router, shutdown).await
        });

        // Give the servers a moment to bind
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Verify the Unix socket file was created
        assert!(sock_path.exists(), "Unix socket file should exist");

        // Verify Unix socket permissions are 0o660
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&sock_path).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o660,
                "Socket permissions should be 0o660, got {mode:o}",
            );
        }

        // Signal shutdown
        let _ = tx.send(());

        // Wait for clean shutdown
        let result = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("Server should shut down within 5 seconds")
            .expect("Server task should not panic");

        assert!(
            result.is_ok(),
            "run_dual should return Ok: {:?}",
            result.err()
        );

        // Socket file should be cleaned up
        assert!(
            !sock_path.exists(),
            "Unix socket file should be removed after shutdown"
        );
    }

    // -----------------------------------------------------------------------
    // TLS plumbing tests (Wave 2.2)
    //
    // These verify the new `serve_tcp_with_optional_tls` helper's three
    // branches without binding real listeners. The static-cert PEM-loading
    // path is exercised with an rcgen-generated self-signed cert written to
    // a tempdir — no network, no system openssl.
    // -----------------------------------------------------------------------

    /// Write an rcgen self-signed cert + key to a tempdir for static-cert
    /// loading tests. Returns the paths.
    fn write_self_signed_pem(dir: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
        let key_pair = rcgen::KeyPair::generate().expect("rcgen keypair");
        let params =
            rcgen::CertificateParams::new(vec!["localhost".to_string()]).expect("rcgen params");
        let cert = params.self_signed(&key_pair).expect("self-sign");

        let cert_path = dir.join("test.crt");
        let key_path = dir.join("test.key");
        std::fs::write(&cert_path, cert.pem()).expect("write cert pem");
        std::fs::write(&key_path, key_pair.serialize_pem()).expect("write key pem");
        (cert_path, key_path)
    }

    #[tokio::test]
    async fn load_static_rustls_config_loads_self_signed_pem() {
        let tmp_dir = ZLayerDirs::system_default()
            .scratch_dir("test-static-rustls-config-")
            .unwrap();
        let (cert_path, key_path) = write_self_signed_pem(tmp_dir.path());

        let result = load_static_rustls_config(&cert_path, &key_path).await;
        assert!(
            result.is_ok(),
            "Expected static rustls config to load from self-signed PEM, got: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn load_static_rustls_config_missing_files_returns_err() {
        let tmp_dir = ZLayerDirs::system_default()
            .scratch_dir("test-static-rustls-missing-")
            .unwrap();
        let cert_path = tmp_dir.path().join("does-not-exist.crt");
        let key_path = tmp_dir.path().join("does-not-exist.key");

        let result = load_static_rustls_config(&cert_path, &key_path).await;
        assert!(
            result.is_err(),
            "Expected load_static_rustls_config to fail when files are missing"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("does-not-exist.crt") || err.contains("does-not-exist.key"),
            "Error should mention one of the missing paths: {err}"
        );
    }

    #[test]
    fn install_default_crypto_provider_is_idempotent() {
        // Two back-to-back calls must not panic. The first call installs
        // (or no-ops if some other test already did). The second is a
        // guaranteed no-op via `std::sync::Once`.
        install_default_crypto_provider();
        install_default_crypto_provider();
    }

    #[test]
    fn bound_listeners_with_tls_attaches_config() {
        // Verifies the builder method threads an `ApiTlsConfig::Static`
        // value through without losing it. We construct the struct by
        // hand here (rather than via `bind_dual_with_local_auth`) so the
        // test stays a pure unit test — no listener binds, no token mint.
        //
        // `BoundListeners` carries platform-conditional fields (`unix` /
        // `unix_path` on unix), so this assertion is itself gated.
        #[cfg(unix)]
        {
            // Bind ephemeral listeners just for struct construction. Using
            // 127.0.0.1:0 + a tempdir socket so this is local and clean.
            let tmp_dir = ZLayerDirs::system_default()
                .scratch_dir("test-with-tls-")
                .unwrap();
            let sock_path = tmp_dir.path().join("with-tls.sock");

            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            rt.block_on(async {
                let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let tcp_local_addr = tcp.local_addr().unwrap();
                let unix = UnixListener::bind(&sock_path).unwrap();

                let listeners = BoundListeners {
                    tcp,
                    tcp_local_addr,
                    unix,
                    local_bearer: "Bearer test".to_string(),
                    unix_path: sock_path.clone(),
                    tls: None,
                };

                assert!(listeners.tls.is_none(), "default tls should be None");

                let listeners = listeners.with_tls(Some(ApiTlsConfig::Static {
                    cert_path: "/tmp/cert.pem".into(),
                    key_path: "/tmp/key.pem".into(),
                }));

                assert!(
                    matches!(listeners.tls, Some(ApiTlsConfig::Static { .. })),
                    "with_tls(Some(Static)) should leave the variant intact"
                );

                // Clean up.
                let _ = std::fs::remove_file(&sock_path);
            });
        }
    }

    #[tokio::test]
    async fn run_with_shutdown_constructs_plain_http_when_tls_none() {
        // Smoke-test: starting `ApiServer::run_with_shutdown` with the
        // default config (no TLS) hits the plaintext branch and shuts
        // down cleanly. Binds 127.0.0.1:0 so we don't collide with any
        // running daemon and the OS picks the port.
        use std::time::Duration;

        let config = ApiConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            tls: None,
            ..Default::default()
        };
        let server = ApiServer::new(config);

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let shutdown = async move {
            let _ = rx.await;
        };

        let handle = tokio::spawn(async move { server.run_with_shutdown(shutdown).await });

        // Give the server a moment to bind. If TLS construction were
        // accidentally entered here it would error out — see the
        // shutdown / timeout below for the failure mode.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Signal shutdown and wait. The plain-HTTP branch wires
        // `with_graceful_shutdown` directly, so this must return Ok
        // within the timeout.
        let _ = tx.send(());
        let result = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("server should shut down within 5s")
            .expect("server task should not panic");

        assert!(
            result.is_ok(),
            "run_with_shutdown with tls=None should return Ok: {:?}",
            result.err()
        );
    }
}
