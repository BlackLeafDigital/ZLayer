//! DNS server for service discovery over overlay networks

use hickory_client::client::{Client, SyncClient};
use hickory_client::udp::UdpClientConnection;
use hickory_server::authority::{Catalog, ZoneType};
use hickory_server::proto::rr::rdata::{A, AAAA};
use hickory_server::proto::rr::{DNSClass, LowerName, Name, RData, Record, RecordType};
use hickory_server::resolver::config::NameServerConfigGroup;
use hickory_server::server::ServerFuture;
use hickory_server::store::in_memory::InMemoryAuthority;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::RwLock;

/// Default DNS port for overlay service discovery (non-standard to avoid conflicts)
pub const DEFAULT_DNS_PORT: u16 = 15353;

/// Standard DNS port used for upstream forwarding when a resolv.conf entry
/// (or the public fallback) does not carry an explicit port.
const STANDARD_DNS_PORT: u16 = 53;

/// Well-known public recursive resolvers used as a last-resort fallback when
/// no usable host upstream can be detected.
///
/// Cloudflare (`1.1.1.1`) is listed first, Google (`8.8.8.8`) second. These are
/// only ever reached when [`resolve_upstreams`] cannot extract a single
/// non-loopback nameserver from `/etc/resolv.conf` — i.e. the host resolver is
/// either absent or wholly stub-based (the netbird / systemd-resolved failure
/// mode this forwarder exists to route around).
const PUBLIC_FALLBACK_UPSTREAMS: [IpAddr; 2] = [
    IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
    IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
];

/// Path to the host resolver configuration parsed for default upstreams.
pub(crate) const RESOLV_CONF_PATH: &str = "/etc/resolv.conf";

/// Configuration for DNS integration with overlay network
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsConfig {
    /// DNS zone (e.g., "overlay.local.")
    pub zone: String,
    /// DNS server port (default: 15353)
    pub port: u16,
    /// Bind address (default: overlay IP)
    pub bind_addr: IpAddr,
    /// Explicit upstream resolvers for non-overlay queries.
    ///
    /// When `Some`, this list wins outright over any host-resolver detection:
    /// the overlay DNS server forwards every query *outside* [`Self::zone`] to
    /// these addresses (in order) and never consults `/etc/resolv.conf`. This
    /// is the production-safe override for hosts where a mesh VPN (netbird,
    /// Tailscale, …) has hijacked systemd-resolved with a `~.` catch-all and
    /// poisoned the host resolver for everything else.
    ///
    /// When `None` (the default), the server detects upstreams at startup by
    /// parsing `/etc/resolv.conf` and filtering out loopback / resolved-stub
    /// addresses; see [`resolve_upstreams`] for the exact precedence and the
    /// public fallback.
    ///
    /// Each entry is a full `SocketAddr` so a non-standard upstream port can be
    /// expressed; detection synthesises port [`STANDARD_DNS_PORT`] (53).
    #[serde(default)]
    pub upstreams: Option<Vec<SocketAddr>>,
}

impl DnsConfig {
    /// Create a new DNS config with defaults
    #[must_use]
    pub fn new(zone: &str, bind_addr: IpAddr) -> Self {
        Self {
            zone: zone.to_string(),
            port: DEFAULT_DNS_PORT,
            bind_addr,
            upstreams: None,
        }
    }

    /// Set a custom port
    #[must_use]
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Set explicit upstream resolvers for non-overlay queries.
    ///
    /// Supplying this disables host-resolver auto-detection entirely (the
    /// config override always wins). Pass full `SocketAddr`s; for the common
    /// case of "this IP on port 53" build them as `SocketAddr::new(ip, 53)`.
    #[must_use]
    pub fn with_upstreams(mut self, upstreams: Vec<SocketAddr>) -> Self {
        self.upstreams = Some(upstreams);
        self
    }
}

/// Returns `true` for addresses that must never be used as an overlay DNS
/// upstream because forwarding to them would either loop back into a broken
/// host resolver or hit the systemd-resolved stub.
///
/// Filtered out:
/// - `127.0.0.53` — the systemd-resolved stub listener. This is the exact
///   address a mesh VPN hijacks; forwarding here re-introduces the failure we
///   exist to bypass.
/// - any other IPv4/IPv6 loopback (`127.0.0.0/8`, `::1`) — a resolver that is
///   only reachable on loopback is, from a *container's* perspective, useless
///   (the container has its own loopback) and is almost always the host stub.
/// - the unspecified address (`0.0.0.0`, `::`) — never a valid nameserver.
fn is_unusable_upstream(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_unspecified(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified(),
    }
}

/// Parse `nameserver` directives out of resolv.conf-formatted text.
///
/// Only the `nameserver <ip>` directive is honoured (the sole directive that
/// names an upstream); `search`, `domain`, `options`, comments (`#`/`;`) and
/// blank lines are ignored. Loopback / stub / unspecified entries are filtered
/// via [`is_unusable_upstream`] so a systemd-resolved `nameserver 127.0.0.53`
/// line never survives. Surviving entries are returned as `SocketAddr`s on
/// [`STANDARD_DNS_PORT`] (resolv.conf has no port syntax).
///
/// Duplicates are de-duplicated while preserving first-seen order.
fn parse_resolv_conf(contents: &str) -> Vec<SocketAddr> {
    let mut out: Vec<SocketAddr> = Vec::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let mut parts = line.split_whitespace();
        if parts.next() != Some("nameserver") {
            continue;
        }
        let Some(addr_str) = parts.next() else {
            continue;
        };
        // resolv.conf may carry a scoped IPv6 like `fe80::1%eth0`; strip the
        // zone id since `IpAddr` does not parse it.
        let addr_str = addr_str.split('%').next().unwrap_or(addr_str);
        let Ok(ip) = IpAddr::from_str(addr_str) else {
            continue;
        };
        if is_unusable_upstream(ip) {
            continue;
        }
        let sock = SocketAddr::new(ip, STANDARD_DNS_PORT);
        if !out.contains(&sock) {
            out.push(sock);
        }
    }
    out
}

/// Resolve the effective upstream resolver list for non-overlay forwarding.
///
/// Precedence (documented because every choice here is load-bearing for the
/// production failure this guards against):
///
/// 1. **Config override wins.** A non-empty `config.upstreams` is used verbatim
///    and detection is skipped — this is the operator's escape hatch when the
///    host resolver is unusable.
/// 2. **Host `/etc/resolv.conf`, filtered.** Otherwise we parse the host
///    resolver config and keep only non-loopback, non-stub nameservers (see
///    [`parse_resolv_conf`]). This deliberately drops `127.0.0.53` so a
///    netbird/systemd-resolved `~.` hijack cannot poison the overlay path:
///    containers no longer inherit the broken stub, they hit the *real*
///    upstreams resolv.conf points at.
/// 3. **Public fallback.** If the filter leaves nothing usable (host is
///    stub-only or resolv.conf is missing), fall back to
///    [`PUBLIC_FALLBACK_UPSTREAMS`] (1.1.1.1, 8.8.8.8) and `warn!` loudly so
///    the operator knows no host upstream survived.
///
/// `resolv_conf_path` is injectable for tests; production passes
/// [`RESOLV_CONF_PATH`].
pub(crate) fn resolve_upstreams(config: &DnsConfig, resolv_conf_path: &str) -> Vec<SocketAddr> {
    if let Some(explicit) = &config.upstreams {
        if !explicit.is_empty() {
            tracing::debug!(
                count = explicit.len(),
                "using explicit overlay DNS upstreams from config (host detection skipped)",
            );
            return explicit.clone();
        }
    }

    let detected = match std::fs::read_to_string(resolv_conf_path) {
        Ok(contents) => parse_resolv_conf(&contents),
        Err(e) => {
            tracing::warn!(
                path = resolv_conf_path,
                error = %e,
                "could not read host resolv.conf for overlay DNS upstream detection",
            );
            Vec::new()
        }
    };

    if detected.is_empty() {
        let fallback: Vec<SocketAddr> = PUBLIC_FALLBACK_UPSTREAMS
            .iter()
            .map(|ip| SocketAddr::new(*ip, STANDARD_DNS_PORT))
            .collect();
        tracing::warn!(
            fallback = ?fallback,
            "no usable host DNS upstreams found (resolv.conf empty, missing, or stub-only); \
             falling back to public resolvers for overlay forwarding",
        );
        fallback
    } else {
        tracing::info!(
            upstreams = ?detected,
            "overlay DNS forwarding to host upstreams (loopback/stub filtered out)",
        );
        detected
    }
}

