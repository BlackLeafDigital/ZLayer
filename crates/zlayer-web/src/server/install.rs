//! Install-script + latest-binary endpoints for <https://zlayer.dev>.
//!
//! `GET /install`            UA-sniff → return install.sh / install.ps1 / install.py body
//! `GET /install.sh|ps1|py`  serve the matching script verbatim
//! `GET /latest-{slug}`      302 redirect to the latest GitHub release asset
//!
//! The slug map intentionally accepts both the CI's canonical names
//! (`linux-amd64`, `darwin-arm64`, `windows-amd64`) and friendlier aliases
//! (`macos-silicon`, `macos-intel`, `windows`, `linux`, `macos`).

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use axum::extract::Path;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use tokio::sync::Mutex;
use tracing::warn;

const GITHUB_REPO: &str = "BlackLeafDigital/ZLayer";
const VERSION_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const REQUEST_USER_AGENT: &str = "zlayer-web/install-redirect (+https://zlayer.dev)";

const INSTALL_SH: &str = include_str!("../../../../install.sh");
const INSTALL_PS1: &str = include_str!("../../../../install.ps1");
const INSTALL_PY: &str = include_str!("../../../../install.py");

static VERSION_CACHE: OnceLock<Mutex<Option<(Instant, String)>>> = OnceLock::new();

#[derive(serde::Deserialize)]
struct GhRelease {
    tag_name: String,
}

async fn latest_version() -> Result<String, String> {
    let cache = VERSION_CACHE.get_or_init(|| Mutex::new(None));
    {
        let guard = cache.lock().await;
        if let Some((at, v)) = guard.as_ref() {
            if at.elapsed() < VERSION_CACHE_TTL {
                return Ok(v.clone());
            }
        }
    }

    let client = reqwest::Client::builder()
        .user_agent(REQUEST_USER_AGENT)
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| format!("build http client: {e}"))?;

    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    let release: GhRelease = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("github api request: {e}"))?
        .error_for_status()
        .map_err(|e| format!("github api status: {e}"))?
        .json()
        .await
        .map_err(|e| format!("github api decode: {e}"))?;

    let version = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name)
        .to_string();

    let mut guard = cache.lock().await;
    *guard = Some((Instant::now(), version.clone()));
    Ok(version)
}

fn script_response(body: &'static str, content_type: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        body,
    )
        .into_response()
}

pub async fn install_sh() -> Response {
    script_response(INSTALL_SH, "application/x-sh; charset=utf-8")
}

pub async fn install_ps1() -> Response {
    script_response(INSTALL_PS1, "application/x-powershell; charset=utf-8")
}

pub async fn install_py() -> Response {
    script_response(INSTALL_PY, "text/x-python; charset=utf-8")
}

/// UA-sniffing entry point. `PowerShell` user-agents get install.ps1, the
/// Python urllib UA gets install.py, everything else (curl, wget, browsers)
/// gets install.sh. Mirrors the same logic as Homebrew's /install endpoint.
pub async fn install_detect(headers: HeaderMap) -> Response {
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    if ua.contains("powershell") || ua.contains("windowspowershell") {
        install_ps1().await
    } else if ua.contains("python") {
        install_py().await
    } else {
        install_sh().await
    }
}

/// Map a friendly URL slug to the (os, arch, ext) tuple GitHub's release
/// asset name uses. The CI publishes assets named
/// `zlayer-{VERSION}-{os}-{arch}.{ext}` (see `.github/workflows/release.yml`),
/// so the redirect target is deterministic once we know the latest version.
fn resolve_slug(slug: &str) -> Option<(&'static str, &'static str, &'static str)> {
    match slug.to_ascii_lowercase().as_str() {
        "linux-amd64" | "linux-x64" | "linux-x86_64" | "linux" => {
            Some(("linux", "amd64", "tar.gz"))
        }
        "linux-arm64" | "linux-aarch64" => Some(("linux", "arm64", "tar.gz")),
        "darwin-amd64" | "macos-amd64" | "macos-intel" | "macos-x64" | "macos-x86_64" => {
            Some(("darwin", "amd64", "tar.gz"))
        }
        "darwin-arm64"
        | "macos-arm64"
        | "macos-aarch64"
        | "macos-silicon"
        | "macos-apple-silicon"
        | "macos" => Some(("darwin", "arm64", "tar.gz")),
        "windows-amd64" | "windows-x64" | "windows-x86_64" | "windows" => {
            Some(("windows", "amd64", "zip"))
        }
        _ => None,
    }
}

pub async fn latest_redirect(Path(slug): Path<String>) -> Response {
    let Some((os, arch, ext)) = resolve_slug(&slug) else {
        return (
            StatusCode::NOT_FOUND,
            format!(
                "unknown platform slug: {slug}\nsupported: linux-amd64, linux-arm64, \
                 darwin-amd64, darwin-arm64, windows-amd64 \
                 (aliases: macos-silicon, macos-intel, windows, linux, macos)\n"
            ),
        )
            .into_response();
    };

    let version = match latest_version().await {
        Ok(v) => v,
        Err(e) => {
            warn!("latest_version lookup failed: {e}");
            return (
                StatusCode::BAD_GATEWAY,
                format!("could not resolve latest ZLayer release: {e}\n"),
            )
                .into_response();
        }
    };

    let target = format!(
        "https://github.com/{GITHUB_REPO}/releases/download/v{version}/\
         zlayer-{version}-{os}-{arch}.{ext}"
    );
    Redirect::temporary(&target).into_response()
}
