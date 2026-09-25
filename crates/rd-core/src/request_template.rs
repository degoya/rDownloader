//! Reproducible replay of an authenticated browser request.
//!
//! [`crate::CapturedRequest`] is the wire contract the capture clients speak. This module
//! adds what turning such a capture into a *repeatable* transfer needs: a bounded POST body,
//! the origins the replay may talk to, an expiry, an explicit replayability marker, and the
//! consent a person gave before any of it is sent anywhere.
//!
//! Two boundaries are structural rather than a matter of discipline:
//!
//! * The body's ciphertext reference lives only in a SQL column, never in a struct here, so
//!   it cannot be serialized into an API or SSE payload by accident.
//! * [`CapturedBody`] carries field *names* only. Values never leave the secret store.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;
use utoipa::ToSchema;

use crate::{AuthMethod, AuthProfile, CapturedRequest};

/// Version of the stored template shape, so an older row stays recognisable.
pub const REQUEST_TEMPLATE_VERSION: u32 = 1;

/// Largest decoded body kept for replay.
///
/// Browser download triggers are form posts and small JSON documents. Anything larger is a
/// file upload in disguise, which the ticket puts out of scope.
pub const MAX_REPLAY_BODY_BYTES: usize = 64 * 1024;

/// Largest base64 payload accepted on the wire, so an oversize body is refused before it is
/// decoded rather than after.
pub const MAX_REPLAY_BODY_B64: usize = 128 * 1024;

/// Largest number of origins one replay may be approved for.
pub const MAX_APPROVED_ORIGINS: usize = 4;

/// Largest number of form field names kept for the preview.
pub const MAX_BODY_FIELD_NAMES: usize = 32;

/// HTTP method a replay may use.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "UPPERCASE")]
pub enum ReplayMethod {
    #[default]
    Get,
    Post,
}

impl ReplayMethod {
    /// Parses the method of a captured request; anything else is unsupported.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_uppercase().as_str() {
            "GET" => Some(Self::Get),
            "POST" => Some(Self::Post),
            _ => None,
        }
    }

    /// Canonical uppercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// Body shapes a replay can reproduce safely.
///
/// Deliberately excludes `multipart/form-data`: reproducing it means reproducing a file
/// part, which is out of scope.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReplayBodyKind {
    FormUrlencoded,
    Json,
    Text,
}

impl ReplayBodyKind {
    /// Maps a `Content-Type` essence onto a supported body kind.
    #[must_use]
    pub fn from_content_type(content_type: &str) -> Option<Self> {
        let essence = content_type
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        match essence.as_str() {
            "application/x-www-form-urlencoded" => Some(Self::FormUrlencoded),
            "application/json" => Some(Self::Json),
            "text/plain" => Some(Self::Text),
            _ => None,
        }
    }
}

/// Why a captured request cannot be replayed.
///
/// Every variant has a translated explanation in the web UI: a capture that cannot be
/// reproduced must say why, not disappear.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReplayBlockReason {
    /// Method other than GET or POST.
    MethodUnsupported,
    /// Decoded body above [`MAX_REPLAY_BODY_BYTES`].
    BodyTooLarge,
    /// The request carried a file part.
    FileUpload,
    /// `multipart/form-data`, which would mean reproducing a file part.
    MultipartUnsupported,
    /// A body whose `Content-Type` is none of the reproducible ones.
    ContentTypeUnsupported,
    /// The client could not read the body before it was streamed away.
    StreamedBody,
    /// A POST whose body was lost before hand-off.
    BodyMissing,
    /// The signed URL's deadline has passed and no refresh source is available.
    Expired,
    /// The transfer was redirected outside the approved origins.
    OriginNotApproved,
    /// Nothing can renew this URL: no resolver claims it and re-requesting failed.
    RefreshUnavailable,
    /// A person withdrew consent.
    ConsentRevoked,
    /// The server speaks an older capture contract than the client.
    ContractDowngrade,
    /// The endpoint ignored `Range`, so the partial cannot be continued.
    PostResumeUnsupported,
}

