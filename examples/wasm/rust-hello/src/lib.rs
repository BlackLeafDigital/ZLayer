//! A minimal Rust WASM example for ZLayer.
//!
//! This example demonstrates the simplest possible Rust WebAssembly module
//! that can run on ZLayer. It uses WASI Preview 2 for I/O.
//!
//! # Building
//!
//! ```bash
//! rustup target add wasm32-wasip2
//! cargo install cargo-component
//! cargo component build --release
//! ```
//!
//! The compiled WASM component will be at:
//! `target/wasm32-wasip2/release/rust_hello_wasm.wasm`

#![no_std]
extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

/// A simple greeter that formats welcome messages.
pub struct Greeter {
    name: String,
}

impl Greeter {
    /// Create a new greeter with the given name.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
        }
    }

    /// Generate a greeting message.
    #[must_use]
    pub fn greet(&self) -> String {
        alloc::format!("Hello from Rust WASM, {}!", self.name)
    }

    /// Generate a farewell message.
    #[must_use]
    pub fn farewell(&self) -> String {
        alloc::format!("Goodbye, {}!", self.name)
    }
}

/// Process a list of names and return greetings for each.
#[must_use]
pub fn greet_many(names: &[&str]) -> Vec<String> {
    names
        .iter()
        .map(|name| Greeter::new(name).greet())
        .collect()
}

/// Entry point for the WASM module.
///
/// This is called when the module is executed as a command.
/// For library usage, import the functions directly.
#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    // In a real application, this would read input from WASI
    // and produce output. For this minimal example, we just
    // demonstrate the structure.
    let greeter = Greeter::new("ZLayer");
    let _message = greeter.greet();

    // With WASI Preview 2, you would use the wasi:cli/stdout
    // interface to print output. This minimal example just
    // demonstrates the module structure.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_greeter() {
        let greeter = Greeter::new("World");
        assert_eq!(greeter.greet(), "Hello from Rust WASM, World!");
        assert_eq!(greeter.farewell(), "Goodbye, World!");
    }

    #[test]
    fn test_greet_many() {
        let names = vec!["Alice", "Bob"];
        let greetings = greet_many(&names);
        assert_eq!(greetings.len(), 2);
        assert_eq!(greetings[0], "Hello from Rust WASM, Alice!");
        assert_eq!(greetings[1], "Hello from Rust WASM, Bob!");
    }
}
