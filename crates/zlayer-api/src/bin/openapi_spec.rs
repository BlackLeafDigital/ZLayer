//! Dumps the `ZLayer` `OpenAPI` spec as JSON to stdout.
//!
//! Usage:
//!   cargo run -p zlayer-api --bin openapi-spec > openapi.json

use utoipa::OpenApi;
use zlayer_api::ApiDoc;

fn main() {
    let doc = ApiDoc::openapi();
    let json = doc
        .to_pretty_json()
        .expect("Failed to serialize OpenAPI spec to JSON");
    println!("{json}");
}
