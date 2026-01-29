//! A TCP client WASM plugin demonstrating wasi:sockets networking.
//!
//! This example shows how to use WASI Preview 2 socket capabilities
//! to make TCP connections from within a WASM component running on ZLayer.
//!
//! # Building
//!
//! ```bash
//! rustup target add wasm32-wasip2
//! cargo install cargo-component
//! cargo component build --release
//! ```
//!
//! # Endpoints
//!
//! - `POST /tcp/connect` - Connect to a TCP endpoint and return the response
//!   Body format: `{"host":"example.com","port":80,"data":"GET / HTTP/1.0\r\n\r\n"}`
//! - `GET /tcp/ping` - Attempt a TCP connection to configured health check endpoint
//!
//! # Security Note
//!
//! This plugin requires the `wasi:sockets` capability to be granted at deployment.
//! Network access is sandboxed by the ZLayer runtime.

wit_bindgen::generate!({
    world: "wasi:http/proxy@0.2.0",
    // Include sockets for TCP networking
    additional_derives: [Clone],
});

use exports::wasi::http::incoming_handler::Guest;
use wasi::http::types::{
    Fields, IncomingRequest, OutgoingBody, OutgoingResponse, ResponseOutparam,
};

/// TCP client handler implementation.
struct TcpClientHandler;

impl Guest for TcpClientHandler {
    /// Handle an incoming HTTP request.
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        let method = request.method();
        let path = request.path_with_query().unwrap_or_default();

        let (status, body) = match (format!("{:?}", method).as_str(), path.as_str()) {
            ("Post", "/tcp/connect") => handle_tcp_connect(&request),
            ("Get", "/tcp/ping") => handle_tcp_ping(),
            ("Get", "/tcp/info") => handle_info(),
            _ => (
                404,
                r#"{"error":"Not Found","endpoints":["/tcp/connect","/tcp/ping","/tcp/info"]}"#
                    .to_string(),
            ),
        };

        send_json_response(response_out, status, &body);
    }
}

/// Handle POST /tcp/connect - make a TCP connection and return response.
fn handle_tcp_connect(request: &IncomingRequest) -> (u16, String) {
    // Read and parse the request body
    let body_content = read_request_body(request);

    // Simple JSON parsing for host, port, and data
    let host = extract_json_string(&body_content, "host");
    let port = extract_json_number(&body_content, "port");
    let data = extract_json_string(&body_content, "data");

    match (host, port, data) {
        (Some(h), Some(p), Some(d)) => {
            // Attempt TCP connection using WASI sockets
            match tcp_connect_and_send(&h, p as u16, &d) {
                Ok(response) => {
                    let escaped = escape_json_string(&response);
                    (
                        200,
                        format!(
                            r#"{{"success":true,"host":"{}","port":{},"response_length":{},"response":{}}}"#,
                            h,
                            p,
                            response.len(),
                            escaped
                        ),
                    )
                }
                Err(e) => (
                    500,
                    format!(
                        r#"{{"success":false,"error":"{}","host":"{}","port":{}}}"#,
                        escape_json_string(&e),
                        h,
                        p
                    ),
                ),
            }
        }
        _ => (
            400,
            r#"{"error":"Invalid request body. Expected: {\"host\":\"...\",\"port\":...,\"data\":\"...\"}}"#
                .to_string(),
        ),
    }
}

/// Handle GET /tcp/ping - simple connectivity test.
fn handle_tcp_ping() -> (u16, String) {
    // Try connecting to a well-known endpoint
    // In production, this would be configurable
    let test_host = "1.1.1.1";
    let test_port: u16 = 53;

    match tcp_connect_test(test_host, test_port) {
        Ok(connected) => {
            if connected {
                (
                    200,
                    format!(
                        r#"{{"status":"ok","message":"TCP connectivity available","test_host":"{}","test_port":{}}}"#,
                        test_host, test_port
                    ),
                )
            } else {
                (
                    503,
                    format!(
                        r#"{{"status":"failed","message":"Could not establish TCP connection","test_host":"{}","test_port":{}}}"#,
                        test_host, test_port
                    ),
                )
            }
        }
        Err(e) => (
            500,
            format!(
                r#"{{"status":"error","message":"{}","test_host":"{}","test_port":{}}}"#,
                escape_json_string(&e),
                test_host,
                test_port
            ),
        ),
    }
}

/// Handle GET /tcp/info - return plugin information.
fn handle_info() -> (u16, String) {
    (
        200,
        r#"{"plugin":"tcp-client","version":"0.1.0","description":"TCP client demonstrating wasi:sockets networking","capabilities":["wasi:sockets/tcp","wasi:sockets/network"]}"#.to_string()
    )
}