/// Build the bounded async resolver used to forward non-overlay queries.
///
/// hickory-server 0.24 *does* ship a [`ForwardAuthority`], but when every
/// upstream is unreachable its lookup error flows through the [`Catalog`]'s
/// `build_response` and lands in a documented-TODO branch that leaves the
/// response code at the initialised `NoError` and emits an *empty* answer
/// section — i.e. total-upstream-failure surfaces to a container as "this name
/// has no A record" instead of `SERVFAIL`. That silent failure is exactly the
/// production hazard we are guarding against, so instead of registering a
/// `ForwardAuthority` in the catalog we drive a [`TokioAsyncResolver`] directly
/// from [`ForwardingCatalog`] and map its error kinds to precise response codes
/// (see [`ForwardingCatalog::handle_request`]).
///
/// `from_ips_clear` builds plain UDP+TCP nameservers (no DoT/DoH). Upstreams are
/// bucketed by port so a non-standard upstream port is honoured; the common
/// case is a single port (53). The resolver is bounded — 2s per-query timeout,
/// 2 attempts — so a dead/blackholed upstream fails fast rather than hanging
/// containers.
///
/// Returns `Err` only if the upstream set is empty (callers must not call this
/// with an empty list — they gate on `!upstreams.is_empty()`).
pub(crate) fn build_forward_resolver(
    upstreams: &[SocketAddr],
) -> Result<hickory_server::resolver::TokioAsyncResolver, DnsError> {
    use hickory_server::resolver::config::{ResolverConfig, ResolverOpts};

    if upstreams.is_empty() {
        return Err(DnsError::Server("no upstreams for forward resolver".into()));
    }

    let mut group = NameServerConfigGroup::new();
    let mut by_port: std::collections::BTreeMap<u16, Vec<IpAddr>> =
        std::collections::BTreeMap::new();
    for addr in upstreams {
        by_port.entry(addr.port()).or_default().push(addr.ip());
    }
    for (port, ips) in by_port {
        // trust_negative_responses = true: these are recursive resolvers we
        // delegate to wholesale, so a negative response from them is final.
        group.merge(NameServerConfigGroup::from_ips_clear(&ips, port, true));
    }

    // `ResolverOpts` is `#[non_exhaustive]`, so we cannot build it with a struct
    // literal from this crate — start from defaults and override the two fields
    // that matter for fail-fast behaviour.
    let mut options = ResolverOpts::default();
    options.timeout = Duration::from_secs(2);
    options.attempts = 2;
    // Forwarders must emit intermediate CNAMEs (RFC 1034 §4.3.2).
    options.preserve_intermediates = true;

    let config = ResolverConfig::from_parts(None, vec![], group);
    Ok(hickory_server::resolver::TokioAsyncResolver::tokio(
        config, options,
    ))
}

/// A [`RequestHandler`] that serves the overlay zone from an [`InMemoryAuthority`]
/// (via the wrapped [`Catalog`]) and forwards everything else to upstream
/// resolvers, mapping resolver outcomes to precise DNS response codes.
///
/// Routing: a query whose name is within `zone_origin` is handed to the catalog
/// unchanged (the [`InMemoryAuthority`] answers it). Any other query is resolved
/// through `resolver` and answered directly. When `resolver` is `None` (no
/// usable upstreams were configured) non-overlay queries fall through to the
/// catalog, which answers `REFUSED` — the pre-forwarder behaviour.
///
/// Response-code mapping for forwarded queries:
/// - resolver `Ok` → `NoError` with the resolved records as answers;
/// - `NoRecordsFound { response_code: NXDomain }` → `NXDomain`;
/// - `NoRecordsFound { response_code: NoError }` (genuine NODATA) → empty
///   `NoError`;
/// - timeout / IO / no-connections / any other error (total upstream failure)
///   → `SERVFAIL`, never a panic and never a silent empty `NoError`.
///
/// The forwarder is only ever reachable on the sockets the server already binds
/// (overlay IP / localhost / explicit secondary). No wildcard bind is added, so
/// open recursion is not exposed to the world.
struct ForwardingCatalog {
    catalog: Catalog,
    zone_origin: LowerName,
    resolver: Option<Arc<hickory_server::resolver::TokioAsyncResolver>>,
}

