//! Target-independent pieces of the account-less ("free") Keep2Share download flow: the request
//! bodies, the response shapes, the captcha-image validation and the wait/limit decisions. Shared
//! verbatim by `native/free.rs` and `guest/free.rs` so both adapters send byte-identical bodies
//! and report byte-identical failures; like the rest of [`crate::api`] this module depends only on
//! `serde`/`serde_json`/`url`.
//!
//! IMPL-VERIFY against JD's `svn_trunk/src/jd/plugins/hoster/K2SApi.java` (revision 53214, the
//! `$Revision$` tag on line 76) — `handleDownload` lines 783-935, `ipBlockedOrAccountLimit` lines
//! 1013-1019, and the `handleErrorsAPI` `switch` lines 1426-1545. Verified against that source:
//!
//! - **`/requestcaptcha` takes an empty body.** JD line 858 posts
//!   `new HashMap<String, Object>()` — the line above it, posting `postdata`, is commented out in
//!   the living source. [`requestcaptcha_body`] therefore sends `{}`, not the file id.
//! - **The captcha is an image captcha.** JD line 867 calls `getCaptchaCode(captcha_url, ...)`,
//!   which downloads `captcha_url` and asks a human (or a solver service) to type what it shows —
//!   the `Image` variant of the host's `captcha-challenge`, not a widget. The sibling
//!   `/requestrecaptcha` endpoint (line 1207, login only) is the reCaptcha one and is not used
//!   here.
//! - **No prompt text.** `/requestcaptcha`'s payload carries `challenge` and `captcha_url` and
//!   nothing else (JD lines 859-860; the response sample on line 1399 shows the same two fields),
//!   so no prompt text is ever available and the challenge is handed over with `prompt: None`.
//! - **`/geturl` after the captcha** carries `file_id` + `captcha_challenge` + `captcha_response`
//!   (JD lines 843, 868-869) — see [`geturl_captcha_body`].
//! - **The wait round** posts `file_id` + `free_download_key` and *removes* the captcha fields
//!   (JD lines 907-910) — see [`geturl_free_key_body`].
//! - **Limits are IP blocks.** With `account == null` — this flow's only case —
//!   `ipBlockedOrAccountLimit` raises `LinkStatus.ERROR_IP_BLOCKED` (JD lines 1013-1019), and it is
//!   reached from errorcodes 1/2/5/6 and from a `time_wait` above 180 seconds or a sixth wait round
//!   in a row (JD lines 884-902). [`free_failure`] and [`wait_step`] mirror both.
//!
//! Assumed, not verified against JD:
//!
//! - **The unauthenticated `/geturl` probe.** JD requests a captcha unconditionally in free mode
//!   and only then calls `/geturl`. This module posts `{"file_id":...}` first (see
//!   [`geturl_probe_body`]) and treats errorcode 30 as "now solve a captcha" — JD's own comment on
//!   that arm quotes exactly that response for this situation ("You need send request for free
//!   download with captcha fields", line 1533). The probe is what lets a stated download limit
//!   surface as an IP block *before* a captcha is paid for; JD reaches the same goal with a local
//!   IP/timestamp bookkeeping (`isEnableReconnectWorkaround`, lines 807-843) that a stateless
//!   plugin cannot keep.
//! - **`free_download_key: null` is not sent** on the captcha round. JD simply never puts the key
//!   into `postdata` until the wait round, so it is omitted rather than sent as an explicit JSON
//!   `null`.
//! - **The image's media type** is taken from the response's `Content-Type` and, when that is
//!   missing or not an `image/*` type, sniffed from the leading bytes ([`image_mime`]). JD hands
//!   the URL to its captcha subsystem and never inspects either.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use super::{API_BASE, ApiFailure, ErrorKind, coded, invalid_url};
use crate::messages;

/// `/geturl` — the only endpoint the free flow calls more than once.
pub(crate) const GETURL_PATH: &str = "geturl";

/// `/requestcaptcha` — the image-captcha challenge endpoint (JD line 858).
pub(crate) const REQUESTCAPTCHA_PATH: &str = "requestcaptcha";

/// Longest countdown the flow waits out itself; anything above is an IP block (JD line 894:
/// `if (waitseconds > 180 || stopNow)`).
pub(crate) const MAX_WAIT_SECONDS: u64 = 180;

/// Wait rounds allowed in a row before the flow gives up (JD lines 887-891: `counter > 4`).
pub(crate) const MAX_WAIT_ROUNDS: usize = 5;

