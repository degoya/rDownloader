//! `POST /api/v1/capture/click-and-load`: Click'n'Load taken by the browser extension
//! (RD-1260-10).
//!
//! Without the desktop agent -- a service on a NAS, a machine the agent is not installed on --
//! nothing answers `127.0.0.1:9666`, and a hoster page shows no Click'n'Load button. The
//! extension takes the page's request in the browser instead and hands its fields here, under
//! its capture token. `jk` is never evaluated in the browser: it comes here as the page sent it,
//! and [`rd_cnl`] reads the key out of it or evaluates it in the sandbox the agent uses, then
//! decrypts, bounds the package name and the password and keeps the pace -- one rule for both.
//!
//! A refusal answers with a stable `cnl.*` code. The extension tells the person, never the page:
//! the page hears success either way, so the answer is no padding oracle against the AES path.

use std::time::Instant;

use axum::{Json, extract::State, http::StatusCode};
use rd_cnl::{Refusal, RefusalKind, pace};
use serde::Deserialize;
use utoipa::ToSchema;

use crate::{
    ApiError, AppState,
    dto::{CollectorIntakeRequest, CollectorIntakeResponse},
};

/// The largest request body: the agent's own limit for `/flash/addcrypted2`, which leaves room
/// for an 8 MiB payload as base64, the `jk` script and the other fields.
pub const BODY_LIMIT_BYTES: usize = 20 * 1024 * 1024;

/// The longest `jk`: a key, or a script that returns one.
pub const MAX_JK_BYTES: usize = 32 * 1024;

/// The longest list of links in the clear, the agent's limit for `/flash/add`.
pub(crate) const MAX_URLS_BYTES: usize = 1024 * 1024;

/// The longest `passwords` and `package` fields as sent, before the first line or the cut.
pub(crate) const MAX_FIELD_BYTES: usize = 64 * 1024;

/// The longest `source` address.
pub(crate) const MAX_SOURCE_BYTES: usize = 2048;

/// The label a batch carries when the page named no address of its own.
const FALLBACK_LABEL: &str = "Browser";

/// The stable codes of a refused hand-over.
pub mod code {
    /// The payload could not be read, or the request mixes the two shapes.
    pub const INVALID_PAYLOAD: &str = "cnl.invalid_payload";
    /// `jk` is missing, ambiguous, or the sandbox refused its script.
    pub(crate) const JK_REFUSED: &str = "cnl.jk_refused";
    /// The request named no link the LinkGrabber takes.
    pub const NO_LINKS: &str = "cnl.no_links";
    /// The payload or a field exceeds its limit.
    pub const TOO_LARGE: &str = "cnl.payload_too_large";
    /// More hand-overs than the service accepts per minute.
    pub const RATE_LIMITED: &str = "cnl.rate_limited";
}

/// The fields of a Click'n'Load request, under the names the page sent them.
///
/// Either `crypted` with `jk` (`/flash/addcrypted2`) or `urls` (`/flash/add`), never both. No
/// `Debug`: `passwords` must not reach a log line.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ClickAndLoadRequest {
    /// The encrypted links, base64: AES-128-CBC with the key as its IV. At most 8 MiB decoded.
    #[serde(default)]
    pub crypted: Option<String>,
    /// The key as the page sent it in `jk` (or `key`): 32 hexadecimal digits, or a script that
    /// returns them. Evaluated here in a bounded sandbox, never in the browser. At most 32 KiB.
    #[serde(default)]
    pub jk: Option<String>,
    /// The links in the clear, one per line. At most 1 MiB.
    #[serde(default)]
    pub urls: Option<String>,
    /// The archive passwords; the first non-empty line is kept, at most 1024 characters.
    #[serde(default)]
    #[schema(write_only)]
    pub passwords: Option<String>,
    /// The package name; cut at 200 characters.
    #[serde(default)]
    pub package: Option<String>,
    /// The page that offered the links, http or https; its host labels the batch.
    #[serde(default)]
    pub source: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/capture/click-and-load",
    tag = "capture",
    request_body = ClickAndLoadRequest,
    responses(
        (status = 201, body = CollectorIntakeResponse),
        (status = 400, description = "cnl.invalid_payload, cnl.jk_refused or cnl.no_links: the payload does not decrypt, the sandbox refused jk, or no link the LinkGrabber takes"),
        (status = 401, description = "Capture token missing or revoked"),
        (status = 413, description = "cnl.payload_too_large: the payload or a field exceeds its limit"),
        (status = 429, description = "cnl.rate_limited: more hand-overs than the service accepts per minute")
    )
)]
pub async fn capture_click_and_load(
    state: State<AppState>,
    audit: crate::audit::AuditContext,
    Json(request): Json<ClickAndLoadRequest>,
) -> Result<(StatusCode, Json<CollectorIntakeResponse>), ApiError> {
    let intake = read(request).await?;
    if !pace::SHARED.admit(Instant::now()) {
        return Err(refused(Refusal::new(
            RefusalKind::RateLimited,
            anyhow::anyhow!("more Click'n'Load hand-overs than the service accepts per minute"),
        )));
    }
    // The extension's route: it tells the person itself (RD-1270-05).
    rd_db::intake_notice::announced_by_extension(crate::collector_handlers::capture_intake(
        state,
        audit,
        Json(intake),
    ))
    .await
}

