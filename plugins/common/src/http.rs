//! What an HTTP status means when nothing else in the answer explains it, and how long a
//! provider asked to be left alone (RD-191-07, PLUG-12).
//!
//! Twenty-two plugins used to carry their own `ensure_http_status`, and they disagreed: `451`
//! was `Offline` in some and `Permanent` in others, and a `Retry-After` on a `429` was read by
//! two of them and dropped by the rest. Twenty-three `Retry-After` readers disagreed again — one
//! ignored a wait past an hour, the others took a year at its word. The status is classified
//! here once; what a plugin still decides is which of its own codes to report each class under,
//! and the statuses its provider documents beyond these (a `423` for a used-up quota, say),
//! which it checks before calling [`http_status`].
//!
//! Plain Rust with no dependencies, so a guest that takes it gains no import.

use crate::types::FailureKind;

/// Longest wait a `Retry-After` is taken at, in seconds: one day.
///
/// The host clamps every wait it is handed to the same ceiling (`rd_core::MAX_RETRY_AFTER_SECONDS`,
/// RD-191-06); a plugin cannot depend on `rd-core`, so the number is repeated here and a test on
/// the native target holds the two together. Clamping here as well means a plugin's own
/// arithmetic — a default it falls back on, a backoff on top — never starts from a year.
pub const MAX_RETRY_AFTER_SECONDS: u64 = 24 * 60 * 60;

/// `seconds`, at most [`MAX_RETRY_AFTER_SECONDS`].
#[must_use]
pub const fn clamp_retry_after(seconds: u64) -> u64 {
    if seconds > MAX_RETRY_AFTER_SECONDS {
        MAX_RETRY_AFTER_SECONDS
    } else {
        seconds
    }
}

/// Reads a `Retry-After` stated in seconds, clamped to [`MAX_RETRY_AFTER_SECONDS`].
///
/// Only the numeric form: the date form needs a clock to subtract from, a guest has none of its
/// own, and a wrong guess would be a wait the scheduler takes literally. A date, a negative
/// number, garbage and `0` are all `None`, which means "the caller's own default", never "retry
/// at once" — a `0` taken literally turns a rate limit into a loop.
#[must_use]
pub fn retry_after_seconds(value: Option<&str>) -> Option<u64> {
    let seconds: u64 = value?.trim().parse().ok()?;
    (seconds > 0).then_some(clamp_retry_after(seconds))
}

/// The `Retry-After` header among `headers`, read as [`retry_after_seconds`] reads it.
#[must_use]
pub fn retry_after(headers: &[(String, String)]) -> Option<u64> {
    retry_after_seconds(header(headers, "retry-after"))
}

/// A response header looked up case-insensitively, for the guests that hold the WIT's header
/// list rather than an [`crate::HttpResponse`].
#[must_use]
pub fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(header, _)| header.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// What a status that is not a success means, before a plugin names it in its own words.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HttpRefusal {
    /// `401` and `403`: the credential was refused.
    Unauthorized,
    /// `404` and `410`: the thing asked for is not there. `Permanent`, not retried (owner,
    /// 2026-10-04): a deleted file does not come back after the next backoff, and retrying it
    /// up to `max_retries` only delays the answer the person needs.
    Gone,
    /// `451`: withheld for legal reasons. `Offline` — retried, since a block can be regional
    /// or lifted, and the person sees the file as offline rather than as a plugin fault. A
    /// plugin may still word it as a refusal; only the class is fixed here.
    Unavailable,
    /// `429`, with the provider's `Retry-After` when it sent a usable one.
    RateLimited(Option<u64>),
    /// Any `5xx`, with the provider's `Retry-After` when it sent one (a `503` may).
    ServerError(Option<u64>),
    /// Every other status that is not a `2xx`.
    Other(u16),
}

impl HttpRefusal {
    /// The scheduler category each class is reported under.
    #[must_use]
    pub const fn kind(self) -> FailureKind {
        match self {
            Self::Unauthorized => FailureKind::AccountInvalid,
            Self::Gone => FailureKind::Permanent,
            Self::Unavailable => FailureKind::Offline,
            Self::RateLimited(wait) => FailureKind::RateLimited(wait),
            Self::ServerError(wait) => FailureKind::Transient(wait),
            Self::Other(_) => FailureKind::Permanent,
        }
    }
}

