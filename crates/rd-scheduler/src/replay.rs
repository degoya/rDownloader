//! Replaying a consented browser request, and renewing its URL before old partial state is
//! reused.
//!
//! Two things live here that the worker deliberately does not carry itself: the decrypted
//! request template (method, body, approved origins, captured user agent), and the
//! pre-resume refresh hook.
//!
//! The hook is bounded on purpose. It never executes JavaScript, never drives a browser, and
//! never follows a redirect outside the origins a person approved. It has exactly two
//! sources, tried in order, and a legible block when neither works.

use anyhow::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, TimeDelta, Utc};
use rd_core::{DownloadFile, Failure, FailureKind, ReplayBlockReason, ReplayMethod};
use rd_http::{ReplayPayload, ReplayScope};
use url::Url;

use crate::SchedulerHandle;

/// How close to its deadline a signed URL is treated as already expired.
///
/// A transfer that starts one second before expiry is a transfer that fails halfway.
const REFRESH_SKEW: TimeDelta = TimeDelta::seconds(60);

/// Everything the transfer needs to reproduce one consented request.
#[derive(Clone, Debug, Default)]
pub(crate) struct ReplayContext {
    pub method: ReplayMethod,
    pub body: Option<ReplayPayload>,
    pub approved_origins: Vec<String>,
    pub captured_user_agent: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl ReplayContext {
    /// Whether this is a POST, whose transfer semantics differ from a plain GET.
    pub fn is_post(&self) -> bool {
        self.method == ReplayMethod::Post
    }

    /// The redirect containment a pooled client must be built with.
    pub fn scope(&self) -> Option<ReplayScope> {
        ReplayScope::new(self.approved_origins.clone())
    }
}

/// Loads the consented template of a download and decrypts its body.
///
/// Returns `None` for every ordinary download, which keeps the whole transfer path
/// byte-identical to what it was before replay existed.
pub(crate) async fn load(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
) -> Result<Option<ReplayContext>> {
    let Some(template) = scheduler.database.request_template(file.id).await? else {
        return Ok(None);
    };
    let request = &template.request;
    let method = ReplayMethod::parse(&request.method).unwrap_or_default();

    let body = match (request.body.as_ref(), method) {
        (Some(metadata), ReplayMethod::Post) if metadata.stored => {
            let reference = scheduler.database.request_body_ref(file.id).await?;
            match reference {
                Some(reference) => {
                    let encoded = scheduler.secrets.get(&reference).await?;
                    let bytes = STANDARD
                        .decode(secrecy::ExposeSecret::expose_secret(&encoded).as_bytes())?;
                    Some(ReplayPayload {
                        content_type: metadata.content_type.clone(),
                        bytes: bytes.into(),
                    })
                }
                None => None,
            }
        }
        _ => None,
    };

    Ok(Some(ReplayContext {
        method,
        body,
        // The consent is authoritative, not the capture: a person may have narrowed the set.
        approved_origins: template.consent.approved_origins.clone(),
        captured_user_agent: request.user_agent.clone(),
        expires_at: request.expires_at,
    }))
}

/// Why a stored transfer URL should not simply be reused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Staleness {
    /// Reuse it; this is the ordinary case and costs nothing.
    Fresh,
    /// Its deadline has passed, or is about to.
    Expired,
    /// Signed, with no readable deadline. After a pause or a restart that is a coin flip.
    Signed,
    /// The last attempt was refused for authentication reasons.
    PriorAuthFailure,
}

/// Decides whether a resume may reuse the URL the partial file came from.
pub(crate) fn staleness(
    source: &Url,
    replay: Option<&ReplayContext>,
    file: &DownloadFile,
    now: DateTime<Utc>,
) -> Staleness {
    if let Some(failure) = &file.last_error
        && matches!(
            failure.category,
            FailureKind::AuthRequired | FailureKind::AccountInvalid
        )
    {
        return Staleness::PriorAuthFailure;
    }
    let deadline = replay
        .and_then(|replay| replay.expires_at)
        .or_else(|| rd_core::signed_url_expiry(source));
    if let Some(deadline) = deadline {
        return if deadline <= now + REFRESH_SKEW {
            Staleness::Expired
        } else {
            Staleness::Fresh
        };
    }
    if rd_core::is_signed_url(source) {
        return Staleness::Signed;
    }
    Staleness::Fresh
}

