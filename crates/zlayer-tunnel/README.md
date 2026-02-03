# zlayer-tunnel

Secure tunneling for ZLayer services with token-based authentication, WebSocket control channels, and automatic reconnection.

## Features

- **Token-Based Authentication** - Secure tunnel establishment with SHA256-hashed tokens
- **TCP/UDP Tunneling** - Protocol-agnostic service exposure through tunnels
- **WebSocket Control Channel** - Binary protocol over WebSocket for coordination and heartbeats
- **Automatic Reconnection** - Exponential backoff retry logic for resilient connections
- **Node-to-Node Tunneling** - Connect ZLayer nodes across different networks/datacenters
- **On-Demand Access** - Cloudflared-style temporary local access to remote services with TTL
- **Session Management** - Track active connections, bytes transferred, and session expiration

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
zlayer-tunnel = "0.8"
```

## Quick Start

### Server Setup

```rust
use std::sync::Arc;
use zlayer_tunnel::{TunnelRegistry, TunnelServerConfig, ControlHandler, accept_all_tokens};

// Create the registry with port range for service assignment
let registry = Arc::new(TunnelRegistry::new((30000, 31000)));

// Configure the server
let config = TunnelServerConfig::default();

// Create handler with token validator
let validator = Arc::new(accept_all_tokens);
let handler = ControlHandler::new(registry.clone(), config, validator);

// Handle incoming WebSocket connections
async fn handle_connection(
    handler: &ControlHandler,
    stream: tokio::net::TcpStream,
    addr: std::net::SocketAddr,
) {
    if let Err(e) = handler.handle_connection(stream, addr).await {
        tracing::error!("Connection error: {}", e);
    }
}
```

### Client Setup

```rust
use std::sync::Arc;
use zlayer_tunnel::{TunnelAgent, TunnelClientConfig, ServiceConfig};

// Configure the client
let config = TunnelClientConfig::new(
    "wss://tunnel.example.com/tunnel/v1",
    "my-auth-token"
)
.with_service(ServiceConfig::tcp("ssh", 22).with_remote_port(2222))
.with_service(ServiceConfig::tcp("postgres", 5432));

// Create the agent with connection callback
let agent = TunnelAgent::new(config)
    .on_connection(Arc::new(|service_id, conn_id, client_addr| {
        println!("Connection {} to service {} from {}", conn_id, service_id, client_addr);
        true // Accept the connection
    }));

// Run with automatic reconnection
agent.run().await?;
```

### Node-to-Node Tunneling

```rust
use zlayer_tunnel::{NodeTunnelManager, NodeTunnel, TunnelServerConfig};

// Create manager for this node
let config = TunnelServerConfig::default();
let manager = NodeTunnelManager::new("node-us-east", config);

// Define a tunnel from this node to another
let tunnel = NodeTunnel::new("db-tunnel", "node-us-east", "node-eu-west")
    .with_ports(5432, 5432)
    .with_token("tun_secret_token");

manager.add_tunnel(tunnel)?;

// Start outbound connection
manager.start_outbound("db-tunnel", "wss://eu-west.example.com/tunnel/v1".to_string())?;

// Check tunnel status
if let Some(status) = manager.get_status("db-tunnel") {
    println!("Tunnel state: {:?}", status.state);
}
```

### On-Demand Access

```rust
use std::time::Duration;
use zlayer_tunnel::AccessManager;

// Create access manager with default 1-hour TTL
let manager = AccessManager::new()
    .with_default_ttl(Duration::from_secs(3600));

// Start a session to access a remote service locally
let session = manager.start_session(
    "internal-api".to_string(),      // Endpoint name
    "10.0.0.50:8080".to_string(),    // Remote address
    Some(8080),                       // Local port (None = auto-assign)
    None,                             // TTL (None = use default)
).await?;

println!("Access via: http://127.0.0.1:{}", session.local_addr.port());

// List active sessions
for info in manager.list_sessions() {
    println!("{}: {} -> {} ({}s remaining)",
        info.endpoint,
        info.local_addr,
        info.remote_addr,
        info.remaining.map(|d| d.as_secs()).unwrap_or(0)
    );
}

// Clean up expired sessions
manager.cleanup_expired();
```

## Protocol Overview

The tunnel uses a compact binary message format over WebSocket:

```text
+----------+----------+----------------------------------+
| Type(1)  | Len(4)   | Payload (variable)               |
+----------+----------+----------------------------------+
```

### Message Types

| Type | Code | Direction | Description |
|------|------|-----------|-------------|
| Auth | 0x01 | C->S | Client authentication request |
| AuthOk | 0x02 | S->C | Authentication success with tunnel ID |
| AuthFail | 0x03 | S->C | Authentication failure with reason |
| Register | 0x10 | C->S | Service registration request |
| RegisterOk | 0x11 | S->C | Registration success with service ID |
| RegisterFail | 0x12 | S->C | Registration failure |
| Connect | 0x20 | S->C | Incoming connection notification |
| ConnectAck | 0x21 | C->S | Connection accepted |
| ConnectFail | 0x22 | C->S | Connection rejected |
| Heartbeat | 0x30 | Bidirectional | Keepalive with timestamp |
| HeartbeatAck | 0x31 | Bidirectional | Heartbeat response |
| Unregister | 0x40 | C->S | Service unregistration |
| Disconnect | 0x41 | S->C | Server disconnect notification |

### Connection Flow

1. **Client connects** via WebSocket to server's control path
2. **Authentication**: Client sends `Auth` with token, server responds `AuthOk`/`AuthFail`
3. **Service registration**: Client sends `Register` for each service, server assigns ports
4. **Heartbeat loop**: Server sends periodic `Heartbeat`, client responds with `HeartbeatAck`
5. **Connection handling**: Server sends `Connect` on incoming connections, client acknowledges

## API Reference

### Core Types

| Type | Description |
|------|-------------|
| `TunnelRegistry` | Server-side registry for active tunnels and services |
| `ControlHandler` | WebSocket control channel handler |
| `TunnelAgent` | Client-side tunnel agent with auto-reconnect |
| `NodeTunnelManager` | Manager for node-to-node tunnel connections |
| `AccessManager` | On-demand access session manager |

### Configuration

| Type | Description |
|------|-------------|
| `TunnelServerConfig` | Server settings (port range, heartbeat, limits) |
| `TunnelClientConfig` | Client settings (URL, token, services) |
| `ServiceConfig` | Service definition (name, protocol, ports) |

### Constants

| Constant | Value | Description |
|----------|-------|-------------|
| `PROTOCOL_VERSION` | 1 | Current protocol version |
| `MAX_MESSAGE_SIZE` | 65536 | Maximum message size (64KB) |
| `HEADER_SIZE` | 5 | Message header size in bytes |

## License

Apache-2.0 - See [LICENSE](../../LICENSE) for details.