/// Checks every field against its limit, reads the links and builds the intake.
async fn read(request: ClickAndLoadRequest) -> Result<CollectorIntakeRequest, ApiError> {
    for (field, value, limit) in [
        ("jk", &request.jk, MAX_JK_BYTES),
        ("urls", &request.urls, MAX_URLS_BYTES),
        ("passwords", &request.passwords, MAX_FIELD_BYTES),
        ("package", &request.package, MAX_FIELD_BYTES),
        ("source", &request.source, MAX_SOURCE_BYTES),
    ] {
        if value.as_ref().is_some_and(|value| value.len() > limit) {
            return Err(refused(Refusal::new(
                RefusalKind::TooLarge,
                anyhow::anyhow!("Click'n'Load field {field} exceeds {limit} bytes"),
            )));
        }
    }
    let source_label = source_label(request.source.as_deref())?;
    let text = match (request.crypted, request.urls) {
        (Some(crypted), None) => {
            let jk = request.jk.ok_or_else(|| {
                refused(Refusal::new(
                    RefusalKind::InvalidKey,
                    anyhow::anyhow!("Click'n'Load request carries crypted without jk"),
                ))
            })?;
            rd_cnl::decrypt(&crypted, &jk).await.map_err(refused)?
        }
        (None, Some(urls)) => urls,
        (Some(_), Some(_)) => {
            return Err(refused(Refusal::new(
                RefusalKind::InvalidPayload,
                anyhow::anyhow!("Click'n'Load request carries both crypted and urls"),
            )));
        }
        (None, None) => {
            return Err(refused(Refusal::new(
                RefusalKind::NoLinks,
                anyhow::anyhow!("Click'n'Load request carries neither crypted nor urls"),
            )));
        }
    };
    let urls = rd_collector::extract_urls(&text);
    if urls.is_empty() {
        return Err(refused(Refusal::new(
            RefusalKind::NoLinks,
            anyhow::anyhow!("Click'n'Load payload contains no link the collector accepts"),
        )));
    }
    let password = pace::password(request.passwords.as_deref())
        .map_err(|length| {
            refused(Refusal::new(
                RefusalKind::TooLarge,
                anyhow::anyhow!("Click'n'Load password of {length} characters exceeds its limit"),
            ))
        })?
        .map(str::to_owned);
    Ok(CollectorIntakeRequest {
        text: Some(
            urls.iter()
                .map(url::Url::as_str)
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        source: rd_core::IngressSource::ClickAndLoad,
        source_label: Some(source_label),
        package_name: pace::package_name(request.package.as_deref()),
        password,
        links: Vec::new(),
    })
}

/// The host of the page that offered the links, or the fallback when it named none.
fn source_label(source: Option<&str>) -> Result<String, ApiError> {
    let Some(source) = source.map(str::trim).filter(|source| !source.is_empty()) else {
        return Ok(FALLBACK_LABEL.to_owned());
    };
    url::Url::parse(source)
        .ok()
        .filter(|address| matches!(address.scheme(), "http" | "https"))
        .and_then(|address| address.host_str().map(str::to_owned))
        .ok_or_else(|| {
            refused(Refusal::new(
                RefusalKind::InvalidPayload,
                anyhow::anyhow!("Click'n'Load source is not an http or https address"),
            ))
        })
}

/// The answer to a refusal: its stable code and a sentence that says nothing about the payload.
/// The account of it goes to the log only.
fn refused(refusal: Refusal) -> ApiError {
    tracing::info!(error = %refusal.source, "Click'n'Load hand-over refused");
    match refusal.kind {
        RefusalKind::InvalidPayload => ApiError::bad_request(
            code::INVALID_PAYLOAD,
            "The Click'n'Load payload could not be read",
        ),
        RefusalKind::InvalidKey => ApiError::bad_request(
            code::JK_REFUSED,
            "The Click'n'Load key was refused; jk is evaluated only in the service's sandbox",
        ),
        RefusalKind::NoLinks => ApiError::bad_request(
            code::NO_LINKS,
            "The Click'n'Load request names no link the LinkGrabber takes",
        ),
        RefusalKind::TooLarge => ApiError::payload_too_large(
            code::TOO_LARGE,
            "The Click'n'Load payload or one of its fields exceeds its limit",
        ),
        RefusalKind::RateLimited => ApiError::too_many_requests(
            code::RATE_LIMITED,
            "More Click'n'Load hand-overs than the service accepts per minute",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{ClickAndLoadRequest, code, read, source_label};

    fn request() -> ClickAndLoadRequest {
        ClickAndLoadRequest {
            crypted: None,
            jk: None,
            urls: None,
            passwords: None,
            package: None,
            source: None,
        }
    }

    #[test]
    fn the_label_is_the_pages_host_and_nothing_else_is_taken_for_one() {
        assert_eq!(
            source_label(Some("https://hoster.test/folder/1")).ok(),
            Some("hoster.test".to_owned())
        );
        assert_eq!(source_label(None).ok(), Some("Browser".to_owned()));
        assert_eq!(source_label(Some("  ")).ok(), Some("Browser".to_owned()));
        for refused in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "not an address",
        ] {
            let error = source_label(Some(refused)).expect_err(refused);
            assert_eq!(error.code(), code::INVALID_PAYLOAD, "{refused}");
        }
    }

    #[tokio::test]
    async fn the_two_shapes_are_never_mixed_and_one_of_them_is_needed() {
        let both = ClickAndLoadRequest {
            crypted: Some("AAAA".to_owned()),
            jk: Some("abababababababababababababababab".to_owned()),
            urls: Some("https://example.com/a".to_owned()),
            ..request()
        };
        let error = read(both).await.err().expect("both shapes at once");
        assert_eq!(error.code(), code::INVALID_PAYLOAD);
        let error = read(request()).await.err().expect("neither shape");
        assert_eq!(error.code(), code::NO_LINKS);
        let keyless = ClickAndLoadRequest {
            crypted: Some("AAAA".to_owned()),
            ..request()
        };
        let error = read(keyless).await.err().expect("crypted without jk");
        assert_eq!(error.code(), code::JK_REFUSED);
    }

    #[tokio::test]
    async fn every_field_is_measured_before_anything_is_read() {
        let oversized = ClickAndLoadRequest {
            urls: Some("https://example.com/a".to_owned()),
            package: Some("p".repeat(super::MAX_FIELD_BYTES + 1)),
            ..request()
        };
        let error = read(oversized).await.err().expect("an oversized package");
        assert_eq!(error.code(), code::TOO_LARGE);
        let script = ClickAndLoadRequest {
            crypted: Some("AAAA".to_owned()),
            jk: Some(format!(
                "function f(){{ return '{}'; }}{}",
                "ab".repeat(16),
                " ".repeat(super::MAX_JK_BYTES)
            )),
            ..request()
        };
        let error = read(script).await.err().expect("an oversized jk");
        assert_eq!(error.code(), code::TOO_LARGE);
    }

    #[tokio::test]
    async fn plain_links_keep_their_package_and_first_password() {
        let plain = ClickAndLoadRequest {
            urls: Some("https://example.com/a.rar\r\nhttps://example.com/b.rar".to_owned()),
            passwords: Some("\nsecret\nother".to_owned()),
            package: Some(" Release ".to_owned()),
            source: Some("https://hoster.test/f/1".to_owned()),
            ..request()
        };
        let intake = read(plain).await.expect("plain links");
        assert_eq!(
            intake.text.as_deref(),
            Some("https://example.com/a.rar\nhttps://example.com/b.rar")
        );
        assert_eq!(intake.package_name.as_deref(), Some("Release"));
        assert_eq!(intake.password.as_deref(), Some("secret"));
        assert_eq!(intake.source_label.as_deref(), Some("hoster.test"));
        assert_eq!(intake.source, rd_core::IngressSource::ClickAndLoad);
    }
}