/// A category of credential a replay would send. Never a value.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialCategory {
    Cookies,
    BasicAuth,
    BearerToken,
    ClientCertificate,
    SignedQuery,
    FormFields,
}

/// Metadata of a captured request body.
///
/// Carries no secret reference and no values: the ciphertext reference lives in a SQL
/// column so it can never be serialized, and only field *names* are kept for the preview.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct CapturedBody {
    pub kind: ReplayBodyKind,
    /// Normalized `Content-Type` essence, e.g. `application/x-www-form-urlencoded`.
    pub content_type: String,
    /// Length of the decoded body in bytes.
    pub byte_len: u32,
    /// SHA-256 of the decoded bytes; consent identity and change detection.
    pub sha256: String,
    /// Form field names only, capped at [`MAX_BODY_FIELD_NAMES`].
    #[serde(default)]
    pub field_names: Vec<String>,
    /// Whether the body was actually stored. `false` when it was refused (too large, a file
    /// upload); the metadata is kept anyway so the UI can explain the block.
    #[serde(default)]
    pub stored: bool,
}

/// A person's explicit approval of one replay, bound to the template it was given for.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct ReplayConsent {
    pub granted_at: DateTime<Utc>,
    /// [`stable_hash`] of the template at the moment consent was granted.
    pub template_hash: String,
    /// Origins the person approved, always a subset of the server-derived set.
    pub approved_origins: Vec<String>,
}

/// The consented template of one download: what a person approved, as captured.
///
/// It records the *capture*, never a later renewal of it. A pre-resume refresh replaces the
/// transfer address for the attempt it was fetched in and nothing writes it back here
/// (RD-108-17); see `rd_scheduler::replay::Refreshed` for why. The fields that the removed
/// write-back would have kept — a `refreshed_at` and a `refresh_count` — are gone with it.
/// Older rows may still carry them in `template_json`; serde ignores them on read, and the
/// refresh budget that is actually enforced lives on the `downloads` row
/// (`replay_refresh_count`, `replay_refreshed_at`, migration 0027).
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct RequestTemplate {
    pub version: u32,
    pub request: CapturedRequest,
    pub consent: ReplayConsent,
}

/// What a download row exposes about its replay without carrying the template itself.
///
/// `DownloadFile` is serialized into REST responses and SSE payloads, so it gets this
/// summary while the full template stays behind an explicit database read.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct ReplaySummary {
    pub method: ReplayMethod,
    pub has_body: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub replayable: bool,
    pub blocked_reason: Option<ReplayBlockReason>,
}

/// `scheme://host[:port]`, lowercased, with the scheme's default port dropped.
///
/// This is the unit the replay origin allowlist is expressed in; a bare host would let a
/// downgrade to plain HTTP through, and a full URL would make every signed link its own
/// origin.
#[must_use]
pub fn origin_of(url: &Url) -> Option<String> {
    let scheme = url.scheme().to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    match url.port() {
        // `Url::port` already returns `None` for the scheme's default port.
        Some(port) => Some(format!("{scheme}://{host}:{port}")),
        None => Some(format!("{scheme}://{host}")),
    }
}

/// The origins a capture may legitimately be replayed against: the link URL and, when it
/// differs, the URL the browser actually ended up at.
#[must_use]
pub fn derive_approved_origins(url: &Url, effective_url: Option<&Url>) -> Vec<String> {
    let mut origins = Vec::new();
    for candidate in [Some(url), effective_url].into_iter().flatten() {
        if let Some(origin) = origin_of(candidate)
            && !origins.contains(&origin)
        {
            origins.push(origin);
        }
    }
    origins.truncate(MAX_APPROVED_ORIGINS);
    origins
}

