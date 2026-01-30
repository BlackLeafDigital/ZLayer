//! A simple HTTP handler WASM plugin for ZLayer.
//!
//! This example demonstrates implementing the `wasi:http/incoming-handler` interface
//! to create a serverless HTTP handler that runs in ZLayer's WASM runtime.
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
//! - `GET /` - Returns a welcome message with request info
//! - `GET /health` - Health check endpoint
//! - `POST /echo` - Echoes back the request body as JSON
//! - Any other path - Returns 404 Not Found

wit_bindgen::generate!({
    world: "wasi:http/proxy@0.2.0",
});

use exports::wasi::http::incoming_handler::Guest;
use wasi::http::types::{
    Fields, IncomingRequest, OutgoingBody, OutgoingResponse, ResponseOutparam,
};

/// The HTTP handler implementation for ZLayer.
struct HelloHandler;

impl Guest for HelloHandler {
    /// Handle an incoming HTTP request.
    ///
    /// This function is called by the ZLayer runtime for each incoming request
    /// that matches the plugin's routing configuration.
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        // Extract request information
        let method = request.method();
        let path = request.path_with_query().unwrap_or_default();

        // Route based on path
        let (status, content_type, body) = match (format!("{:?}", method).as_str(), path.as_str())
        {
            ("Get", "/") | ("Get", "") => {
                let json = format!(
                    r#"{{"message":"Hello from ZLayer WASM!","method":"GET","path":"/","runtime":"wasi-preview2"}}"#
                );
                (200, "application/json", json)
            }
            ("Get", "/health") => {
                let json = r#"{"status":"healthy","plugin":"hello-handler"}"#.to_string();
                (200, "application/json", json)
            }
            ("Post", "/echo") => {
                // Read the request body
                let body_content = read_request_body(&request);
                let json = format!(
                    r#"{{"echoed":true,"length":{},"content":{}}}"#,
                    body_content.len(),
                    escape_json_string(&body_content)
                );
                (200, "application/json", json)
            }
            (method_str, p) => {
                let json = format!(
                    r#"{{"error":"Not Found","method":"{}","path":"{}"}}"#,
                    method_str, p
                );
                (404, "application/json", json)
            }
        };

        // Build response headers
        let headers = Fields::new();
        headers
            .append(&"content-type".to_string(), &content_type.as_bytes().to_vec())
            .expect("Failed to set content-type header");
        headers
            .append(
                &"x-zlayer-plugin".to_string(),
                &b"hello-handler".to_vec(),
            )
            .expect("Failed to set x-zlayer-plugin header");

        // Create the response
        let response = OutgoingResponse::new(headers);
        response
            .set_status_code(status)
            .expect("Failed to set status code");

        // Write the response body
        let outgoing_body = response.body().expect("Failed to get response body");
        {
            let stream = outgoing_body.write().expect("Failed to get body stream");
            stream
                .blocking_write_and_flush(body.as_bytes())
                .expect("Failed to write response body");
            // Stream must be dropped before finishing the body
        }
        OutgoingBody::finish(outgoing_body, None).expect("Failed to finish response body");

        // Send the response
        ResponseOutparam::set(response_out, Ok(response));
    }
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

export!(HelloHandler);
