//! Wire types for the Groups page.
//!
//! Mirror of `zlayer_api::storage::StoredUserGroup` and the daemon's
//! `GroupMembersResponse`. Defined locally so the hydrate/WASM build
//! doesn't need `zlayer-api` (which is SSR-only).
//!
//! Field shape was cross-checked against
//! `crates/zlayer-api/src/storage/mod.rs` (`StoredUserGroup`) and
//! `crates/zlayer-api/src/handlers/groups.rs`
//! (`CreateGroupRequest`, `UpdateGroupRequest`, `AddMemberRequest`,
//! `GroupMembersResponse`). The daemon serialises `DateTime<Utc>` as
//! RFC-3339, so receiving those here as `String` round-trips cleanly.
//!
//! NOTE: the daemon DOES expose a `PATCH /api/v1/groups/{id}` endpoint,
//! so `WireUpdateGroup` is supported. The list-members response is a
//! bare `{ group_id, members: [user_id, ...] }` — names/emails must be
//! joined client-side from `manager_list_users`.

use serde::{Deserialize, Serialize};

/// A stored user group returned by `/api/v1/groups`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireUserGroup {
    /// UUID identifier.
    pub id: String,
    /// Group name (unique).
    pub name: String,
    /// Free-form description.
    #[serde(default)]
    pub description: Option<String>,
    /// RFC-3339 creation timestamp.
    pub created_at: String,
    /// RFC-3339 last-updated timestamp.
    pub updated_at: String,
}

/// Response body for `GET /api/v1/groups/{id}/members` — just a list of
/// user ids. The page joins these against `manager_list_users` to render
/// usernames/emails.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireGroupMembers {
    /// Group id the members belong to.
    pub group_id: String,
    /// User ids that are members of the group.
    pub members: Vec<String>,
}

/// Request body for `POST /api/v1/groups`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WireCreateGroup {
    /// Group name (required, non-empty).
    pub name: String,
    /// Free-form description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Request body for `PATCH /api/v1/groups/{id}`. All fields optional.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WireUpdateGroup {
    /// New name, or leave unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// New description, or leave unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Request body for `POST /api/v1/groups/{id}/members`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireAddMember {
    /// User id to add to the group.
    pub user_id: String,
}