/// Failure codes [`free_failure`] re-labels as an IP block. Each maps an errorcode whose JD arm
/// ends in `ipBlockedOrAccountLimit` (1, 2, 5, 6) or, for errorcode 1, in a bare
/// `ERROR_IP_BLOCKED` — all of them "this IP may not download for a while", none of them
/// something a retry of *this* link can fix.
const LIMIT_CODES: &[&str] = &[
    messages::DOWNLOAD_LIMIT_REACHED.0,
    messages::TRAFFIC_EXHAUSTED.0,
    messages::DOWNLOAD_WAIT.0,
    messages::TOO_MANY_PARALLEL.0,
];

#[derive(Serialize)]
struct FileIdRequest<'a> {
    file_id: &'a str,
}

/// `POST /geturl {"file_id":"<id>"}` — the unauthenticated probe (see the module doc's assumption
/// note). Answers with the download URL, with errorcode 30 ("send the captcha fields"), or with a
/// limit error before any captcha has been paid for.
pub(crate) fn geturl_probe_body(file_id: &str) -> Vec<u8> {
    serde_json::to_vec(&FileIdRequest { file_id }).expect("FileIdRequest always serializes")
}

/// `POST /requestcaptcha {}` — JD line 858 posts an empty map.
pub(crate) fn requestcaptcha_body() -> Vec<u8> {
    b"{}".to_vec()
}

#[derive(Serialize)]
struct CaptchaAnswerRequest<'a> {
    file_id: &'a str,
    captcha_challenge: &'a str,
    captcha_response: &'a str,
}

/// `POST /geturl {"file_id":..,"captcha_challenge":..,"captcha_response":..}` (JD lines 843,
/// 868-869).
pub(crate) fn geturl_captcha_body(file_id: &str, challenge: &str, response: &str) -> Vec<u8> {
    serde_json::to_vec(&CaptchaAnswerRequest {
        file_id,
        captcha_challenge: challenge,
        captcha_response: response,
    })
    .expect("CaptchaAnswerRequest always serializes")
}

#[derive(Serialize)]
struct FreeKeyRequest<'a> {
    file_id: &'a str,
    free_download_key: &'a str,
}

/// `POST /geturl {"file_id":..,"free_download_key":..}` — the post-countdown round, with the
/// captcha fields removed exactly as JD removes them (lines 907-910).
pub(crate) fn geturl_free_key_body(file_id: &str, free_download_key: &str) -> Vec<u8> {
    serde_json::to_vec(&FreeKeyRequest {
        file_id,
        free_download_key,
    })
    .expect("FreeKeyRequest always serializes")
}

/// `/requestcaptcha`'s success payload (JD lines 859-860).
#[derive(Deserialize)]
pub(crate) struct RequestCaptchaResult {
    #[serde(default)]
    pub(crate) challenge: Option<String>,
    #[serde(default)]
    pub(crate) captcha_url: Option<String>,
}

/// `/geturl`'s success payload on the free path. `url` ends the flow; `free_download_key` plus
/// `time_wait` start a countdown round (JD's documented sample, line 879:
/// `{"status":"success","code":200,"message":"Captcha accepted, please wait",
/// "free_download_key":"homeHash","time_wait":30}`).
#[derive(Deserialize)]
pub(crate) struct FreeUrlResult {
    #[serde(default)]
    pub(crate) url: Option<String>,
    #[serde(default)]
    pub(crate) free_download_key: Option<String>,
    #[serde(default)]
    pub(crate) time_wait: Option<Value>,
    #[serde(default)]
    pub(crate) message: Option<String>,
}

impl FreeUrlResult {
    /// `time_wait` in whole seconds. JD reads it as a `Number` (line 884); a numeric string is
    /// accepted too, since several other fields of this API are served in both shapes.
    pub(crate) fn wait_seconds(&self) -> Option<u64> {
        match self.time_wait.as_ref()? {
            Value::Number(number) => number
                .as_u64()
                .or_else(|| number.as_f64().map(|value| value.max(0.0) as u64)),
            Value::String(text) => text.split('.').next()?.parse().ok(),
            _ => None,
        }
    }

    /// The API's own wording, for a failure the plugin has no better explanation for.
    pub(crate) fn diagnosis(&self) -> String {
        self.message
            .as_deref()
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .unwrap_or("no download URL in the API response")
            .to_owned()
    }
}

/// What to do with a countdown the API asked for in round `round` (0-based).
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum WaitStep {
    /// Sit out the countdown and post `/geturl` again.
    Wait(u32),
    /// Too long, or too many rounds in a row: JD's `ipBlockedOrAccountLimit` case.
    Blocked(u64),
}

/// Mirrors JD lines 884-902: a countdown longer than [`MAX_WAIT_SECONDS`], or a
/// [`MAX_WAIT_ROUNDS`]-th round in a row, is an IP block rather than something to wait out.
pub(crate) fn wait_step(seconds: u64, round: usize) -> WaitStep {
    if seconds > MAX_WAIT_SECONDS || round >= MAX_WAIT_ROUNDS {
        return WaitStep::Blocked(seconds);
    }
    // The cap above keeps this conversion in range on every target.
    WaitStep::Wait(u32::try_from(seconds).unwrap_or(u32::MAX))
}