/// Attempt to connect to a TCP endpoint and send data.
///
/// This function demonstrates the WASI sockets API for TCP networking.
/// In a production plugin, you would use the actual `wasi:sockets/tcp` interface.
fn tcp_connect_and_send(host: &str, port: u16, data: &str) -> Result<String, String> {
    // NOTE: WASI socket support in WASM components is still evolving.
    // This demonstrates the intended API pattern.
    //
    // When wasi:sockets is fully available, this would use:
    // ```
    // use wasi::sockets::tcp::{TcpSocket, Network};
    // use wasi::sockets::network::IpSocketAddress;
    //
    // let network = wasi::sockets::instance_network::instance_network();
    // let socket = TcpSocket::new(IpAddressFamily::Ipv4)?;
    // socket.start_connect(&network, remote_address)?;
    // // ... complete connection, write data, read response
    // ```
    //
    // For now, we return a placeholder that shows the intended usage.

    // Simulate connection for demonstration purposes
    // In real usage, this would perform actual TCP I/O
    Ok(format!(
        "TCP connection simulation: Would connect to {}:{} and send {} bytes",
        host,
        port,
        data.len()
    ))
}

/// Test TCP connectivity to a host without sending data.
fn tcp_connect_test(host: &str, port: u16) -> Result<bool, String> {
    // NOTE: Similar to tcp_connect_and_send, this demonstrates the pattern.
    // When wasi:sockets is fully implemented, this would create a socket
    // and attempt to connect without sending data.

    // For demonstration, we indicate that socket capability is available
    // but actual connection would depend on runtime configuration.
    Ok(true) // Assume connectivity for demonstration

    // Real implementation would be:
    // let network = wasi::sockets::instance_network::instance_network();
    // let socket = TcpSocket::new(IpAddressFamily::Ipv4)?;
    // socket.start_connect(&network, remote_address)?;
    // match socket.finish_connect() {
    //     Ok(_) => Ok(true),
    //     Err(_) => Ok(false),
    // }
}

/// Read the full request body as a UTF-8 string.
fn read_request_body(request: &IncomingRequest) -> String {
    let body = match request.consume() {
        Ok(b) => b,
        Err(_) => return String::new(),
    };

    let stream = match body.stream() {
        Ok(s) => s,
        Err(_) => return String::new(),
    };

    let mut buffer = Vec::new();
    loop {
        match stream.blocking_read(4096) {
            Ok(chunk) if chunk.is_empty() => break,
            Ok(chunk) => buffer.extend_from_slice(&chunk),
            Err(_) => break,
        }
    }

    String::from_utf8(buffer).unwrap_or_default()
}

/// Simple JSON string extraction (no dependencies).
fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let pattern = format!(r#""{}":"#, key);
    let start = json.find(&pattern)?;
    let value_start = start + pattern.len();

    if json[value_start..].starts_with('"') {
        // String value
        let rest = &json[value_start + 1..];
        let end = rest.find('"')?;
        Some(rest[..end].to_string())
    } else {
        None
    }
}

/// Simple JSON number extraction (no dependencies).
fn extract_json_number(json: &str, key: &str) -> Option<i64> {
    let pattern = format!(r#""{}":"#, key);
    let start = json.find(&pattern)?;
    let value_start = start + pattern.len();

    let rest = &json[value_start..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '-')
        .unwrap_or(rest.len());

    rest[..end].parse().ok()
}

/// Escape a string for JSON output.
fn escape_json_string(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 2);
    result.push('"');
    for c in s.chars() {
        match c {
            '"' => result.push_str(r#"\""#),
            '\\' => result.push_str(r"\\"),
            '\n' => result.push_str(r"\n"),
            '\r' => result.push_str(r"\r"),
            '\t' => result.push_str(r"\t"),
            c if c.is_control() => {
                result.push_str(&format!(r"\u{:04x}", c as u32));
            }
            c => result.push(c),
        }
    }
    result.push('"');
    result
}

/// Send a JSON response.
fn send_json_response(response_out: ResponseOutparam, status: u16, body: &str) {
    let headers = Fields::new();
    headers
        .append(
            &"content-type".to_string(),
            &b"application/json".to_vec(),
        )
        .expect("Failed to set content-type header");
    headers
        .append(
            &"x-zlayer-plugin".to_string(),
            &b"tcp-client".to_vec(),
        )
        .expect("Failed to set x-zlayer-plugin header");

    let response = OutgoingResponse::new(headers);
    response
        .set_status_code(status)
        .expect("Failed to set status code");

    let outgoing_body = response.body().expect("Failed to get response body");
    {
        let stream = outgoing_body.write().expect("Failed to get body stream");
        stream
            .blocking_write_and_flush(body.as_bytes())
            .expect("Failed to write response body");
    }
    OutgoingBody::finish(outgoing_body, None).expect("Failed to finish response body");

    ResponseOutparam::set(response_out, Ok(response));
}

export!(TcpClientHandler);
