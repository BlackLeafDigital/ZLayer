//! DNS server for service discovery over overlay networks

use hickory_client::client::{Client, SyncClient};
use hickory_client::udp::UdpClientConnection;
use hickory_server::authority::{Catalog, ZoneType};
use hickory_server::proto::rr::rdata::{A, AAAA};
use hickory_server::proto::rr::{DNSClass, Name, RData, Record, RecordType};
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

/// Configuration for DNS integration with overlay network
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsConfig {
    /// DNS zone (e.g., "overlay.local.")
    pub zone: String,
    /// DNS server port (default: 15353)
    pub port: u16,
    /// Bind address (default: overlay IP)
    pub bind_addr: IpAddr,
}

impl DnsConfig {
    /// Create a new DNS config with defaults
    #[must_use]
    pub fn new(zone: &str, bind_addr: IpAddr) -> Self {
        Self {
            zone: zone.to_string(),
            port: DEFAULT_DNS_PORT,
            bind_addr,
        }
    }

    /// Set a custom port
    #[must_use]
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
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
}

impl DnsServer {
    /// Create a new DNS server for the given zone
    ///
    /// # Errors
    ///
    /// Returns `DnsError::InvalidName` if the zone name is invalid.
    pub fn new(listen_addr: SocketAddr, zone: &str) -> Result<Self, DnsError> {
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
        })
    }

    /// Create from a `DnsConfig`
    ///
    /// # Errors
    ///
    /// Returns `DnsError::InvalidName` if the zone name is invalid.
    pub fn from_config(config: &DnsConfig) -> Result<Self, DnsError> {
        let listen_addr = SocketAddr::new(config.bind_addr, config.port);
        Self::new(listen_addr, &config.zone)
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

        // Spawn the server in a background task
        tokio::spawn(async move {
            if let Err(e) = Self::run_server(listen_addr, zone_origin, authority).await {
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

        tokio::spawn(async move {
            if let Err(e) = Self::run_server(listen_addr, zone_origin, authority).await {
                tracing::error!("DNS server error: {}", e);
            }
        });

        Ok(handle)
    }

    /// Internal method to run the DNS server
    async fn run_server(
        listen_addr: SocketAddr,
        zone_origin: Name,
        authority: Arc<InMemoryAuthority>,
    ) -> Result<(), DnsError> {
        // Create the catalog and add our authority
        let mut catalog = Catalog::new();

        // The catalog accepts Arc<dyn AuthorityObject> - InMemoryAuthority implements this
        catalog.upsert(zone_origin.into(), Box::new(authority));

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
}
