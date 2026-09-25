//! Request building and answer reading for the JSON API under `app.<site>/api`.
//!
//! Everything here speaks [`plugin_common`] rather than either host's vocabulary, which is what
//! lets the flows above it be compiled once for both builds. Answers are read the way
//! `docs/plugins.md` ("How strictly a provider's answer may be read") asks: every field but the
//! one a step cannot do without is optional, a count may arrive as a number or a quoted string,
//! and a body that does not fit is a `Permanent` failure naming the field, never the value.
//!
//! One measured fact shapes every request: the API answers a refusal as JSON only when the
//! request carries `Accept: application/json`. Without it a validation failure is a `302` to
//! the SPA and a conflict is Symfony's HTML error page (both kept as fixtures), neither of which
//! says anything a resolver can act on.

use plugin_common::{Failure, FailureKind, HttpRequest, HttpResponse, PluginHost};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::brand::Brand;
use crate::reason::{coded, invalid_response, sanitize_code};

/// A JSON `POST` to `api/<path>`, sent as the site's own page would send it.
#[must_use]
pub fn post(brand: &Brand, path: &str, body: &serde_json::Value, page: &str) -> HttpRequest {
    with_browser_headers(
        brand,
        HttpRequest::post(brand.api_url(path), body.to_string().into_bytes())
            .with_header("Content-Type", "application/json"),
        page,
    )
}

/// A `GET` of `api/<path>`.
#[must_use]
pub fn get(brand: &Brand, path: &str, page: &str) -> HttpRequest {
    with_browser_headers(brand, HttpRequest::get(brand.api_url(path)), page)
}

/// The operator's documented link check: `links`, URL-encoded, one link per line.
#[must_use]
pub fn links_check(brand: &Brand, links: &[String]) -> HttpRequest {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("links", &links.join("\n"))
        .finish();
    with_browser_headers(
        brand,
        HttpRequest::post(brand.api_url("links/check"), body.into_bytes())
            .with_header("Content-Type", "application/x-www-form-urlencoded"),
        &brand.site_url(),
    )
}

fn with_browser_headers(brand: &Brand, request: HttpRequest, page: &str) -> HttpRequest {
    request
        .with_header("Accept", "application/json")
        .with_header("Origin", format!("https://{}", brand.site_host))
        .with_header("Referer", page.to_owned())
}

/// A count as the API sends it: a number, occasionally a float, occasionally a quoted string
/// with a comma for the decimal point (`dayTrafficLeft`).
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum Count {
    Integer(u64),
    Float(f64),
    Text(String),
}

impl Count {
    /// The whole number of units, or `None` for a value that is not one.
    #[must_use]
    pub fn into_u64(self) -> Option<u64> {
        match self {
            Self::Integer(value) => Some(value),
            Self::Float(value) => float_to_u64(value),
            Self::Text(text) => {
                let text = text.trim().replace(',', ".");
                text.parse::<u64>()
                    .ok()
                    .or_else(|| text.parse::<f64>().ok().and_then(float_to_u64))
            }
        }
    }

    /// The value as a fraction, for a figure stated in gigabytes.
    #[must_use]
    pub fn into_f64(self) -> Option<f64> {
        match self {
            Self::Integer(value) => Some(value as f64),
            Self::Float(value) => value.is_finite().then_some(value),
            Self::Text(text) => text.trim().replace(',', ".").parse::<f64>().ok(),
        }
    }
}

fn float_to_u64(value: f64) -> Option<u64> {
    (value.is_finite() && value >= 0.0 && value < u64::MAX as f64).then_some(value as u64)
}

/// The envelope every refusal arrives in: an `error_name` (a code), a `message` (prose, for a
/// person), Laravel's per-field `errors`, and the login's `needCaptcha`. All optional, because
/// the same endpoint sends different subsets on different days.
#[derive(Debug, Default, Deserialize)]
pub struct ErrorBody {
    pub error_name: Option<String>,
    pub message: Option<String>,
    pub errors: Option<serde_json::Value>,
    #[serde(rename = "needCaptcha")]
    pub need_captcha: Option<bool>,
}

/// How the API refused a call, in the terms the flows decide on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Refusal {
    /// `404` `file_is_not_available_for_download`, `File not found`.
    FileUnavailable,
    /// `File can be download only with premium`, `File size is greater than allowed`.
    PremiumOnly,
    /// `409` `Download url not found`.
    NoDirectLink,
    /// `422` `invalid_captcha`, `Captcha is invalid`.
    CaptchaInvalid,
    /// `422` `password_incorrect`.
    PasswordIncorrect,
    /// `422` `needCaptcha: true` — the login wants a Turnstile first.
    NeedCaptcha,
    /// `401` `Unauthenticated.`
    Unauthenticated,
    /// `429`.
    RateLimited,
    /// An `error_name` this crate does not know, sanitised.
    Api(String),
    /// A status nothing above explains.
    Http(u16),
}