/// Whether an error means "now solve a captcha" rather than "this link failed". Errorcodes 30/33
/// (JD lines 1526-1538); the arm's own comment quotes the response the free probe provokes:
/// `{"message":"You need send request for free download with captcha fields",...,"errorCode":30}`.
pub(crate) fn needs_captcha(failure: &ApiFailure) -> bool {
    matches!(failure.kind, ErrorKind::NeedsCaptcha) && failure.code == messages::LOGIN_CAPTCHA.0
}

/// Re-labels a limit error the free flow ran into as an [`ErrorKind::IpBlocked`] under one stable
/// code, so the scheduler holds the whole hoster back instead of retrying each free link into the
/// same wait and captcha. The provider's own wording is preserved as a `message` parameter.
pub(crate) fn free_failure(failure: ApiFailure) -> ApiFailure {
    if !LIMIT_CODES.contains(&failure.code) {
        return failure;
    }
    let retry_after_seconds = match failure.kind {
        ErrorKind::RateLimited(seconds) | ErrorKind::Transient(seconds) => seconds,
        _ => None,
    };
    let mut params = vec![("message", failure.message)];
    if let Some(seconds) = retry_after_seconds {
        params.push(("wait_seconds", seconds.to_string()));
    }
    ApiFailure {
        kind: ErrorKind::IpBlocked(retry_after_seconds),
        code: messages::FREE_LIMIT_REACHED,
        message: messages::free_limit_reached(retry_after_seconds),
        params,
    }
}

/// The IP block a too-long or too-often-repeated countdown amounts to (JD lines 894-901).
pub(crate) fn wait_limit_failure(seconds: u64) -> ApiFailure {
    ApiFailure {
        kind: ErrorKind::IpBlocked(Some(seconds)),
        code: messages::FREE_LIMIT_REACHED,
        message: messages::free_limit_reached(Some(seconds)),
        params: vec![("wait_seconds", seconds.to_string())],
    }
}

/// The free flow ended without a download URL; carries the API's own wording as `diagnosis`.
pub(crate) fn no_free_link_failure(diagnosis: &str) -> ApiFailure {
    ApiFailure {
        kind: ErrorKind::Permanent,
        code: messages::NO_FREE_LINK,
        message: messages::no_free_link(diagnosis),
        params: vec![("diagnosis", diagnosis.to_owned())],
    }
}

/// The challenge id and the absolute image URL `/requestcaptcha` answered with. JD aborts when
/// either is empty (lines 862-866); a relative or `http://` URL is repaired the way JD repairs the
/// login captcha's (lines 1211-1219) rather than rejected.
pub(crate) fn captcha_request(result: &RequestCaptchaResult) -> Result<(String, Url), ApiFailure> {
    let challenge = non_empty(result.challenge.as_deref())
        .ok_or_else(|| coded(ErrorKind::Permanent, messages::CAPTCHA_UNAVAILABLE))?;
    let raw = non_empty(result.captcha_url.as_deref())
        .ok_or_else(|| coded(ErrorKind::Permanent, messages::CAPTCHA_UNAVAILABLE))?;
    let raw = match raw.strip_prefix("http://") {
        Some(rest) => format!("https://{rest}"),
        None => raw.to_owned(),
    };
    let base = Url::parse(API_BASE).map_err(|error| invalid_url(&error))?;
    let image = base.join(&raw).map_err(|error| invalid_url(&error))?;
    Ok((challenge.to_owned(), image))
}

/// The media type to hand the solver with the captcha bytes: the response's own `Content-Type`
/// when it names an image, otherwise the type the leading bytes identify. A body that is neither
/// is an error page rather than a captcha, and reported as such.
pub(crate) fn image_mime(content_type: Option<&str>, data: &[u8]) -> Result<String, ApiFailure> {
    let declared = content_type
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| value.starts_with("image/"));
    if let Some(declared) = declared {
        return Ok(declared);
    }
    sniff_image_mime(data)
        .map(str::to_owned)
        .ok_or_else(|| coded(ErrorKind::Permanent, messages::CAPTCHA_UNAVAILABLE))
}

/// Magic-byte sniffing for the handful of formats a captcha is ever served as.
fn sniff_image_mime(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if data.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if data.starts_with(b"RIFF") && data.len() > 12 && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else if data.starts_with(b"BM") {
        Some("image/bmp")
    } else {
        None
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
#[path = "free/tests.rs"]
mod tests;