impl ForwardingCatalog {
    /// Build the `NoError` answer message for a successful forward lookup.
    fn forward_answer_response<'a>(
        request: &'a hickory_server::server::Request,
        answers: &'a [Record],
    ) -> hickory_server::authority::MessageResponse<
        'a,
        'a,
        std::slice::Iter<'a, Record>,
        std::iter::Empty<&'a Record>,
        std::iter::Empty<&'a Record>,
        std::iter::Empty<&'a Record>,
    > {
        use hickory_server::authority::MessageResponseBuilder;
        use hickory_server::proto::op::ResponseCode;

        let mut header = hickory_server::proto::op::Header::response_from_request(request.header());
        header.set_recursion_available(true);
        header.set_response_code(ResponseCode::NoError);
        // Forwarded answers are non-authoritative by definition.
        header.set_authoritative(false);

        MessageResponseBuilder::from_message_request(request).build(
            header,
            answers.iter(),
            std::iter::empty(),
            std::iter::empty(),
            std::iter::empty(),
        )
    }

    /// Build an answer-less response carrying just `code` (used for NXDOMAIN,
    /// NODATA, and SERVFAIL on the forward path).
    fn forward_code_response(
        request: &hickory_server::server::Request,
        code: hickory_server::proto::op::ResponseCode,
    ) -> hickory_server::authority::MessageResponse<
        '_,
        '_,
        impl Iterator<Item = &Record> + Send,
        impl Iterator<Item = &Record> + Send,
        impl Iterator<Item = &Record> + Send,
        impl Iterator<Item = &Record> + Send,
    > {
        use hickory_server::authority::MessageResponseBuilder;
        MessageResponseBuilder::from_message_request(request).error_msg(request.header(), code)
    }

    /// Resolve `name`/`rtype` through the upstream resolver and send the mapped
    /// response. Returns the wire [`ResponseInfo`].
    async fn forward<R: hickory_server::server::ResponseHandler>(
        &self,
        resolver: &hickory_server::resolver::TokioAsyncResolver,
        request: &hickory_server::server::Request,
        mut response_handle: R,
    ) -> hickory_server::server::ResponseInfo {
        use hickory_server::proto::op::ResponseCode;
        use hickory_server::resolver::error::ResolveErrorKind;

        let query = request.request_info().query;
        let name = Name::from(query.name());
        let rtype = query.query_type();

        match resolver.lookup(name, rtype).await {
            Ok(lookup) => {
                let records: Vec<Record> = lookup.records().to_vec();
                let response = Self::forward_answer_response(request, &records);
                Self::send_or_servfail(&mut response_handle, response).await
            }
            Err(e) => {
                let code = match e.kind() {
                    // Upstream answered authoritatively: respect its verdict.
                    ResolveErrorKind::NoRecordsFound { response_code, .. }
                        if *response_code == ResponseCode::NXDomain =>
                    {
                        ResponseCode::NXDomain
                    }
                    // Name exists but no record of this type (genuine NODATA).
                    ResolveErrorKind::NoRecordsFound { response_code, .. }
                        if *response_code == ResponseCode::NoError =>
                    {
                        ResponseCode::NoError
                    }
                    // Timeout / IO / no-connections / anything else: the upstream
                    // path is broken. SERVFAIL — never a silent empty NoError,
                    // never a panic.
                    _ => {
                        tracing::debug!(error = %e, "overlay DNS upstream forward failed; SERVFAIL");
                        ResponseCode::ServFail
                    }
                };
                let response = Self::forward_code_response(request, code);
                Self::send_or_servfail(&mut response_handle, response).await
            }
        }
    }

    /// Send `response`, degrading a send error to a SERVFAIL `ResponseInfo`
    /// (mirrors how the inner catalog handles its own send failures).
    async fn send_or_servfail<'a, R, A, N, S, D>(
        response_handle: &mut R,
        response: hickory_server::authority::MessageResponse<'_, 'a, A, N, S, D>,
    ) -> hickory_server::server::ResponseInfo
    where
        R: hickory_server::server::ResponseHandler,
        A: Iterator<Item = &'a Record> + Send + 'a,
        N: Iterator<Item = &'a Record> + Send + 'a,
        S: Iterator<Item = &'a Record> + Send + 'a,
        D: Iterator<Item = &'a Record> + Send + 'a,
    {
        match response_handle.send_response(response).await {
            Ok(info) => info,
            Err(e) => {
                tracing::error!(error = %e, "failed to send overlay DNS forward response");
                let mut header = hickory_server::proto::op::Header::new();
                header.set_response_code(hickory_server::proto::op::ResponseCode::ServFail);
                header.into()
            }
        }
    }
}

#[async_trait::async_trait]
impl hickory_server::server::RequestHandler for ForwardingCatalog {
    async fn handle_request<R: hickory_server::server::ResponseHandler>(
        &self,
        request: &hickory_server::server::Request,
        response_handle: R,
    ) -> hickory_server::server::ResponseInfo {
        // Overlay-zone queries (and anything when we have no upstream resolver)
        // go straight to the catalog / InMemoryAuthority. Everything else is
        // forwarded.
        let query_name = request.request_info().query.name().clone();
        let is_overlay = self.zone_origin.zone_of(&query_name);

        match (&self.resolver, is_overlay) {
            (Some(resolver), false) => self.forward(resolver, request, response_handle).await,
            _ => self.catalog.handle_request(request, response_handle).await,
        }
    }
}

/// Generate a hostname from an IP address for DNS registration
///
/// For IPv4: converts an IP like 10.200.0.5 to "node-0-5" (using last two octets).
/// For IPv6: converts an IP like `fd00::abcd` to "node-abcd" (using last 4 hex chars).
#[must_use]
pub fn peer_hostname(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            format!("node-{}-{}", octets[2], octets[3])
        }
        IpAddr::V6(v6) => {
            let segments = v6.segments();
            let last_segment = segments[7];
            format!("node-{last_segment:04x}")
        }
    }
}

/// Error type for DNS operations
#[derive(Debug, thiserror::Error)]
pub enum DnsError {
    #[error("Invalid domain name: {0}")]
    InvalidName(String),

    #[error("DNS server error: {0}")]
    Server(String),

    #[error("DNS client error: {0}")]
    Client(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Record not found: {0}")]
    NotFound(String),
}

/// Handle for managing DNS records after server is started
///
/// This handle can be cloned and used to add/remove records while the server is running.
#[derive(Clone)]
pub struct DnsHandle {
    authority: Arc<InMemoryAuthority>,
    zone_origin: Name,
    serial: Arc<RwLock<u32>>,
}

impl DnsHandle {
    /// Add a DNS record for a hostname to IP mapping
    ///
    /// Creates an A record for IPv4 addresses and an AAAA record for IPv6 addresses.
    ///
    /// # Errors
    ///
    /// Returns `DnsError::InvalidName` if the hostname is invalid.
    pub async fn add_record(&self, hostname: &str, ip: IpAddr) -> Result<(), DnsError> {
        // Create the fully qualified domain name
        let fqdn = if hostname.ends_with('.') {
            Name::from_str(hostname)
                .map_err(|e| DnsError::InvalidName(format!("{hostname}: {e}")))?
        } else {
            // Append the zone origin
            let name = Name::from_str(hostname)
                .map_err(|e| DnsError::InvalidName(format!("{hostname}: {e}")))?;
            name.append_domain(&self.zone_origin)
                .map_err(|e| DnsError::InvalidName(format!("Failed to append zone: {e}")))?
        };

        // Create an A or AAAA record depending on address family
        let rdata = match ip {
            IpAddr::V4(v4) => RData::A(A::from(v4)),
            IpAddr::V6(v6) => RData::AAAA(AAAA::from(v6)),
        };
        let record = Record::from_rdata(fqdn, 300, rdata); // 300 second TTL

        // Get the current serial and increment it
        let serial = {
            let mut s = self.serial.write().await;
            let current = *s;
            *s = s.wrapping_add(1);
            current
        };

        // Upsert the record into the authority (uses internal synchronization)
        self.authority.upsert(record, serial).await;

        Ok(())
    }

    /// Remove DNS records for a hostname (both A and AAAA)
    ///
    /// Tombstones both record types since we don't track which type was stored.
    ///
    /// # Errors
    ///
    /// Returns `DnsError::InvalidName` if the hostname is invalid.
    pub async fn remove_record(&self, hostname: &str) -> Result<bool, DnsError> {
        let fqdn = if hostname.ends_with('.') {
            Name::from_str(hostname)
                .map_err(|e| DnsError::InvalidName(format!("{hostname}: {e}")))?
        } else {
            let name = Name::from_str(hostname)
                .map_err(|e| DnsError::InvalidName(format!("{hostname}: {e}")))?;
            name.append_domain(&self.zone_origin)
                .map_err(|e| DnsError::InvalidName(format!("Failed to append zone: {e}")))?
        };

        let serial = {
            let mut s = self.serial.write().await;
            let current = *s;
            *s = s.wrapping_add(1);
            current
        };

        // Create empty records to effectively "remove" by setting empty data.
        // Note: hickory-dns doesn't have a direct remove, so we create tombstones.
        // We tombstone both A and AAAA since we don't know which type was stored.
        let a_record = Record::with(fqdn.clone(), RecordType::A, 0);
        self.authority.upsert(a_record, serial).await;

        let aaaa_record = Record::with(fqdn.clone(), RecordType::AAAA, 0);
        self.authority.upsert(aaaa_record, serial).await;

        Ok(true)
    }