/// What the response says, when it says no. `None` for a `2xx` that carries no `error_name`.
#[must_use]
pub fn refusal(response: &HttpResponse) -> Option<Refusal> {
    let body: ErrorBody = serde_json::from_slice(&response.body).unwrap_or_default();
    if let Some(name) = body.error_name.as_deref() {
        return Some(match name {
            "file_is_not_available_for_download" => Refusal::FileUnavailable,
            "invalid_captcha" => Refusal::CaptchaInvalid,
            "password_incorrect" => Refusal::PasswordIncorrect,
            other => Refusal::Api(sanitize_code(other)),
        });
    }
    if body.need_captcha == Some(true) {
        return Some(Refusal::NeedCaptcha);
    }
    let message = body.message.unwrap_or_default().to_ascii_lowercase();
    let captcha_field = body
        .errors
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .is_some_and(|fields| fields.contains_key("captcha"));
    if message.contains("file not found") {
        return Some(Refusal::FileUnavailable);
    }
    if message.contains("only with premium") || message.contains("greater than allowed") {
        return Some(Refusal::PremiumOnly);
    }
    if message.contains("download url not found") {
        return Some(Refusal::NoDirectLink);
    }
    if message.contains("captcha is invalid") || captcha_field {
        return Some(Refusal::CaptchaInvalid);
    }
    match response.status {
        200..=299 => None,
        401 => Some(Refusal::Unauthenticated),
        404 => Some(Refusal::FileUnavailable),
        429 => Some(Refusal::RateLimited),
        422 => Some(Refusal::Api(first_error_field(body.errors.as_ref()))),
        status => Some(Refusal::Http(status)),
    }
}

/// The name of the first field Laravel's `errors` object complains about, or `validation`.
fn first_error_field(errors: Option<&serde_json::Value>) -> String {
    errors
        .and_then(serde_json::Value::as_object)
        .and_then(|fields| fields.keys().next())
        .map_or_else(|| "validation".to_owned(), |field| sanitize_code(field))
}

/// The failure a refusal is reported as.
#[must_use]
pub fn failure_for(brand: &Brand, refusal: Refusal) -> Failure {
    let codes = &brand.codes;
    match refusal {
        Refusal::FileUnavailable => coded(
            brand,
            FailureKind::Offline,
            codes.file_unavailable,
            "the file is not available for download",
        ),
        Refusal::PremiumOnly => coded(
            brand,
            FailureKind::AuthRequired,
            codes.premium_only,
            "this file can be downloaded with a premium account only",
        ),
        Refusal::NoDirectLink => coded(
            brand,
            FailureKind::Permanent,
            codes.no_direct_link,
            "the site did not hand out a download link",
        ),
        Refusal::CaptchaInvalid => coded(
            brand,
            FailureKind::CaptchaFailed,
            codes.captcha_rejected,
            "the captcha answer was rejected",
        ),
        Refusal::PasswordIncorrect => coded(
            brand,
            FailureKind::AccountInvalid,
            codes.login_failed,
            "the e-mail address or password was rejected",
        ),
        Refusal::NeedCaptcha => coded(
            brand,
            FailureKind::Transient(None),
            codes.login_captcha,
            "the sign-in's captcha was not accepted",
        ),
        Refusal::Unauthenticated => coded(
            brand,
            FailureKind::AuthRequired,
            codes.not_signed_in,
            "the API refused the call as not signed in",
        ),
        Refusal::RateLimited => coded(
            brand,
            FailureKind::RateLimited(None),
            codes.rate_limited,
            "the API's rate limit was reached",
        ),
        Refusal::Api(code) => coded(
            brand,
            FailureKind::Permanent,
            codes.api_error,
            &format!("the API refused the call ({code})"),
        )
        .with_param("code", code),
        Refusal::Http(status) => {
            let kind = match status {
                408 | 500..=599 => FailureKind::Transient(None),
                _ => FailureKind::Permanent,
            };
            coded(
                brand,
                kind,
                codes.http_error,
                &format!("HTTP status {status}"),
            )
            .with_param("status", status.to_string())
        }
    }
}

/// Sends `request` and reads the answer as `T`, or says how the API refused it.
///
/// The outer `Err` is a failure of the call itself — transport, a body that is not JSON, an
/// answer that is a page — and the inner one is the API's refusal, for the flows that act on
/// the refusal's kind (a second captcha round, a sign-in) before turning it into a failure.
///
/// # Errors
///
/// The host's failure for a request it would not make, `invalid_response` for a body that
/// cannot be read.
pub async fn exchange<T: DeserializeOwned, H: PluginHost>(
    brand: &Brand,
    host: &H,
    request: HttpRequest,
    what: &str,
) -> Result<Result<T, Refusal>, Failure> {
    let response = host.http(request).await?;
    if is_html(&response) {
        // The SPA shell, a redirect page, an error page: never a result and never a refusal
        // worth classifying, whatever the status — a `302` to the SPA says nothing about the
        // file, only that the request lacked the `Accept` header.
        return Err(invalid_response(brand, what));
    }
    if let Some(refusal) = refusal(&response) {
        return Ok(Err(refusal));
    }
    serde_json::from_slice(&response.body)
        .map(Ok)
        .map_err(|_| invalid_response(brand, what))
}

/// [`exchange`] with the refusal already turned into its failure.
///
/// # Errors
///
/// See [`exchange`] and [`failure_for`].
pub async fn call<T: DeserializeOwned, H: PluginHost>(
    brand: &Brand,
    host: &H,
    request: HttpRequest,
    what: &str,
) -> Result<T, Failure> {
    exchange(brand, host, request, what)
        .await?
        .map_err(|refusal| failure_for(brand, refusal))
}

/// Whether the answer is a page rather than JSON: by content type, or by a body that opens
/// like markup when no type was sent.
#[must_use]
pub fn is_html(response: &HttpResponse) -> bool {
    match response.header("content-type") {
        Some(value) => value.to_ascii_lowercase().starts_with("text/html"),
        None => response
            .body
            .iter()
            .find(|byte| !byte.is_ascii_whitespace())
            .is_some_and(|byte| *byte == b'<'),
    }
}