/// Classifies an HTTP status no document in the answer explains.
///
/// `retry_after` is the wait the response stated, usually [`retry_after`] of its headers; it
/// is clamped again here, so a caller that read it some other way cannot pass a year through.
///
/// # Errors
///
/// The class of every status that is not a `2xx`.
pub fn http_status(status: u16, retry_after: Option<u64>) -> Result<(), HttpRefusal> {
    let wait = retry_after
        .filter(|seconds| *seconds > 0)
        .map(clamp_retry_after);
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(HttpRefusal::Unauthorized),
        404 | 410 => Err(HttpRefusal::Gone),
        451 => Err(HttpRefusal::Unavailable),
        429 => Err(HttpRefusal::RateLimited(wait)),
        500..=599 => Err(HttpRefusal::ServerError(wait)),
        other => Err(HttpRefusal::Other(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HttpRefusal, MAX_RETRY_AFTER_SECONDS, http_status, retry_after, retry_after_seconds,
    };
    use crate::FailureKind;

    #[test]
    fn a_success_is_not_a_refusal() {
        for status in [200, 204, 206, 299] {
            assert_eq!(http_status(status, None), Ok(()), "{status}");
        }
    }

    /// One mapping for every plugin: the disagreement over `451` is what this module ended.
    #[test]
    fn each_status_class_maps_the_same_way_for_everybody() {
        assert_eq!(http_status(401, None), Err(HttpRefusal::Unauthorized));
        assert_eq!(http_status(403, None), Err(HttpRefusal::Unauthorized));
        assert_eq!(http_status(429, None), Err(HttpRefusal::RateLimited(None)));
        assert_eq!(http_status(502, None), Err(HttpRefusal::ServerError(None)));
        assert_eq!(http_status(418, None), Err(HttpRefusal::Other(418)));
        assert_eq!(HttpRefusal::Other(418).kind(), FailureKind::Permanent);
        assert_eq!(
            HttpRefusal::Unauthorized.kind(),
            FailureKind::AccountInvalid
        );
    }

    /// `404` and `410` are final and not retried; `451` is `Offline` and retried (owner,
    /// 2026-10-04, RA-PLG-02).
    #[test]
    fn not_found_is_permanent_and_a_legal_block_is_offline() {
        for gone in [404, 410] {
            assert_eq!(http_status(gone, None), Err(HttpRefusal::Gone), "{gone}");
        }
        assert_eq!(HttpRefusal::Gone.kind(), FailureKind::Permanent);
        assert_eq!(http_status(451, None), Err(HttpRefusal::Unavailable));
        assert_eq!(HttpRefusal::Unavailable.kind(), FailureKind::Offline);
        // A stated wait changes neither class.
        assert_eq!(http_status(404, Some(60)), Err(HttpRefusal::Gone));
        assert_eq!(http_status(451, Some(60)), Err(HttpRefusal::Unavailable));
    }

    /// A `Retry-After` on a `429` or a `503` travels into the refusal — it used to be read by
    /// two plugins of twenty-two.
    #[test]
    fn a_stated_wait_travels_with_a_rate_limit_and_a_server_error() {
        assert_eq!(
            http_status(429, Some(120)),
            Err(HttpRefusal::RateLimited(Some(120)))
        );
        assert_eq!(
            http_status(503, Some(30)),
            Err(HttpRefusal::ServerError(Some(30)))
        );
        assert_eq!(
            HttpRefusal::RateLimited(Some(120)).kind(),
            FailureKind::RateLimited(Some(120))
        );
        assert_eq!(
            HttpRefusal::ServerError(Some(30)).kind(),
            FailureKind::Transient(Some(30))
        );
    }

    /// A provider that asks for a year is answered with a day, the host's own ceiling — here as
    /// well, so a plugin's arithmetic on top never starts from the year.
    #[test]
    fn a_wait_is_clamped_to_one_day() {
        assert_eq!(
            http_status(429, Some(u64::MAX)),
            Err(HttpRefusal::RateLimited(Some(MAX_RETRY_AFTER_SECONDS)))
        );
        assert_eq!(
            retry_after_seconds(Some("31536000")),
            Some(MAX_RETRY_AFTER_SECONDS)
        );
        assert_eq!(retry_after_seconds(Some("86400")), Some(86_400));
    }

    #[test]
    fn only_a_positive_number_of_seconds_is_a_wait() {
        assert_eq!(retry_after_seconds(Some(" 90 ")), Some(90));
        assert_eq!(retry_after_seconds(Some("0")), None);
        assert_eq!(retry_after_seconds(Some("-5")), None);
        assert_eq!(
            retry_after_seconds(Some("Wed, 21 Oct 2015 07:28:00 GMT")),
            None
        );
        assert_eq!(retry_after_seconds(Some("")), None);
        assert_eq!(retry_after_seconds(None), None);
        assert_eq!(
            http_status(429, Some(0)),
            Err(HttpRefusal::RateLimited(None))
        );
    }

    #[test]
    fn the_header_is_found_whatever_its_case() {
        let headers = vec![
            ("Content-Type".to_owned(), "text/plain".to_owned()),
            ("RETRY-AFTER".to_owned(), "45".to_owned()),
        ];
        assert_eq!(retry_after(&headers), Some(45));
        assert_eq!(retry_after(&[]), None);
    }

    /// The host clamps to the same number; a plugin cannot depend on `rd-core`, so the two are
    /// held together here, on the target where both exist.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_ceiling_is_the_hosts() {
        assert_eq!(MAX_RETRY_AFTER_SECONDS, rd_core::MAX_RETRY_AFTER_SECONDS);
    }
}