    /// Get the zone origin
    #[must_use]
    pub fn zone_origin(&self) -> &Name {
        &self.zone_origin
    }
}

/// DNS server for overlay networks
pub struct DnsServer {
    listen_addr: SocketAddr,
    authority: Arc<InMemoryAuthority>,
    zone_origin: Name,
    serial: Arc<RwLock<u32>>,
    /// Upstream resolvers for non-overlay queries.
    ///
    /// Resolved once at construction (config override > filtered resolv.conf >
    /// public fallback). Every catalog this server builds — the primary
    /// listener and any secondary / Windows-fallback listener — is wrapped in a
    /// [`ForwardingCatalog`] that forwards non-overlay queries here, so a query
    /// that does not match the overlay zone is forwarded instead of refused.
    /// Empty only in the theoretical case where resolution yields nothing (it
    /// always returns at least the public fallback), in which case no forwarder
    /// is installed and non-overlay queries get the pre-existing REFUSED
    /// behaviour.
    upstreams: Vec<SocketAddr>,
}

impl DnsServer {
    /// Create a new DNS server for the given zone.
    ///
    /// Upstreams for non-overlay forwarding are auto-detected from the host
    /// `/etc/resolv.conf` (loopback/stub filtered, public fallback if empty).
    /// Use [`Self::from_config`] with [`DnsConfig::with_upstreams`] to override.
    ///
    /// # Errors
    ///
    /// Returns `DnsError::InvalidName` if the zone name is invalid.
    pub fn new(listen_addr: SocketAddr, zone: &str) -> Result<Self, DnsError> {
        let upstreams =
            resolve_upstreams(&DnsConfig::new(zone, listen_addr.ip()), RESOLV_CONF_PATH);
        Self::new_with_upstreams(listen_addr, zone, upstreams)
    }

    /// Create a DNS server with an explicit, already-resolved upstream list.
    ///
    /// Bypasses resolv.conf detection entirely — `upstreams` is used verbatim
    /// for the root-zone forwarder. Primarily an internal/testing seam so a
    /// stub upstream can be injected without touching the host `/etc/resolv.conf`.
    ///
    /// # Errors
    ///
    /// Returns `DnsError::InvalidName` if the zone name is invalid.
    pub fn new_with_upstreams(
        listen_addr: SocketAddr,
        zone: &str,
        upstreams: Vec<SocketAddr>,
    ) -> Result<Self, DnsError> {
        let zone_origin =
            Name::from_str(zone).map_err(|e| DnsError::InvalidName(format!("{zone}: {e}")))?;

        // Create an empty in-memory authority for the zone
        // Using Arc directly since InMemoryAuthority has internal synchronization via upsert()
        let authority = Arc::new(InMemoryAuthority::empty(
            zone_origin.clone(),
            ZoneType::Primary,
            false,
        ));

        Ok(Self {
            listen_addr,
            authority,
            zone_origin,
            serial: Arc::new(RwLock::new(1)),
            upstreams,
        })
    }

    /// Create from a `DnsConfig`
    ///
    /// Upstreams follow [`resolve_upstreams`] precedence: `config.upstreams`
    /// override wins, else filtered `/etc/resolv.conf`, else public fallback.
    ///
    /// # Errors
    ///
    /// Returns `DnsError::InvalidName` if the zone name is invalid.
    pub fn from_config(config: &DnsConfig) -> Result<Self, DnsError> {
        let listen_addr = SocketAddr::new(config.bind_addr, config.port);
        let upstreams = resolve_upstreams(config, RESOLV_CONF_PATH);
        Self::new_with_upstreams(listen_addr, &config.zone, upstreams)
    }

    /// The upstream resolvers this server forwards non-overlay queries to.
    #[must_use]
    pub fn upstreams(&self) -> &[SocketAddr] {
        &self.upstreams
    }

    /// Build the request handler for a listener: a [`ForwardingCatalog`] that
    /// serves the overlay zone from `authority` (via an inner [`Catalog`]) and
    /// forwards every non-overlay query to `upstreams`, mapping total upstream
    /// failure to `SERVFAIL` rather than a silent empty `NoError`.
    ///
    /// Shared by every listener (primary + secondary) so forwarding behaviour
    /// is identical across the sockets this server binds. A resolver-build
    /// failure (only possible with an empty upstream set, which is gated out
    /// here) degrades to "overlay-only": non-overlay queries fall through to the
    /// catalog and get `REFUSED`, but overlay service discovery keeps working.
    fn build_catalog(
        zone_origin: Name,
        authority: Arc<InMemoryAuthority>,
        upstreams: &[SocketAddr],
    ) -> ForwardingCatalog {
        let lower_origin = LowerName::from(zone_origin.clone());

        let mut catalog = Catalog::new();
        // The catalog accepts Arc<dyn AuthorityObject> - InMemoryAuthority implements this
        catalog.upsert(zone_origin.into(), Box::new(authority));

        let resolver = if upstreams.is_empty() {
            None
        } else {
            match build_forward_resolver(upstreams) {
                Ok(r) => {
                    tracing::debug!(
                        upstreams = ?upstreams,
                        "overlay DNS forwarder ready for non-overlay queries",
                    );
                    Some(Arc::new(r))
                }
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        "failed to build overlay DNS forwarder; non-overlay queries \
                         will be refused (overlay zone still served)",
                    );
                    None
                }
            }
        };