/// Hash of everything a person consented to that must not silently change.
///
/// Covers method, body identity, content type, the approved origin set and the auth profile
/// the credentials come from. It deliberately **excludes** the signature query parameters:
/// a CDN re-signing the same object yields a different URL for identical content, and
/// forcing re-consent on every refresh would make the feature unusable.
#[must_use]
pub fn stable_hash(url: &Url, request: &CapturedRequest, auth_profile: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(REQUEST_TEMPLATE_VERSION.to_le_bytes());
    hasher.update(request.method.trim().to_ascii_uppercase().as_bytes());
    hasher.update([0]);

    // The path identifies the object; the query is where the signature lives.
    for candidate in [Some(url), request.effective_url.as_ref()]
        .into_iter()
        .flatten()
    {
        hasher.update(origin_of(candidate).unwrap_or_default().as_bytes());
        hasher.update(candidate.path().as_bytes());
        hasher.update([0]);
    }

    let mut origins = request.approved_origins.clone();
    origins.sort();
    for origin in &origins {
        hasher.update(origin.as_bytes());
        hasher.update([0]);
    }

    if let Some(body) = &request.body {
        hasher.update(body.content_type.as_bytes());
        hasher.update([0]);
        hasher.update(body.sha256.as_bytes());
        hasher.update([0]);
    }
    hasher.update(auth_profile.unwrap_or_default().as_bytes());
    hex::encode(hasher.finalize())
}

/// The credential categories a replay of this request would actually send.
///
/// Drives the consent modal, which has to state which category goes to which domain, so
/// this must be derived from the same data the transfer uses — never hand-maintained.
#[must_use]
pub fn credential_categories(
    url: &Url,
    request: &CapturedRequest,
    profile: Option<&AuthProfile>,
) -> Vec<CredentialCategory> {
    let mut categories = Vec::new();
    if let Some(profile) = profile {
        categories.push(match profile.method {
            AuthMethod::Cookies => CredentialCategory::Cookies,
            AuthMethod::Basic => CredentialCategory::BasicAuth,
            AuthMethod::Bearer => CredentialCategory::BearerToken,
        });
        if profile.has_client_certificate {
            categories.push(CredentialCategory::ClientCertificate);
        }
    }
    let signed = [Some(url), request.effective_url.as_ref()]
        .into_iter()
        .flatten()
        .any(crate::is_signed_url);
    if signed {
        categories.push(CredentialCategory::SignedQuery);
    }
    if request.body.is_some() {
        categories.push(CredentialCategory::FormFields);
    }
    categories.sort_unstable();
    categories.dedup();
    categories
}

/// Whether replaying this request would send something the browser sent.
///
/// A plain captured GET needs no consent, which keeps the behaviour introduced with browser
/// interception exactly as it was.
#[must_use]
pub fn needs_consent(request: &CapturedRequest) -> bool {
    !matches!(
        ReplayMethod::parse(&request.method),
        Some(ReplayMethod::Get)
    ) || request.body.is_some()
        || request.expires_at.is_some()
}

#[cfg(test)]
mod tests {
    use super::{
        CapturedBody, ReplayBodyKind, ReplayMethod, derive_approved_origins, needs_consent,
        origin_of, stable_hash,
    };
    use crate::CapturedRequest;
    use url::Url;

    fn url(input: &str) -> Url {
        input.parse().expect("url")
    }

    /// A captured request plus the link URL it belongs to, which lives on the candidate
    /// rather than inside `CapturedRequest`.
    fn request(method: &str, source: &str) -> (Url, CapturedRequest) {
        (
            url(source),
            CapturedRequest {
                method: method.to_owned(),
                ..CapturedRequest::default()
            },
        )
    }

    #[test]
    fn origins_drop_default_ports_and_keep_explicit_ones() {
        assert_eq!(
            origin_of(&url("https://Example.COM:443/f")).as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            origin_of(&url("https://example.com:8443/f")).as_deref(),
            Some("https://example.com:8443")
        );
        // A scheme downgrade must be a different origin, not the same one.
        assert_ne!(
            origin_of(&url("http://example.com/f")),
            origin_of(&url("https://example.com/f"))
        );
        assert_eq!(origin_of(&url("ftp://example.com/f")), None);
    }