/// Outcome of the pre-resume refresh hook.
pub(crate) enum Refreshed {
    /// Nothing needed renewing.
    Fresh,
    /// The URL was renewed; the transfer continues against this one, and **only** this one.
    ///
    /// Deliberately not persisted (RD-108-17). A renewed address arrives with no statement
    /// of how long it is good for: `ResolvedDownload` has no expiry field, and
    /// [`renew_from_capture`] hands back a bare redirect target. Writing an address of
    /// unknown lifetime into the template would turn the next resume into a gamble — a
    /// single-use or minute-scale ticket is dead by then, and the attempt it breaks costs
    /// more than the refresh it saved: the 403 comes back as `AuthRequired`, which burns
    /// the one reactive resolver refresh in `failures::record_http_error_with_replay` and
    /// writes a user-visible failure, and a refresh still has to happen afterwards.
    ///
    /// Re-deriving is cheap and always correct instead. The durable address is
    /// `file.source`, which every attempt re-resolves from (`worker::run`), and the
    /// deadline of a signed URL is read back out of the URL itself
    /// (`rd_core::signed_url_expiry`) rather than from a stored copy that can go stale.
    /// `rd_core::stable_hash` already treats the signature as volatile and excludes it from
    /// the consent identity for the same reason. The cost of that choice is one windowed
    /// budget slot per resume of an expired capture, which is what the window in migration
    /// 0027 is sized for.
    Replaced {
        url: Url,
        headers: Vec<(String, String)>,
    },
    /// Nothing could renew it. The download is blocked with this reason, and the partial
    /// file stays on disk so a person can decide what to do.
    Impossible(ReplayBlockReason),
}

/// Renews an expiring or signed URL before old partial state is reused.
///
/// Called only when bytes are already committed: a fresh start has nothing to protect and
/// pays no cost here.
pub(crate) async fn before_resume(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    source: &Url,
    replay: Option<&ReplayContext>,
) -> Result<Refreshed> {
    if staleness(source, replay, file, Utc::now()) == Staleness::Fresh {
        return Ok(Refreshed::Fresh);
    }
    // Bounded: a windowed budget separate from the one-shot reactive resolver refresh, so
    // neither can consume the other.
    if !scheduler.database.claim_replay_refresh(file.id).await? {
        return Ok(Refreshed::Impossible(ReplayBlockReason::RefreshUnavailable));
    }

    // Source 1 — a resolver that claims this link. Always re-resolve the *original* source,
    // never the expired transfer URL.
    let pin = scheduler.database.resolver_pin(file.id).await?;
    match scheduler
        .resolvers
        .resolve(
            file.source.clone(),
            file.account_id,
            file.proxy_profile_id,
            pin.as_ref(),
        )
        .await
    {
        Ok(Some(resolved)) => {
            scheduler.clients.clear().await;
            let headers = resolved
                .headers
                .into_iter()
                .map(|header| (header.name, header.value))
                .collect();
            return Ok(Refreshed::Replaced {
                url: resolved.url,
                headers,
            });
        }
        Ok(None) => {}
        Err(failure) => {
            tracing::warn!(
                failure = %rd_core::redact_text(&failure.message),
                "resolver could not renew an expiring transfer URL"
            );
        }
    }

    // Source 2 — re-request the original capture URL and adopt the fresh redirect.
    //
    // Needed because `ResolverService::resolve` answers `Ok(None)` for any link no plugin
    // claims, which is exactly the plain browser capture this feature exists for. Kept
    // deliberately narrow: one request, no JavaScript, and the client's redirect policy
    // still confines it to the approved origins.
    if let Some(replay) = replay {
        if replay.is_post() {
            // A refresh POST would be an unrequested second side effect on the origin.
            return Ok(Refreshed::Impossible(ReplayBlockReason::RefreshUnavailable));
        }
        match renew_from_capture(scheduler, file, replay).await {
            Ok(Some(url)) => {
                return Ok(Refreshed::Replaced {
                    url,
                    headers: Vec::new(),
                });
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(
                    error = %rd_core::redact_text(&error.to_string()),
                    "could not renew a captured transfer URL"
                );
            }
        }
    }

    Ok(Refreshed::Impossible(ReplayBlockReason::RefreshUnavailable))
}

/// Re-requests the original captured URL and returns the URL it now resolves to.
async fn renew_from_capture(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    replay: &ReplayContext,
) -> Result<Option<Url>> {
    let network = crate::worker::build_replay_client(scheduler, file, Some(replay)).await?;
    let probe =
        rd_http::probe_with_headers(&network.client, file.source.clone(), &network.headers).await;
    let Ok(result) = probe else { return Ok(None) };
    // A landing page is not a renewed download; adopting it would store HTML under the
    // file's name, which is the exact failure `not_a_file` exists to prevent.
    if !result.looks_downloadable() {
        return Ok(None);
    }
    // Only accept a genuinely different URL: the same expired one is not a renewal.
    Ok((result.final_url != file.source).then_some(result.final_url))
}

