//! Network management API DTOs.

use serde::{Deserialize, Serialize};
#[cfg(feature = "utoipa")]
use utoipa::ToSchema;

use crate::spec::NetworkPolicySpec;

/// Summary returned when listing networks.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
pub struct NetworkSummary {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub cidr_count: usize,
    pub member_count: usize,
    pub rule_count: usize,
}

impl From<&NetworkPolicySpec> for NetworkSummary {
    fn from(spec: &NetworkPolicySpec) -> Self {
        Self {
            name: spec.name.clone(),
            description: spec.description.clone(),
            cidr_count: spec.cidrs.len(),
            member_count: spec.members.len(),
            rule_count: spec.access_rules.len(),
        }
    }
}
