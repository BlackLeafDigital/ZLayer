//! Enumerate live compute systems. Used at agent startup to discover
//! zombie compute systems from a previous agent run that must be
//! terminated before we proceed.

#![allow(clippy::missing_errors_doc)]

use serde::Deserialize;
use windows::core::HSTRING;
use windows::Win32::System::HostComputeSystem::HcsEnumerateComputeSystems;

use crate::error::HcsResult;
use crate::operation::run_operation;

/// Compact summary of an enumerated compute system. HCS returns this JSON
/// array (one entry per running / paused system) from `HcsEnumerateComputeSystems`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct EnumeratedSystem {
    /// Container / VM identifier (usually a GUID).
    pub id: String,
    /// HCS system type, e.g. `"Container"` or `"VirtualMachine"`.
    #[serde(default)]
    pub system_type: String,
    /// Caller-supplied owner tag from the compute-system configuration.
    #[serde(default)]
    pub owner: String,
    /// Runtime ID (present for some system types, absent for others).
    #[serde(default)]
    pub runtime_id: Option<String>,
    /// State string as reported by HCS (e.g. `"Created"`, `"Running"`, `"Paused"`).
    #[serde(default)]
    pub state: String,
}

/// Query filter for [`list`].
///
/// Each optional field narrows the result set; multiple fields are `ANDed`.
/// HCS accepts a JSON `PropertyQuery` document — we serialize this struct into
/// it. Defaults match "return all compute systems".
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct EnumerateQuery {
    /// Filter by owner string (e.g. the agent's owner tag). If `Some`, only
    /// systems matching this owner are returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owners: Option<Vec<String>>,
    /// Filter by `IDs` prefix match. Rarely used today.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<String>>,
}

/// List compute systems currently known to HCS.
///
/// `query` narrows the result; pass [`EnumerateQuery::default`] to return all.
///
/// Typical use at agent startup: filter by our `owner` tag ("zlayer") and
/// terminate any stragglers before starting fresh.
pub async fn list(query: &EnumerateQuery) -> HcsResult<Vec<EnumeratedSystem>> {
    let query_json = serde_json::to_string(query)?;
    let query_w = HSTRING::from(query_json);

    let result_json = run_operation(|op| {
        // SAFETY: `query_w` lives for the whole closure invocation; `op` is a
        // live HCS operation handle owned by `run_operation`. The call kicks
        // off asynchronously and the JSON result is delivered via the
        // operation's completion callback.
        match unsafe { HcsEnumerateComputeSystems(&query_w, op) } {
            Ok(()) => windows::core::HRESULT(0),
            Err(e) => e.code(),
        }
    })
    .await?;

    let trimmed = result_json.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Ok(Vec::new());
    }

    let systems: Vec<EnumeratedSystem> = serde_json::from_str(&result_json)?;
    Ok(systems)
}

/// Convenience: list compute systems owned by a specific tag. `ZLayer` tags
/// every compute system it creates with `owner = "zlayer"` so this finds
/// our zombies on startup.
pub async fn list_by_owner(owner: &str) -> HcsResult<Vec<EnumeratedSystem>> {
    list(&EnumerateQuery {
        owners: Some(vec![owner.to_string()]),
        ids: None,
    })
    .await
}