/// The failure a download is blocked with when nothing could renew its URL.
///
/// `AuthRequired` is deliberate: `record_failure` maps it to `Blocked` rather than `Failed`,
/// which preserves the partial file, renders in the UI as an actionable state, and starts no
/// retry storm.
pub(crate) fn blocked(reason: ReplayBlockReason) -> Failure {
    let reason = serde_json::to_string(&reason).unwrap_or_default();
    Failure::coded(
        FailureKind::AuthRequired,
        "download.replay_expired",
        "The download link expired and could not be renewed",
    )
    .with_param("reason", reason.trim_matches('"'))
}

#[cfg(test)]
mod tests {
    use super::{ReplayContext, Staleness, staleness};
    use chrono::{TimeDelta, Utc};
    use rd_core::{DownloadFile, Failure, FailureKind};
    use url::Url;

    fn url(input: &str) -> Url {
        input.parse().expect("url")
    }

    fn file(last_error: Option<Failure>) -> DownloadFile {
        let now = Utc::now();
        DownloadFile {
            recording: None,
            id: rd_core::DownloadId::new(),
            package_id: rd_core::PackageId::new(),
            source: url("https://hoster.example/dl/1"),
            file_name: "f.bin".to_owned(),
            state: rd_core::DownloadState::Downloading,
            total_bytes: None,
            committed_bytes: rd_core::ByteCount::default(),
            retry_count: 0,
            next_retry_at: None,
            expected_checksum: None,
            computed_checksum: None,
            last_error,
            account_id: None,
            proxy_profile_id: None,
            remote_credential_id: None,
            mirror_group: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            position: 0,
            kind: rd_core::DownloadKind::Http,
            nzb_file_id: None,
            recovery: false,
            media: None,
            enrichment: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn an_ordinary_url_is_never_refreshed() {
        // The hot path must cost nothing for the overwhelmingly common case.
        assert_eq!(
            staleness(
                &url("https://cdn.example/f.bin"),
                None,
                &file(None),
                Utc::now()
            ),
            Staleness::Fresh
        );
    }

    #[test]
    fn a_signed_url_without_a_readable_deadline_is_refreshed() {
        assert_eq!(
            staleness(
                &url("https://cdn.example/f.bin?sig=abc"),
                None,
                &file(None),
                Utc::now()
            ),
            Staleness::Signed
        );
    }

    #[test]
    fn a_deadline_is_read_from_the_url_and_honoured_with_skew() {
        let now = Utc::now();
        let soon = (now + TimeDelta::seconds(30)).timestamp();
        // Inside the skew window: starting now would fail halfway.
        assert_eq!(
            staleness(
                &url(&format!("https://cdn.example/f.bin?Expires={soon}")),
                None,
                &file(None),
                now
            ),
            Staleness::Expired
        );
        let later = (now + TimeDelta::hours(2)).timestamp();
        assert_eq!(
            staleness(
                &url(&format!("https://cdn.example/f.bin?Expires={later}")),
                None,
                &file(None),
                now
            ),
            Staleness::Fresh
        );
    }

    #[test]
    fn the_templates_expiry_wins_over_the_urls() {
        let now = Utc::now();
        let replay = ReplayContext {
            expires_at: Some(now - TimeDelta::hours(1)),
            ..ReplayContext::default()
        };
        assert_eq!(
            staleness(
                &url("https://cdn.example/f.bin"),
                Some(&replay),
                &file(None),
                now
            ),
            Staleness::Expired
        );
    }

    #[test]
    fn a_previous_auth_failure_forces_a_refresh() {
        for category in [FailureKind::AuthRequired, FailureKind::AccountInvalid] {
            assert_eq!(
                staleness(
                    &url("https://cdn.example/f.bin"),
                    None,
                    &file(Some(Failure::new(category, "denied"))),
                    Utc::now()
                ),
                Staleness::PriorAuthFailure
            );
        }
        // An unrelated failure is not a reason to re-resolve.
        assert_eq!(
            staleness(
                &url("https://cdn.example/f.bin"),
                None,
                &file(Some(Failure::new(FailureKind::Offline, "no route"))),
                Utc::now()
            ),
            Staleness::Fresh
        );
    }
}
