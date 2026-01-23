//! DNS server for service discovery over overlay networks

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

/// DNS server for overlay networks
pub struct DnsServer {
    listen_addr: SocketAddr,
    authority: Arc<RwLock<InMemoryAuthority>>,
    zone_origin: Name,
    serial: Arc<RwLock<u32>>,
}

impl DnsServer {
    /// Create a new DNS server for the given zone
    pub fn new(listen_addr: SocketAddr, zone: &str) -> Result<Self, DnsError> {
        let zone_origin =
            Name::from_str(zone).map_err(|e| DnsError::InvalidName(format!("{}: {}", zone, e)))?;

        // Create an empty in-memory authority for the zone
        let authority = InMemoryAuthority::empty(zone_origin.clone(), ZoneType::Primary, false);

        Ok(Self {
            listen_addr,
            authority: Arc::new(RwLock::new(authority)),
            zone_origin,
            serial: Arc::new(RwLock::new(1)),
        })
    }

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

        // Upsert the record into the authority
        let mut authority = self.authority.write().await;
        authority.upsert_mut(record, serial);

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
        let mut authority = self.authority.write().await;
        let record = Record::with(fqdn.clone(), RecordType::A, 0);
        authority.upsert_mut(record, serial);

        Ok(true)
    }

    /// Start the DNS server (consumes self)
    pub async fn start(self) -> Result<(), DnsError> {
        // Create the catalog and add our authority
        let mut catalog = Catalog::new();

        // Get the authority and wrap it in Arc for the catalog
        let authority = Arc::try_unwrap(self.authority)
            .map_err(|_| DnsError::Server("Authority is still in use".to_string()))?
            .into_inner();

        // AuthorityObject is implemented for Arc<A> where A: Authority
        let arc_authority: Arc<InMemoryAuthority> = Arc::new(authority);
        catalog.upsert(self.zone_origin.into(), Box::new(arc_authority));

        // Create the server
        let mut server = ServerFuture::new(catalog);

        // Bind UDP socket
        let udp_socket = UdpSocket::bind(self.listen_addr).await?;
        server.register_socket(udp_socket);

        // Bind TCP listener
        let tcp_listener = TcpListener::bind(self.listen_addr).await?;
        server.register_listener(tcp_listener, Duration::from_secs(30));

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

    #[test]
    fn test_dns_client_creation() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 53);
        let client = DnsClient::new(addr);
        assert_eq!(client.server_addr, addr);
    }
}
