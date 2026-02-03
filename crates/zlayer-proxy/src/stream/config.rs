//! Configuration types for stream (L4) proxy

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Configuration for a TCP listener
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TcpListenerConfig {
    /// Listen port (proxy binds to 0.0.0.0:{port})
    pub port: u16,

    /// Protocol hint for metrics/logging (e.g., "postgresql", "mongodb", "minecraft")
    #[serde(default)]
    pub protocol_hint: Option<String>,

    /// Enable TLS termination (auto-provision cert)
    #[serde(default)]
    pub tls: bool,

    /// Enable PROXY protocol for passing client IP to backend
    #[serde(default)]
    pub proxy_protocol: bool,
}

/// Configuration for a UDP listener
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UdpListenerConfig {
    /// Listen port
    pub port: u16,

    /// Protocol hint for metrics/logging (e.g., "source-engine", "game-generic")
    #[serde(default)]
    pub protocol_hint: Option<String>,

    /// Session timeout override (default uses global udp_session_timeout)
    #[serde(default, with = "optional_duration_serde")]
    pub session_timeout: Option<Duration>,
}

/// Default UDP session timeout (60 seconds)
pub const DEFAULT_UDP_SESSION_TIMEOUT: Duration = Duration::from_secs(60);

/// Serde helper for optional Duration serialization
mod optional_duration_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(value: &Option<Duration>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(d) => d.as_secs().serialize(serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<u64> = Option::deserialize(deserializer)?;
        Ok(opt.map(Duration::from_secs))
    }
}
