//! Parameters of the collision, duplicate and storage-history tools (RD-150-01, RD-150-02).

use rmcp::schemars;
use serde::Deserialize;

use crate::ApiError;

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CollisionPolicyParams {
    /// The category (id from list_configuration section categories) or the package (id from
    /// list_packages), depending on the tool.
    pub id: String,
    /// `rename`, `skip`, `overwrite`, `compare` or `ask`; absent removes the level's own policy
    /// so it inherits again.
    #[serde(default)]
    pub policy: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CollisionDecisionParams {
    /// The waiting download, as list_collision_prompts names it.
    pub id: String,
    /// `rename`, `skip` or `overwrite`.
    pub decision: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct DuplicateLookupParams {
    /// Addresses to look up (at most 500).
    pub urls: Vec<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct DedupeParams {
    /// The finished download whose file becomes a link.
    pub id: String,
    /// The finished download whose identical file stays.
    pub original_download_id: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct StorageOperationsParams {
    /// Newest rows to return (1-1000, default 100).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// A policy word, refused with the code the interface translates.
pub(crate) fn policy(value: Option<&str>) -> Result<Option<rd_core::CollisionPolicy>, ApiError> {
    value
        .map(|value| {
            rd_core::CollisionPolicy::parse(value).ok_or_else(|| {
                ApiError::bad_request(
                    "collision.policy_invalid",
                    "The policy must be rename, skip, overwrite, compare or ask",
                )
            })
        })
        .transpose()
}

/// A decision word, refused with the code the interface translates.
pub(crate) fn decision(value: &str) -> Result<rd_core::CollisionDecision, ApiError> {
    rd_core::CollisionDecision::parse(value).ok_or_else(|| {
        ApiError::bad_request(
            "collision.decision_invalid",
            "The decision must be rename, skip or overwrite",
        )
    })
}