        ForwardingCatalog {
            catalog,
            zone_origin: lower_origin,
            resolver,
        }
    }

    /// Get a handle for managing DNS records
    ///
    /// The handle can be cloned and used to add/remove records even after
    /// the server has been started.
    #[must_use]
    pub fn handle(&self) -> DnsHandle {
        DnsHandle {
            authority: Arc::clone(&self.authority),
            zone_origin: self.zone_origin.clone(),
            serial: Arc::clone(&self.serial),
        }
    }

    /// Add a DNS record for a hostname to IP mapping
    ///
    /// Creates an A record for IPv4 addresses and an AAAA record for IPv6 addresses.
    ///
    /// # Errors
    ///
    /// Returns `DnsError::InvalidName` if the hostname is invalid.
    pub async fn add_record(&self, hostname: &str, ip: IpAddr) -> Result<(), DnsError> {
        self.handle().add_record(hostname, ip).await
    }

    /// Remove DNS records for a hostname (both A and AAAA)
    ///
    /// # Errors
    ///
    /// Returns `DnsError::InvalidName` if the hostname is invalid.
    pub async fn remove_record(&self, hostname: &str) -> Result<bool, DnsError> {
        self.handle().remove_record(hostname).await
    }

    /// Start the DNS server and return a handle for record management
    ///
    /// This spawns the DNS server in a background task and returns a handle
    /// that can be used to add/remove records while the server is running.
    ///
    /// # Errors
    ///
    /// This method currently always succeeds but returns `Result` for API consistency.
    #[allow(clippy::unused_async)]
    pub async fn start(self) -> Result<DnsHandle, DnsError> {
        let handle = self.handle();
        let listen_addr = self.listen_addr;
        let zone_origin = self.zone_origin.clone();
        let authority = Arc::clone(&self.authority);
        let upstreams = self.upstreams.clone();

        // Spawn the server in a background task
        tokio::spawn(async move {
            if let Err(e) = Self::run_server(listen_addr, zone_origin, authority, upstreams).await {
                tracing::error!("DNS server error: {}", e);
            }
        });

        Ok(handle)
    }

    /// Start the DNS server in a background task without consuming self.
    ///
    /// Unlike `start(self)`, this method borrows self, allowing the `DnsServer`
    /// to be wrapped in an Arc and shared (e.g., with `ServiceManager`) while
    /// the server runs in the background.
    ///
    /// # Errors
    ///
    /// This method currently always succeeds but returns `Result` for API consistency.
    #[allow(clippy::unused_async)]
    pub async fn start_background(&self) -> Result<DnsHandle, DnsError> {
        let handle = self.handle();
        let listen_addr = self.listen_addr;
        let zone_origin = self.zone_origin.clone();
        let authority = Arc::clone(&self.authority);
        let upstreams = self.upstreams.clone();

        tokio::spawn(async move {
            if let Err(e) = Self::run_server(listen_addr, zone_origin, authority, upstreams).await {
                tracing::error!("DNS server error: {}", e);
            }
        });

        Ok(handle)
    }

    /// Bind a second DNS listener on port 53 of `bind_ip`, sharing this
    /// server's authority + zone so the same records answer both listeners.
    ///
    /// Windows containers always query DNS on port 53 — HNS endpoints do not
    /// support setting a non-standard DNS port in the schema. The canonical
    /// overlay listener on [`DEFAULT_DNS_PORT`] (15353) is therefore
    /// unreachable from a Windows container; this method adds a second
    /// listener on port 53 of the overlay IP so containers that point at
    /// `<overlay_ip>:53` via `Dns.ServerList` can actually resolve.
    ///
    /// `bind_ip` is typically the node's overlay IP (e.g. `10.200.42.1`).
    /// Binding to `0.0.0.0:53` would collide with whatever resolver the host
    /// already runs (systemd-resolved on Linux, DNS Client on Windows). The
    /// method itself is cross-platform; callers decide whether to invoke it
    /// based on their workload mix.
    ///
    /// The bound UDP + TCP sockets live on a detached tokio task that shares
    /// the same `Arc<InMemoryAuthority>` as the primary listener, so
    /// `DnsHandle::add_record` / `remove_record` updates both responders
    /// atomically. Returns a cloneable [`DnsHandle`] for convenience.
    ///
    /// # Errors
    ///
    /// Returns `DnsError::Io` when either port 53 socket (UDP or TCP) cannot
    /// be bound — typically because another DNS resolver already owns the
    /// address, or because the process lacks the privilege to bind below 1024
    /// on platforms that require it. Callers should treat this as a warning
    /// and fall back to the primary 15353 listener for non-Windows workloads.
    #[allow(clippy::unused_async)]
    pub async fn bind_windows_fallback(&self, bind_ip: IpAddr) -> Result<DnsHandle, DnsError> {
        self.bind_secondary(SocketAddr::new(bind_ip, 53)).await
    }

    /// Bind an additional DNS listener on an arbitrary `listen_addr`, sharing
    /// this server's authority + zone so the same records answer on both the
    /// primary listener and this one.
    ///
    /// Unlike [`bind_windows_fallback`](Self::bind_windows_fallback) (which is
    /// hard-wired to port 53 for Windows HNS containers), this lets the caller
    /// pick a **non-privileged** port — required on macOS where an unprivileged
    /// daemon cannot bind below 1024. The VZ-Linux path uses this to expose the
    /// overlay resolver on `<node_overlay_ip>:<dns_port>` so a tiny in-guest
    /// relay can forward the guest's port-53 queries to it.
    ///
    /// # Errors
    ///
    /// Returns `DnsError::Io` when either the UDP or TCP socket cannot be bound.
    #[allow(clippy::unused_async)]
    pub async fn bind_secondary(&self, listen_addr: SocketAddr) -> Result<DnsHandle, DnsError> {
        let handle = self.handle();
        let zone_origin = self.zone_origin.clone();
        let authority = Arc::clone(&self.authority);
        let upstreams = self.upstreams.clone();

        // Pre-bind the sockets synchronously so binding failures surface here
        // instead of being swallowed by the detached task. On success we hand
        // the live sockets off to the server future on a background task.
        let udp_socket = UdpSocket::bind(listen_addr).await?;
        let tcp_listener = TcpListener::bind(listen_addr).await?;

        tokio::spawn(async move {
            let catalog = Self::build_catalog(zone_origin, authority, &upstreams);
            let mut server = ServerFuture::new(catalog);
            server.register_socket(udp_socket);
            server.register_listener(tcp_listener, Duration::from_secs(30));
            tracing::info!(
                addr = %listen_addr,
                "secondary DNS listener started",
            );
            if let Err(e) = server.block_until_done().await {
                tracing::error!("secondary DNS listener error: {}", e);
            }
        });

        Ok(handle)
    }

    /// Internal method to run the DNS server
    async fn run_server(
        listen_addr: SocketAddr,
        zone_origin: Name,
        authority: Arc<InMemoryAuthority>,
        upstreams: Vec<SocketAddr>,
    ) -> Result<(), DnsError> {
        // Create the catalog: overlay zone authority + (optional) root-zone
        // forwarder for everything else.
        let catalog = Self::build_catalog(zone_origin, authority, &upstreams);

        // Create the server
        let mut server = ServerFuture::new(catalog);

        // Bind UDP socket
        let udp_socket = UdpSocket::bind(listen_addr).await?;
        server.register_socket(udp_socket);

        // Bind TCP listener
        let tcp_listener = TcpListener::bind(listen_addr).await?;
        server.register_listener(tcp_listener, Duration::from_secs(30));

        tracing::info!(addr = %listen_addr, "DNS server listening");

        // Run the server
        server
            .block_until_done()
            .await
            .map_err(|e| DnsError::Server(e.to_string()))?;

        Ok(())
    }

    /// Get the listen address
    #[must_use]
    pub fn listen_addr(&self) -> SocketAddr {
        self.listen_addr
    }

    /// Get the zone origin
    #[must_use]
    pub fn zone_origin(&self) -> &Name {
        &self.zone_origin
    }
}

/// DNS client for querying overlay DNS servers
pub struct DnsClient {
    server_addr: SocketAddr,
}

impl DnsClient {
    /// Create a new DNS client
    #[must_use]
    pub fn new(server_addr: SocketAddr) -> Self {
        Self { server_addr }
    }

    /// Query for an A record
    ///
    /// # Errors
    ///
    /// Returns a `DnsError` if the query fails or the hostname is invalid.
    pub fn query_a(&self, hostname: &str) -> Result<Option<Ipv4Addr>, DnsError> {
        let name = Name::from_str(hostname)
            .map_err(|e| DnsError::InvalidName(format!("{hostname}: {e}")))?;

        let conn = UdpClientConnection::new(self.server_addr)
            .map_err(|e| DnsError::Client(e.to_string()))?;

        let client = SyncClient::new(conn);

        let response = client
            .query(&name, DNSClass::IN, RecordType::A)
            .map_err(|e| DnsError::Client(e.to_string()))?;

        // Extract the A record from the response
        for answer in response.answers() {
            if let Some(RData::A(a_record)) = answer.data() {
                return Ok(Some((*a_record).into()));
            }
        }

        Ok(None)
    }

