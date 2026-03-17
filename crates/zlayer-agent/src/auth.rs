use jsonwebtoken::{encode, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: u64,
    iat: u64,
    iss: String,
    roles: Vec<String>,
}

/// Mint a scoped JWT for a container.
///
/// # Errors
///
/// Returns an error string if the system clock is unavailable or JWT encoding fails.
pub fn mint_container_token(
    secret: &str,
    service_name: &str,
    container_id: &str,
    ttl: Duration,
) -> Result<String, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?;
    let claims = Claims {
        sub: format!("container:{service_name}:{container_id}"),
        iat: now.as_secs(),
        exp: (now + ttl).as_secs(),
        iss: "zlayer".to_string(),
        roles: vec!["container".to_string()],
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| e.to_string())
}
