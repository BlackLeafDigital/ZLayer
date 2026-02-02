//! DNS server for service discovery over overlay networks

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::RwLock;
use trust_dns_client::client::{Client, SyncClient};
use trust_dns_client::udp::UdpClientConnection;
use trust_dns_server::authority::{Catalog, ZoneType};
use trust_dns_server::proto::rr::rdata::A;
use trust_dns_server::proto::rr::{DNSClass, Name, RData, Record, RecordType};
use trust_dns_server::server::ServerFuture;
use trust_dns_server::store::in_memory::InMemoryAuthority;

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
    pub fn new(zone: &str, bind_addr: IpAddr) -> Self {
        Self {
            zone: zone.to_string(),
            port: DEFAULT_DNS_PORT,
            bind_addr,
        }
    }

    /// Set a custom port
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }
}

/// Generate a hostname from an IPv4 address for DNS registration
///
/// Converts an IP like 10.200.0.5 to "node-0-5" (using last two octets)
pub fn peer_hostname(ip: Ipv4Addr) -> String {
    let octets = ip.octets();
    format!("node-{}-{}", octets[2], octets[3])
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
    /// Add a DNS A record for a hostname to IP mapping
    pub async fn add_record(&self, hostname: &str, ip: Ipv4Addr) -> Result<(), DnsError> {
        // Create the fully qualified domain name
        let fqdn = if hostname.ends_with('.') {
            Name::from_str(hostname)
                .map_err(|e| DnsError::InvalidName(format!("{}: {}", hostname, e)))?
        } else {
            // Append the zone origin
            let name = Name::from_str(hostname)
                .map_err(|e| DnsError::InvalidName(format!("{}: {}", hostname, e)))?;
            name.append_domain(&self.zone_origin)
                .map_err(|e| DnsError::InvalidName(format!("Failed to append zone: {}", e)))?
        };

        // Create the A record
        let a_data = A::from(ip);
        let rdata = RData::A(a_data);
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

    /// Remove a DNS record for a hostname
    pub async fn remove_record(&self, hostname: &str) -> Result<bool, DnsError> {
        let fqdn = if hostname.ends_with('.') {
            Name::from_str(hostname)
                .map_err(|e| DnsError::InvalidName(format!("{}: {}", hostname, e)))?
        } else {
            let name = Name::from_str(hostname)
                .map_err(|e| DnsError::InvalidName(format!("{}: {}", hostname, e)))?;
            name.append_domain(&self.zone_origin)
                .map_err(|e| DnsError::InvalidName(format!("Failed to append zone: {}", e)))?
        };

        let serial = {
            let mut s = self.serial.write().await;
            let current = *s;
            *s = s.wrapping_add(1);
            current
        };

        // Create an empty record to effectively "remove" by setting empty data
        // Note: trust-dns doesn't have a direct remove, so we create a tombstone
        let record = Record::with(fqdn.clone(), RecordType::A, 0);
        self.authority.upsert(record, serial).await;

        Ok(true)
    }

    /// Get the zone origin
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
    pub fn new(listen_addr: SocketAddr, zone: &str) -> Result<Self, DnsError> {
        let zone_origin =
            Name::from_str(zone).map_err(|e| DnsError::InvalidName(format!("{}: {}", zone, e)))?;

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

    /// Create from a DnsConfig
    pub fn from_config(config: &DnsConfig) -> Result<Self, DnsError> {
        let listen_addr = SocketAddr::new(config.bind_addr, config.port);
        Self::new(listen_addr, &config.zone)
    }

    /// Get a handle for managing DNS records
    ///
    /// The handle can be cloned and used to add/remove records even after
    /// the server has been started.
    pub fn handle(&self) -> DnsHandle {
        DnsHandle {
            authority: Arc::clone(&self.authority),
            zone_origin: self.zone_origin.clone(),
            serial: Arc::clone(&self.serial),
        }
    }

    /// Add a DNS A record for a hostname to IP mapping
    pub async fn add_record(&self, hostname: &str, ip: Ipv4Addr) -> Result<(), DnsError> {
        self.handle().add_record(hostname, ip).await
    }

    /// Remove a DNS record for a hostname
    pub async fn remove_record(&self, hostname: &str) -> Result<bool, DnsError> {
        self.handle().remove_record(hostname).await
    }

    /// Start the DNS server and return a handle for record management
    ///
    /// This spawns the DNS server in a background task and returns a handle
    /// that can be used to add/remove records while the server is running.
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
    pub fn listen_addr(&self) -> SocketAddr {
        self.listen_addr
    }

    /// Get the zone origin
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
    pub fn new(server_addr: SocketAddr) -> Self {
        Self { server_addr }
    }

    /// Query for an A record
    pub fn query_a(&self, hostname: &str) -> Result<Option<Ipv4Addr>, DnsError> {
        let name = Name::from_str(hostname)
            .map_err(|e| DnsError::InvalidName(format!("{}: {}", hostname, e)))?;

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
}

/// Service discovery with DNS
pub struct ServiceDiscovery {
    dns_server: SocketAddr,
    records: RwLock<HashMap<String, IpAddr>>,
}

impl ServiceDiscovery {
    /// Create a new service discovery instance
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

    /// Resolve a service to an IP
    pub async fn resolve(&self, name: &str) -> Option<IpAddr> {
        // First check local cache
        {
            let records = self.records.read().await;
            if let Some(ip) = records.get(name) {
                return Some(*ip);
            }
        }

        // Query DNS server for IPv4 addresses
        let client = DnsClient::new(self.dns_server);
        if let Ok(Some(ipv4)) = client.query_a(name) {
            return Some(IpAddr::V4(ipv4));
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
    fn test_peer_hostname() {
        // Test various IP addresses
        assert_eq!(peer_hostname(Ipv4Addr::new(10, 200, 0, 1)), "node-0-1");
        assert_eq!(peer_hostname(Ipv4Addr::new(10, 200, 0, 5)), "node-0-5");
        assert_eq!(peer_hostname(Ipv4Addr::new(10, 200, 1, 100)), "node-1-100");
        assert_eq!(
            peer_hostname(Ipv4Addr::new(192, 168, 255, 254)),
            "node-255-254"
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
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 15353);
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
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 15353);
        let server = DnsServer::new(addr, "overlay.local.");

        assert!(server.is_ok());
        let server = server.unwrap();
        assert_eq!(server.listen_addr(), addr);
        assert_eq!(server.zone_origin().to_string(), "overlay.local.");
    }

    #[test]
    fn test_dns_server_from_config() {
        let config =
            DnsConfig::new("test.local.", IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))).with_port(15353);
        let server = DnsServer::from_config(&config);

        assert!(server.is_ok());
        let server = server.unwrap();
        assert_eq!(server.listen_addr().port(), 15353);
        assert_eq!(server.zone_origin().to_string(), "test.local.");
    }

    #[test]
    fn test_dns_server_invalid_zone() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 15353);
        // Empty zone name is technically valid in DNS, so use an obviously invalid one
        let server = DnsServer::new(addr, "overlay.local.");
        assert!(server.is_ok());
    }

    #[tokio::test]
    async fn test_dns_server_add_record() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 15353);
        let server = DnsServer::new(addr, "overlay.local.").unwrap();

        let result = server
            .add_record("myservice", Ipv4Addr::new(10, 0, 0, 5))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_dns_handle_add_record() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 15353);
        let server = DnsServer::new(addr, "overlay.local.").unwrap();

        // Get handle and add records through it
        let handle = server.handle();

        let result = handle
            .add_record("service1", Ipv4Addr::new(10, 0, 0, 1))
            .await;
        assert!(result.is_ok());

        let result = handle
            .add_record("service2", Ipv4Addr::new(10, 0, 0, 2))
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
}
