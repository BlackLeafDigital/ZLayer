# zlayer-proxy

High-performance reverse proxy for ZLayer with TLS termination and L4/L7 routing.

## Features

- Host and path-based routing via `ServiceRegistry`
- Round-robin backend selection
- Health-aware backend selection for L4 streams
- HTTP/1.1 with upgrade (WebSocket) pass-through
- Forwarding headers (`X-Forwarded-For`, etc.)
- TLS termination with dynamic SNI certificate selection
- ACME (Let's Encrypt) automatic certificate provisioning
- L4 TCP/UDP stream proxying
- Network policy enforcement

## Modules

- `config`: Proxy configuration types
- `server`: HTTP/HTTPS proxy server
- `routes`: Route registry and matching
- `lb`: Load balancing strategies
- `tls`: TLS termination and certificate handling
- `sni_resolver`: Dynamic SNI certificate selection
- `acme`: ACME (Let's Encrypt) integration
- `stream`: L4 TCP/UDP stream proxying
- `tunnel`: Tunneled backend support
- `network_policy`: Network policy enforcement
- `service`: Service-level proxy orchestration

## License

Apache-2.0