    #[test]
    fn approved_origins_cover_the_link_and_the_redirect_target() {
        let origins = derive_approved_origins(
            &url("https://hoster.example/dl/1"),
            Some(&url("https://cdn.example.net/f.bin?sig=a")),
        );
        assert_eq!(
            origins,
            ["https://hoster.example", "https://cdn.example.net"]
        );
        // The same origin twice collapses.
        let single = derive_approved_origins(
            &url("https://hoster.example/dl/1"),
            Some(&url("https://hoster.example/dl/1/file")),
        );
        assert_eq!(single, ["https://hoster.example"]);
    }

    #[test]
    fn a_resigned_url_keeps_its_consent_hash() {
        // The whole point: a CDN re-signing the same object must not force re-consent.
        let (first_url, mut first) =
            request("GET", "https://cdn.example/f.bin?X-Amz-Signature=aaa");
        first.approved_origins = vec!["https://cdn.example".to_owned()];
        let (second_url, mut second) =
            request("GET", "https://cdn.example/f.bin?X-Amz-Signature=bbb");
        second.approved_origins = vec!["https://cdn.example".to_owned()];
        assert_eq!(
            stable_hash(&first_url, &first, None),
            stable_hash(&second_url, &second, None)
        );
    }

    #[test]
    fn changing_what_was_consented_to_changes_the_hash() {
        let (base_url, base) = request("POST", "https://cdn.example/f.bin");
        let baseline = stable_hash(&base_url, &base, None);

        let mut other_method = base.clone();
        other_method.method = "GET".to_owned();
        assert_ne!(stable_hash(&base_url, &other_method, None), baseline);

        assert_ne!(
            stable_hash(&url("https://cdn.example/other.bin"), &base, None),
            baseline
        );

        let mut other_body = base.clone();
        other_body.body = Some(CapturedBody {
            kind: ReplayBodyKind::FormUrlencoded,
            content_type: "application/x-www-form-urlencoded".to_owned(),
            byte_len: 9,
            sha256: "abc".to_owned(),
            field_names: vec!["id".to_owned()],
            stored: true,
        });
        assert_ne!(stable_hash(&base_url, &other_body, None), baseline);

        // A different auth profile sends different credentials to the same place.
        assert_ne!(stable_hash(&base_url, &base, Some("profile-1")), baseline);
    }

    #[test]
    fn only_replays_that_send_something_need_consent() {
        assert!(!needs_consent(
            &request("GET", "https://cdn.example/f.bin").1
        ));
        assert!(needs_consent(
            &request("POST", "https://cdn.example/f.bin").1
        ));

        let (_, mut expiring) = request("GET", "https://cdn.example/f.bin");
        expiring.expires_at = Some(chrono::Utc::now());
        assert!(needs_consent(&expiring));
    }

    #[test]
    fn body_kinds_come_from_the_content_type_essence() {
        assert_eq!(
            ReplayBodyKind::from_content_type("application/json; charset=utf-8"),
            Some(ReplayBodyKind::Json)
        );
        assert_eq!(
            ReplayBodyKind::from_content_type("APPLICATION/X-WWW-FORM-URLENCODED"),
            Some(ReplayBodyKind::FormUrlencoded)
        );
        // Reproducing a multipart body means reproducing a file part.
        assert_eq!(
            ReplayBodyKind::from_content_type("multipart/form-data; boundary=x"),
            None
        );
    }

    #[test]
    fn methods_outside_the_supported_set_are_rejected() {
        assert_eq!(ReplayMethod::parse("get"), Some(ReplayMethod::Get));
        assert_eq!(ReplayMethod::parse(" post "), Some(ReplayMethod::Post));
        assert_eq!(ReplayMethod::parse("PUT"), None);
        assert_eq!(ReplayMethod::parse("DELETE"), None);
    }
}
