//! Context propagation utilities for distributed tracing
//!
//! Provides helpers for extracting and injecting trace context
//! in HTTP headers using W3C Trace Context standard.

use opentelemetry::propagation::{Extractor, Injector};
use std::collections::HashMap;

/// HTTP header extractor for incoming requests (works with http crate HeaderMap)
pub struct HeaderExtractor<'a, T>(pub &'a T);

impl Extractor for HeaderExtractor<'_, http::HeaderMap> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|k| k.as_str()).collect()
    }
}

impl Extractor for HeaderExtractor<'_, HashMap<String, String>> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(|s| s.as_str())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|s| s.as_str()).collect()
    }
}

/// HTTP header injector for outgoing requests
pub struct HeaderInjector<'a, T>(pub &'a mut T);

impl Injector for HeaderInjector<'_, http::HeaderMap> {
    fn set(&mut self, key: &str, value: String) {
        if let Ok(name) = http::header::HeaderName::from_bytes(key.as_bytes()) {
            if let Ok(val) = http::header::HeaderValue::from_str(&value) {
                self.0.insert(name, val);
            }
        }
    }
}

impl Injector for HeaderInjector<'_, HashMap<String, String>> {
    fn set(&mut self, key: &str, value: String) {
        self.0.insert(key.to_string(), value);
    }
}

/// Extract trace context from HTTP headers
///
/// Returns a Context that should be used as the parent for spans
/// handling this request.
#[cfg(feature = "propagation")]
pub fn extract_context_from_headers(headers: &http::HeaderMap) -> opentelemetry::Context {
    use opentelemetry::global;
    global::get_text_map_propagator(|propagator| propagator.extract(&HeaderExtractor(headers)))
}

/// Inject current trace context into HTTP headers
///
/// Used when making outgoing HTTP requests to propagate trace context.
#[cfg(feature = "propagation")]
pub fn inject_context_to_headers(headers: &mut http::HeaderMap) {
    use opentelemetry::global;
    global::get_text_map_propagator(|propagator| {
        propagator.inject(&mut HeaderInjector(headers));
    });
}

/// Extract trace context from a HashMap (useful for gRPC metadata)
#[cfg(feature = "propagation")]
pub fn extract_context_from_map(map: &HashMap<String, String>) -> opentelemetry::Context {
    use opentelemetry::global;
    global::get_text_map_propagator(|propagator| propagator.extract(&HeaderExtractor(map)))
}

/// Inject trace context into a HashMap
#[cfg(feature = "propagation")]
pub fn inject_context_to_map(map: &mut HashMap<String, String>) {
    use opentelemetry::global;
    global::get_text_map_propagator(|propagator| {
        propagator.inject(&mut HeaderInjector(map));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hashmap_extractor() {
        let mut map = HashMap::new();
        map.insert("traceparent".to_string(), "00-abc123-def456-01".to_string());

        let extractor = HeaderExtractor(&map);
        assert_eq!(extractor.get("traceparent"), Some("00-abc123-def456-01"));
        assert_eq!(extractor.get("nonexistent"), None);
    }

    #[test]
    fn test_hashmap_injector() {
        let mut map = HashMap::new();
        {
            let mut injector = HeaderInjector(&mut map);
            injector.set("traceparent", "00-abc123-def456-01".to_string());
        }
        assert_eq!(
            map.get("traceparent"),
            Some(&"00-abc123-def456-01".to_string())
        );
    }

    #[test]
    fn test_header_map_extractor() {
        let mut headers = http::HeaderMap::new();
        headers.insert("traceparent", "00-abc123-def456-01".parse().unwrap());

        let extractor = HeaderExtractor(&headers);
        assert_eq!(extractor.get("traceparent"), Some("00-abc123-def456-01"));
    }

    #[test]
    fn test_header_map_injector() {
        let mut headers = http::HeaderMap::new();
        {
            let mut injector = HeaderInjector(&mut headers);
            injector.set("traceparent", "00-abc123-def456-01".to_string());
        }
        assert_eq!(
            headers.get("traceparent").and_then(|v| v.to_str().ok()),
            Some("00-abc123-def456-01")
        );
    }
}