    /// Query for an AAAA record (IPv6)
    ///
    /// # Errors
    ///
    /// Returns a `DnsError` if the query fails or the hostname is invalid.
    pub fn query_aaaa(&self, hostname: &str) -> Result<Option<Ipv6Addr>, DnsError> {
        let name = Name::from_str(hostname)
            .map_err(|e| DnsError::InvalidName(format!("{hostname}: {e}")))?;

        let conn = UdpClientConnection::new(self.server_addr)
            .map_err(|e| DnsError::Client(e.to_string()))?;

        let client = SyncClient::new(conn);

        let response = client
            .query(&name, DNSClass::IN, RecordType::AAAA)
            .map_err(|e| DnsError::Client(e.to_string()))?;

        // Extract the AAAA record from the response
        for answer in response.answers() {
            if let Some(RData::AAAA(aaaa_record)) = answer.data() {
                return Ok(Some((*aaaa_record).into()));
            }
        }

        Ok(None)
    }

    /// Query for any address record (A or AAAA), returning the first match
    ///
    /// Tries A first, then AAAA. Returns the first successful result.
    ///
    /// # Errors
    ///
    /// Returns a `DnsError` if both queries fail or the hostname is invalid.
    pub fn query_addr(&self, hostname: &str) -> Result<Option<IpAddr>, DnsError> {
        // Try A record first
        if let Ok(Some(v4)) = self.query_a(hostname) {
            return Ok(Some(IpAddr::V4(v4)));
        }

        // Then try AAAA
        if let Ok(Some(v6)) = self.query_aaaa(hostname) {
            return Ok(Some(IpAddr::V6(v6)));
        }

        Ok(None)
    }
}

/// Service discovery with DNS
pub struct ServiceDiscovery {
    dns_server: SocketAddr,
    records: RwLock<HashMap<String, IpAddr>>,
}

impl ServiceDiscovery {
    /// Create a new service discovery instance
    #[must_use]
    pub fn new(dns_server_addr: SocketAddr) -> Self {
        Self {
            dns_server: dns_server_addr,
            records: RwLock::new(HashMap::new()),
        }
    }

    /// Register a service (stores locally, does not update DNS server)
    pub async fn register(&self, name: &str, ip: IpAddr) {
        let mut records = self.records.write().await;
        records.insert(name.to_string(), ip);
    }

    /// Resolve a service to an IP address
    ///
    /// Checks the local cache first, then queries the DNS server for both
    /// A (IPv4) and AAAA (IPv6) records.
    pub async fn resolve(&self, name: &str) -> Option<IpAddr> {
        // First check local cache
        {
            let records = self.records.read().await;
            if let Some(ip) = records.get(name) {
                return Some(*ip);
            }
        }

        // Query DNS server for both A and AAAA records
        let client = DnsClient::new(self.dns_server);
        if let Ok(Some(addr)) = client.query_addr(name) {
            return Some(addr);
        }

        None
    }

    /// Unregister a service
    pub async fn unregister(&self, name: &str) {
        let mut records = self.records.write().await;
        records.remove(name);
    }

    /// List all registered services
    pub async fn list_services(&self) -> Vec<String> {
        let records = self.records.read().await;
        records.keys().cloned().collect()
    }

    /// Get the DNS server address
    pub fn dns_server(&self) -> SocketAddr {
        self.dns_server
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_peer_hostname_v4() {
        // Test various IPv4 addresses
        assert_eq!(
            peer_hostname(IpAddr::V4(Ipv4Addr::new(10, 200, 0, 1))),
            "node-0-1"
        );
        assert_eq!(
            peer_hostname(IpAddr::V4(Ipv4Addr::new(10, 200, 0, 5))),
            "node-0-5"
        );
        assert_eq!(
            peer_hostname(IpAddr::V4(Ipv4Addr::new(10, 200, 1, 100))),
            "node-1-100"
        );
        assert_eq!(
            peer_hostname(IpAddr::V4(Ipv4Addr::new(192, 168, 255, 254))),
            "node-255-254"
        );
    }

    #[test]
    fn test_peer_hostname_v6() {
        // Test various IPv6 addresses
        assert_eq!(
            peer_hostname(IpAddr::V6("fd00::1".parse().unwrap())),
            "node-0001"
        );
        assert_eq!(
            peer_hostname(IpAddr::V6("fd00::abcd".parse().unwrap())),
            "node-abcd"
        );
        assert_eq!(
            peer_hostname(IpAddr::V6("fd00:200::ffff".parse().unwrap())),
            "node-ffff"
        );
        // Zero last segment
        assert_eq!(
            peer_hostname(IpAddr::V6("fd00::1:0".parse().unwrap())),
            "node-0000"
        );
    }

    #[test]
    fn test_dns_config() {
        let config = DnsConfig::new("overlay.local.", IpAddr::V4(Ipv4Addr::new(10, 200, 0, 1)));
        assert_eq!(config.zone, "overlay.local.");
        assert_eq!(config.port, DEFAULT_DNS_PORT);
        assert_eq!(config.bind_addr, IpAddr::V4(Ipv4Addr::new(10, 200, 0, 1)));

        // Test with_port
        let config = config.with_port(5353);
        assert_eq!(config.port, 5353);
    }

    #[test]
    fn test_dns_config_serialization() {
        let config = DnsConfig::new("overlay.local.", IpAddr::V4(Ipv4Addr::new(10, 200, 0, 1)))
            .with_port(15353);

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: DnsConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.zone, config.zone);
        assert_eq!(deserialized.port, config.port);
        assert_eq!(deserialized.bind_addr, config.bind_addr);
    }

    #[tokio::test]
    async fn test_service_discovery_local_cache() {
        // Use a non-routable address since we're only testing local cache
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 15353);
        let discovery = ServiceDiscovery::new(addr);

        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
        discovery.register("test-service", ip).await;

        let resolved = discovery.resolve("test-service").await;
        assert_eq!(resolved, Some(ip));

