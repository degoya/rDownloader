//! REST contract for previewing and approving the replay of a captured request.
//!
//! Everything here is derived from the stored capture; nothing carries a credential value.
//! URLs are redacted before they leave the process, the body is described by its field
//! names only, and credentials appear as *categories* paired with the host they would go to.

use chrono::{DateTime, Utc};
use rd_core::{
    AuthMethod, AuthProfileId, CandidateId, CapturedBody, CapturedHeader, CredentialCategory,
    ReplayBlockReason, ReplayConsent, ReplayMethod,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The auth profile a replay would use, without any of its secrets.
#[derive(Debug, Serialize, ToSchema)]
pub struct AuthProfileSummary {
    pub id: AuthProfileId,
    pub name: String,
    pub method: AuthMethod,
    /// Host the profile's scope covers, i.e. where its credentials would be sent.
    pub scope_host: String,
    pub has_client_certificate: bool,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Exactly what a replay of this capture would send, and to where.
///
/// This is the payload the consent dialog renders, so it has to be complete enough that a
/// person can make an informed decision and contain nothing that would be harmful to show.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReplayPreviewResponse {
    pub candidate_id: CandidateId,
    /// Link URL with every signature and credential parameter redacted.
    pub url: String,
    /// Redacted URL the browser actually ended up at, when it differs.
    pub effective_url: Option<String>,
    /// Origin the request itself goes to.
    pub target_origin: String,
    /// Origins the replay may follow redirects into.
    pub approved_origins: Vec<String>,
    pub method: ReplayMethod,
    pub content_type: Option<String>,
    /// Body description: kind, size and field *names*. Never values.
    pub body: Option<CapturedBody>,
    pub headers: Vec<CapturedHeader>,
    /// Which categories of credential this replay would send.
    pub credential_categories: Vec<CredentialCategory>,
    pub auth_profile: Option<AuthProfileSummary>,
    pub expires_at: Option<DateTime<Utc>>,
    pub replayable: bool,
    pub blocked_reason: Option<ReplayBlockReason>,
    /// Hash the consent is bound to; the client sends it back when approving.
    pub template_hash: String,
    /// Consent already on file, if any.
    pub consent: Option<ReplayConsent>,
}

/// A person's approval of one specific captured request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ReplayConsentRequest {
    /// `template_hash` from the preview the person actually looked at.
    ///
    /// Approving a template that has since changed must fail rather than silently apply to
    /// the new one, so this is required rather than advisory.
    pub template_hash: String,
    /// Origins being approved; must be a subset of the preview's `approved_origins`.
    #[serde(default)]
    pub approved_origins: Vec<String>,
}