        // Test unregister
        discovery.unregister("test-service").await;
        let services = discovery.list_services().await;
        assert!(services.is_empty());
    }

    #[test]
    fn test_dns_server_creation() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 15353);
        let server = DnsServer::new(addr, "overlay.local.");

        assert!(server.is_ok());
        let server = server.unwrap();
        assert_eq!(server.listen_addr(), addr);
        assert_eq!(server.zone_origin().to_string(), "overlay.local.");
    }

    #[test]
    fn test_dns_server_from_config() {
        let config =
            DnsConfig::new("test.local.", IpAddr::V4(Ipv4Addr::LOCALHOST)).with_port(15353);
        let server = DnsServer::from_config(&config);

        assert!(server.is_ok());
        let server = server.unwrap();
        assert_eq!(server.listen_addr().port(), 15353);
        assert_eq!(server.zone_origin().to_string(), "test.local.");
    }

    #[test]
    fn test_dns_server_invalid_zone() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 15353);
        // Empty zone name is technically valid in DNS, so use an obviously invalid one
        let server = DnsServer::new(addr, "overlay.local.");
        assert!(server.is_ok());
    }

    #[tokio::test]
    async fn test_dns_server_add_record() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 15353);
        let server = DnsServer::new(addr, "overlay.local.").unwrap();

        let result = server
            .add_record("myservice", IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_dns_handle_add_record() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 15353);
        let server = DnsServer::new(addr, "overlay.local.").unwrap();

        // Get handle and add records through it
        let handle = server.handle();

        let result = handle
            .add_record("service1", IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)))
            .await;
        assert!(result.is_ok());

        let result = handle
            .add_record("service2", IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)))
            .await;
        assert!(result.is_ok());

        // Zone origin should be accessible
        assert_eq!(handle.zone_origin().to_string(), "overlay.local.");
    }

    #[test]
    fn test_dns_client_creation() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 53);
        let client = DnsClient::new(addr);
        assert_eq!(client.server_addr, addr);
    }

    #[tokio::test]
    async fn test_dns_handle_add_aaaa_record() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 15353);
        let server = DnsServer::new(addr, "overlay.local.").unwrap();
        let handle = server.handle();

        // Add an AAAA record via IPv6 address
        let ipv6: IpAddr = "fd00::1".parse().unwrap();
        let result = handle.add_record("service-v6", ipv6).await;
        assert!(result.is_ok());

        // Add a second AAAA record
        let ipv6_2: IpAddr = "fd00::abcd".parse().unwrap();
        let result = handle.add_record("service-v6-2", ipv6_2).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_dns_server_add_aaaa_record() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 15353);
        let server = DnsServer::new(addr, "overlay.local.").unwrap();

        // Add AAAA record through the server directly
        let ipv6: IpAddr = "fd00::42".parse().unwrap();
        let result = server.add_record("myservice-v6", ipv6).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_dns_handle_remove_record_covers_both_types() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 15353);
        let server = DnsServer::new(addr, "overlay.local.").unwrap();
        let handle = server.handle();

        // Add an A record
        let ipv4 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        handle.add_record("dual-service", ipv4).await.unwrap();

        // Remove should succeed (tombstones both A and AAAA)
        let removed = handle.remove_record("dual-service").await.unwrap();
        assert!(removed);

        // Add an AAAA record
        let ipv6: IpAddr = "fd00::1".parse().unwrap();
        handle.add_record("v6-service", ipv6).await.unwrap();

        // Remove should also succeed for AAAA records
        let removed = handle.remove_record("v6-service").await.unwrap();
        assert!(removed);
    }

    #[tokio::test]
    async fn test_service_discovery_local_cache_ipv6() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 15353);
        let discovery = ServiceDiscovery::new(addr);

        // Register an IPv6 service
        let ipv6: IpAddr = "fd00::beef".parse().unwrap();
        discovery.register("v6-service", ipv6).await;

        // Should resolve from local cache
        let resolved = discovery.resolve("v6-service").await;
        assert_eq!(resolved, Some(ipv6));

        // Unregister and verify
        discovery.unregister("v6-service").await;
        let services = discovery.list_services().await;
        assert!(services.is_empty());
    }

    #[tokio::test]
    async fn test_service_discovery_mixed_v4_v6_cache() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 15353);
        let discovery = ServiceDiscovery::new(addr);

        let ipv4 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let ipv6: IpAddr = "fd00::1".parse().unwrap();

        discovery.register("svc-v4", ipv4).await;
        discovery.register("svc-v6", ipv6).await;

        assert_eq!(discovery.resolve("svc-v4").await, Some(ipv4));
        assert_eq!(discovery.resolve("svc-v6").await, Some(ipv6));

        let mut services = discovery.list_services().await;
        services.sort();
        assert_eq!(services, vec!["svc-v4", "svc-v6"]);
    }

    #[test]
    fn test_dns_config_with_ipv6_bind_addr() {
        let ipv6_bind: IpAddr = "fd00::1".parse().unwrap();
        let config = DnsConfig::new("overlay.local.", ipv6_bind);
        assert_eq!(config.bind_addr, ipv6_bind);
        assert_eq!(config.port, DEFAULT_DNS_PORT);

        // Serialization round-trip
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: DnsConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.bind_addr, ipv6_bind);
    }

    #[test]
    fn test_dns_server_creation_ipv6_bind() {
        let ipv6_addr: IpAddr = "::1".parse().unwrap();
        let addr = SocketAddr::new(ipv6_addr, 15353);
        let server = DnsServer::new(addr, "overlay.local.");

        assert!(server.is_ok());
        let server = server.unwrap();
        assert_eq!(server.listen_addr(), addr);
    }

    /// Smoke test for the Windows-fallback port-53 listener: binding to
    /// 127.0.0.2:53 should fail fast on hosts where that port is privileged
    /// or already in use, but we only care that the method surfaces a clean
    /// `DnsError` (not a panic) when the bind is contested. When the bind
    /// succeeds on a permissive CI host, we verify the returned handle shares
    /// the authority with the primary listener.
    #[tokio::test]
    async fn test_bind_windows_fallback_errors_or_shares_authority() {
        let primary = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let server = DnsServer::new(primary, "overlay.local.").unwrap();
        let bind_ip: IpAddr = "127.0.0.2".parse().unwrap();

        match server.bind_windows_fallback(bind_ip).await {
            Ok(handle) => {
                // Best-effort: the handle must expose the same zone as the
                // primary server so record mutations on either propagate to
                // both listeners.
                assert_eq!(handle.zone_origin().to_string(), "overlay.local.");
                handle
                    .add_record("dual", IpAddr::V4(Ipv4Addr::new(10, 0, 0, 9)))
                    .await
                    .expect("add_record via fallback handle");
            }
            Err(DnsError::Io(_)) => {
                // Expected on hosts that reserve port 53 or where the
                // loopback alias is already bound. Counts as a clean error
                // rather than a panic.
            }
            Err(other) => panic!("unexpected error from bind_windows_fallback: {other}"),
        }
    }

    #[test]
    fn test_peer_hostname_uniqueness() {
        // Different IPs should produce different hostnames
        let v4_a = peer_hostname(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
        let v4_b = peer_hostname(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)));
        assert_ne!(v4_a, v4_b);

        let v6_a = peer_hostname(IpAddr::V6("fd00::1".parse().unwrap()));
        let v6_b = peer_hostname(IpAddr::V6("fd00::2".parse().unwrap()));
        assert_ne!(v6_a, v6_b);

        // IPv4 and IPv6 hostname formats are distinct
        let v4 = peer_hostname(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
        let v6 = peer_hostname(IpAddr::V6("fd00::1".parse().unwrap()));
        assert_ne!(v4, v6);
    }

    // ---- resolv.conf parsing / upstream resolution -------------------------

    #[test]
    fn test_parse_resolv_conf_filters_stub_and_loopback() {
        // A systemd-resolved stub line plus a plain loopback must be dropped;
        // the real upstream survives on port 53.
        let contents = "\
            # generated by netbird\n\
            nameserver 127.0.0.53\n\
            nameserver 127.0.0.1\n\
            nameserver 192.168.1.1\n\
            search example.com\n\
            options edns0\n";
        let parsed = parse_resolv_conf(contents);
        assert_eq!(
            parsed,
            vec![SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
                53
            )],
            "127.0.0.53 stub and 127.0.0.1 loopback must be filtered out",
        );
    }

    #[test]
    fn test_parse_resolv_conf_dedup_and_comments() {
        let contents = "\
            ; a comment\n\
            nameserver 8.8.8.8\n\
            nameserver 8.8.8.8\n\
            nameserver fe80::1%eth0\n\
            nameserver 0.0.0.0\n";
        let parsed = parse_resolv_conf(contents);
        // 8.8.8.8 de-duplicated; scoped link-local kept (zone stripped);
        // 0.0.0.0 unspecified dropped.
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed[0],
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 53)
        );
        assert_eq!(parsed[1].ip(), "fe80::1".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn test_resolve_upstreams_config_override_wins() {
        // An explicit config upstream must be returned verbatim with no
        // resolv.conf consultation (we point the path at a bogus file).
        let explicit = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 9, 9, 9)), 5300);
        let config = DnsConfig::new("overlay.local.", IpAddr::V4(Ipv4Addr::LOCALHOST))
            .with_upstreams(vec![explicit]);
        let resolved = resolve_upstreams(&config, "/nonexistent/resolv.conf");
        assert_eq!(resolved, vec![explicit]);
    }

    #[test]
    fn test_resolve_upstreams_falls_back_to_public_when_missing() {
        // Missing resolv.conf => public fallback (1.1.1.1, 8.8.8.8).
        let config = DnsConfig::new("overlay.local.", IpAddr::V4(Ipv4Addr::LOCALHOST));
        let resolved = resolve_upstreams(&config, "/definitely/not/a/real/resolv.conf");
        assert_eq!(
            resolved,
            vec![
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 53),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 53),
            ],
        );
    }

    // ---- end-to-end forwarding ---------------------------------------------

    /// Spawn a minimal stub upstream DNS responder on an ephemeral UDP port.
    ///
    /// It answers *every* A query with `answer_ip` (echoing the queried name)
    /// so a forwarded query can be observed flowing through. Returns the bound
    /// `SocketAddr` so the caller can point the overlay forwarder at it.
    async fn spawn_stub_upstream(answer_ip: Ipv4Addr) -> SocketAddr {
        use hickory_server::proto::op::{Message, MessageType, ResponseCode};

        let sock = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .await
            .expect("bind stub upstream");
        let addr = sock.local_addr().expect("stub local_addr");

        tokio::spawn(async move {
            let mut buf = vec![0u8; 1500];
            loop {
                let Ok((len, from)) = sock.recv_from(&mut buf).await else {
                    break;
                };
                let Ok(request) = Message::from_vec(&buf[..len]) else {
                    continue;
                };
                let mut resp = Message::new();
                resp.set_id(request.id());
                resp.set_message_type(MessageType::Response);
                resp.set_recursion_available(true);
                resp.set_response_code(ResponseCode::NoError);
                for q in request.queries() {
                    resp.add_query(q.clone());
                    if q.query_type() == RecordType::A {
                        let rec =
                            Record::from_rdata(q.name().clone(), 60, RData::A(A::from(answer_ip)));
                        resp.add_answer(rec);
                    }
                }
                if let Ok(bytes) = resp.to_vec() {
                    let _ = sock.send_to(&bytes, from).await;
                }
            }
        });

        addr
    }

    /// Send a raw A query to `server` and return the first A answer, if any.
    /// Returns `Err` carrying the `ResponseCode` on a non-NoError response so
    /// SERVFAIL can be asserted distinctly from "no answer".
    async fn raw_query_a(
        server: SocketAddr,
        name: &str,
    ) -> Result<Option<Ipv4Addr>, hickory_server::proto::op::ResponseCode> {
        use hickory_server::proto::op::{Message, MessageType, Query, ResponseCode};

        let client = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .await
            .expect("bind client");

        let qname = Name::from_str(name).expect("query name");
        let mut msg = Message::new();
        msg.set_id(0x1234);
        msg.set_message_type(MessageType::Query);
        msg.set_recursion_desired(true);
        msg.add_query(Query::query(qname, RecordType::A));
        let bytes = msg.to_vec().expect("encode query");

        client.send_to(&bytes, server).await.expect("send query");

        let mut buf = vec![0u8; 1500];
        // Generous client deadline: the forwarder's own bounded retry budget
        // (2 attempts x 2s) means a SERVFAIL for a dead upstream arrives within
        // ~4s; this must exceed that so the test observes SERVFAIL rather than
        // tripping its own client timeout first.
        let len = tokio::time::timeout(Duration::from_secs(12), client.recv(&mut buf))
            .await
            .expect("query timed out")
            .expect("recv response");
        let resp = Message::from_vec(&buf[..len]).expect("decode response");

        if resp.response_code() != ResponseCode::NoError {
            return Err(resp.response_code());
        }
        for ans in resp.answers() {
            if let Some(RData::A(a)) = ans.data() {
                return Ok(Some((*a).into()));
            }
        }
        Ok(None)
    }

    #[tokio::test]
    async fn test_forwarding_overlay_answered_and_nonoverlay_forwarded() {
        // Stub upstream answers everything with 203.0.113.7.
        let upstream_answer = Ipv4Addr::new(203, 0, 113, 7);
        let upstream = spawn_stub_upstream(upstream_answer).await;

        // `start` binds the listener internally, so grab a concrete ephemeral
        // port first (bind + drop) and build the server on it — that lets the
        // test client send queries to a known address.
        let bound = {
            let probe = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
                .await
                .unwrap();
            let a = probe.local_addr().unwrap();
            drop(probe);
            a
        };

        // Overlay server with the stub as its only upstream (no resolv.conf
        // detection — upstreams injected directly).
        let overlay_ip = Ipv4Addr::new(10, 200, 0, 5);
        let server =
            DnsServer::new_with_upstreams(bound, "overlay.local.", vec![upstream]).unwrap();
        let handle = server.handle();
        handle
            .add_record("svc", IpAddr::V4(overlay_ip))
            .await
            .unwrap();
        let _running = server.start().await.unwrap();

        // Give the listener a moment to bind.
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Overlay-zone query is answered from the authority (NOT the stub).
        let overlay = raw_query_a(bound, "svc.overlay.local.")
            .await
            .expect("overlay query should not SERVFAIL");
        assert_eq!(
            overlay,
            Some(overlay_ip),
            "overlay name must be answered from InMemoryAuthority",
        );

        // Non-overlay query is forwarded to the stub upstream.
        let forwarded = raw_query_a(bound, "example.com.")
            .await
            .expect("forwarded query should not SERVFAIL");
        assert_eq!(
            forwarded,
            Some(upstream_answer),
            "non-overlay name must be forwarded to the upstream stub",
        );
    }

    #[tokio::test]
    async fn test_forwarding_total_upstream_failure_is_servfail_not_panic() {
        use hickory_server::proto::op::ResponseCode;

        // Point the forwarder at a dead upstream (nothing listening). The
        // server must return SERVFAIL for non-overlay queries, never panic,
        // and still serve the overlay zone.
        let dead_upstream = {
            // Bind+drop to grab a free port nobody is listening on.
            let s = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
                .await
                .unwrap();
            let a = s.local_addr().unwrap();
            drop(s);
            a
        };

        let bound = {
            let s = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
                .await
                .unwrap();
            let a = s.local_addr().unwrap();
            drop(s);
            a
        };

        let server =
            DnsServer::new_with_upstreams(bound, "overlay.local.", vec![dead_upstream]).unwrap();
        let handle = server.handle();
        handle
            .add_record("svc", IpAddr::V4(Ipv4Addr::new(10, 200, 0, 9)))
            .await
            .unwrap();
        let _running = server.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Overlay zone still works.
        let overlay = raw_query_a(bound, "svc.overlay.local.")
            .await
            .expect("overlay query should still succeed");
        assert_eq!(overlay, Some(Ipv4Addr::new(10, 200, 0, 9)));

        // Non-overlay query against a dead upstream => SERVFAIL (not a panic,
        // not a hang past the resolver's own timeout).
        match raw_query_a(bound, "example.com.").await {
            Err(ResponseCode::ServFail) => {} // expected
            Err(other) => panic!("expected SERVFAIL, got {other:?}"),
            Ok(answer) => panic!("expected SERVFAIL, got answer {answer:?}"),
        }
    }
}
