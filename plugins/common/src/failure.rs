//! The error glue of the API plugins, once (RD-1110-02, audit R1).
//!
//! Every debrid and cloud plugin carried the same `ApiFailure`, the same `ErrorKind`, the same
//! `ensure_http_status` that named each [`HttpRefusal`] class in the plugin's own codes, the same
//! conversion into the failure the host reads and the same `call` around it — different in
//! nothing but the codes, the texts and two default waits. Those differences are the plugin's
//! [`HttpWords`] now, a table it declares once; what it still writes itself is what its provider
//! really says: the words of its error envelope and how they are read.
//!
//! Plain Rust with no dependencies, so a guest that takes it gains no import.

use std::fmt::Display;

use crate::host::PluginHost;
use crate::http::{HttpRefusal, http_status, retry_after};
use crate::types::{Failure, FailureKind, HttpRequest, HttpResponse};

/// A stable translation code and its English text: the shape of every `messages` constant.
pub type Message = (&'static str, &'static str);

/// The category a classified refusal is reported under: the scheduler's own, so that the
/// conversion into the host's failure is a move and never a second decision.
pub type ErrorKind = FailureKind;

/// A classified refusal, independent of the native (`rd_core::Failure`) and the WebAssembly
/// (WIT-generated) representation. `code` is a plugin constant, so a refusal cannot carry a code
/// the plugin's catalogue does not have.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApiFailure {
    pub kind: ErrorKind,
    pub code: &'static str,
    /// English, redaction-safe text.
    pub message: String,
    /// Flat parameters the translated text references, in the order they were added.
    pub params: Vec<(&'static str, String)>,
}

impl ApiFailure {
    /// `kind` under one of the plugin's `(code, message)` pairs, without parameters.
    #[must_use]
    pub fn new(kind: ErrorKind, (code, message): (&'static str, &str)) -> Self {
        Self {
            kind,
            code,
            message: message.to_owned(),
            params: Vec::new(),
        }
    }

    /// As [`ApiFailure::new`], carrying the provider's own error code as the `api_code`
    /// parameter: the stable code stays generic across a group of provider codes, and the
    /// parameter still says which one it was.
    #[must_use]
    pub fn with_api_code(
        kind: ErrorKind,
        message: (&'static str, &str),
        api_code: impl Display,
    ) -> Self {
        Self::new(kind, message).with_param("api_code", api_code.to_string())
    }

    /// Adds one parameter the translated text can reference.
    #[must_use]
    pub fn with_param(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.params.push((name, value.into()));
        self
    }

    /// The value of the first parameter called `name`.
    #[must_use]
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, value)| value.as_str())
    }
}

/// The mapping onto the failure the host reads; the WIT failure is the same record, one
/// conversion further on, in the guest crates.
impl From<ApiFailure> for Failure {
    fn from(failure: ApiFailure) -> Self {
        Self {
            kind: failure.kind,
            message: failure.message,
            code: Some(failure.code.to_owned()),
            params: failure
                .params
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value))
                .collect(),
        }
    }
}

/// A failure under one of the plugin's `(code, message)` pairs, for the logic that builds the
/// host's failure directly.
#[must_use]
pub fn coded(kind: FailureKind, (code, message): (&str, &str)) -> Failure {
    Failure::coded(kind, code, message)
}

/// A provider address that does not parse: `Permanent` under the plugin's `invalid_url` code,
/// with the parser's words as the `error` parameter. An [`ApiFailure`], because most callers
/// validate an address the API handed back; a resolver turns it into a [`Failure`] with `into`.
#[must_use]
pub fn invalid_url(code: &'static str, error: &dyn Display) -> ApiFailure {
    let text = format!("Invalid provider URL: {error}");
    ApiFailure::new(FailureKind::Permanent, (code, text.as_str()))
        .with_param("error", error.to_string())
}

/// The account a call needs, or `AuthRequired` under the plugin's `missing` words: an empty id
/// is no account either.
///
/// # Errors
///
/// `missing`, when there is no account or its id is empty.
pub fn require_account<'a>(
    account_id: Option<&'a str>,
    missing: (&str, &str),
) -> Result<&'a str, Failure> {
    account_id
        .filter(|id| !id.is_empty())
        .ok_or_else(|| coded(FailureKind::AuthRequired, missing))
}

/// The secret slot every call of a plugin needs, and the words it refuses with when the slot
/// is empty; a plugin declares it once, as a constant.
#[derive(Clone, Copy, Debug)]
pub struct SecretSlot {
    /// The reference the `{{secret:...}}` marker names.
    pub reference: &'static str,
    /// Reported `AuthRequired` when nothing is stored under `reference`.
    pub missing: Message,
}

/// Refuses before any request when the account holds nothing in `slot`.
///
/// Asked rather than assumed: without it the host would expand `{{secret:...}}` into nothing, and
/// the provider's `401` would read as "your sign-in expired" for an account that never had a
/// key or a token at all.
///
/// # Errors
///
/// The slot's `missing` words, `AuthRequired`, when the secret is not there.
pub async fn require_secret<H: PluginHost>(
    host: &H,
    account_id: &str,
    slot: SecretSlot,
) -> Result<(), Failure> {
    if host.secret_available(account_id, slot.reference).await {
        Ok(())
    } else {
        Err(coded(FailureKind::AuthRequired, slot.missing))
    }
}

/// A dead end a page explains: `kind` under the plugin's code, with the text built from the
/// page's own diagnosis and the diagnosis as the `diagnosis` parameter, so the failure names a
/// cause instead of being empty (RD-1110-03, audit R4).
#[must_use]
pub fn diagnosed(
    kind: FailureKind,
    code: &str,
    text: fn(&str) -> String,
    diagnosis: String,
) -> Failure {
    Failure::coded(kind, code, text(&diagnosis)).with_param("diagnosis", diagnosis)
}

/// A free download this IP may not start yet, from the wait a page stated: `None` is no limit,
/// `Some(0)` a limit without a duration, whose delay is left to the scheduler's own hold-off
/// rather than invented here. `IpBlocked`, so the scheduler holds back the hoster's other free
/// links instead of spending another wait and captcha on each of them.
///
/// # Errors
///
/// The limit under the plugin's code, with `wait_seconds` when the page named a duration.
pub fn free_limit(
    stated: Option<u64>,
    code: &str,
    text: fn(Option<u64>) -> String,
) -> Result<(), Failure> {
    let Some(seconds) = stated else {
        return Ok(());
    };
    let wait = (seconds > 0).then_some(seconds);
    let failure = Failure::coded(FailureKind::IpBlocked(wait), code, text(wait));
    Err(match wait {
        Some(seconds) => failure.with_param("wait_seconds", seconds.to_string()),
        None => failure,
    })
}

/// The code a plugin reports a status under when nothing more specific applies, and the
/// English text it builds from the status.
#[derive(Clone, Copy)]
pub struct HttpError {
    pub code: &'static str,
    pub text: fn(u16) -> String,
}

impl HttpError {
    /// `status` under this code, in `kind`, with the `status` parameter its text references.
    #[must_use]
    pub fn failure(&self, kind: ErrorKind, status: u16) -> ApiFailure {
        let text = (self.text)(status);
        ApiFailure::new(kind, (self.code, text.as_str())).with_param("status", status.to_string())
    }

    /// Classifies a status with [`http_status`] and reports every refusal under this one code,
    /// in its class's kind: for a provider whose body explains its refusals, so that the status
    /// says no more than the class.
    ///
    /// # Errors
    ///
    /// The classified refusal, for every status that is not a `2xx`.
    pub fn ensure_http_status(
        &self,
        status: u16,
        retry_after: Option<u64>,
    ) -> Result<(), ApiFailure> {
        http_status(status, retry_after).map_err(|refusal| self.failure(refusal.kind(), status))
    }

    /// As [`HttpError::ensure_http_status`], for a request that carried no account: a `401` or
    /// `403` is a plain `Permanent` HTTP error there, never `AccountInvalid` — no account was
    /// sent, so none was refused, and a bot wall's `403` read as a refused account sends the
    /// person to fix credentials they never gave (RA-PLG-01).
    ///
    /// # Errors
    ///
    /// The classified refusal, for every status that is not a `2xx`.
    pub fn ensure_without_account(
        &self,
        status: u16,
        retry_after: Option<u64>,
    ) -> Result<(), ApiFailure> {
        if matches!(status, 401 | 403) {
            return Err(self.failure(FailureKind::Permanent, status));
        }
        self.ensure_http_status(status, retry_after)
    }
}

/// The words a plugin reports each [`HttpRefusal`] class under, and the waits it falls back on
/// when the answer stated none. The classes and their kinds are [`http_status`]'s and the same
/// for every plugin (RD-191-07); only the naming is the plugin's.
#[derive(Clone, Copy)]
pub struct HttpWords {
    /// `401` and `403`: `AccountInvalid`.
    pub unauthorized: Message,
    /// `404` and `410`: `Permanent`.
    pub gone: Message,
    /// `451`: `Offline`.
    pub unavailable: Message,
    /// `429`: `RateLimited`.
    pub rate_limited: Message,
    /// Any `5xx`: `Transient`.
    pub server_error: Message,
    /// The wait of a `429` that stated none; `None` leaves it to the scheduler.
    pub rate_limited_wait: Option<u64>,
    /// The wait of a `5xx` that stated none; `None` leaves it to the scheduler.
    pub server_error_wait: Option<u64>,
    /// Every other status, `Permanent`, with a `status` parameter.
    pub other: HttpError,
}

impl HttpWords {
    /// One refusal class in this plugin's words.
    #[must_use]
    pub fn refusal(&self, refusal: HttpRefusal) -> ApiFailure {
        match refusal {
            HttpRefusal::Unauthorized => ApiFailure::new(refusal.kind(), self.unauthorized),
            HttpRefusal::Gone => ApiFailure::new(refusal.kind(), self.gone),
            HttpRefusal::Unavailable => ApiFailure::new(refusal.kind(), self.unavailable),
            HttpRefusal::RateLimited(wait) => ApiFailure::new(
                FailureKind::RateLimited(wait.or(self.rate_limited_wait)),
                self.rate_limited,
            ),
            HttpRefusal::ServerError(wait) => ApiFailure::new(
                FailureKind::Transient(wait.or(self.server_error_wait)),
                self.server_error,
            ),
            HttpRefusal::Other(status) => self.other.failure(refusal.kind(), status),
        }
    }

    /// Maps an HTTP status no document in the answer explains. `retry_after` is the
    /// response's `Retry-After`, which a `429` or a `5xx` carries into the wait.
    ///
    /// # Errors
    ///
    /// The classified refusal, for every status that is not a `2xx`.
    pub fn ensure_http_status(
        &self,
        status: u16,
        retry_after: Option<u64>,
    ) -> Result<(), ApiFailure> {
        http_status(status, retry_after).map_err(|refusal| self.refusal(refusal))
    }

    /// As [`HttpWords::ensure_http_status`], for a request that carried no account: a `401` or
    /// `403` is [`HttpWords::other`], `Permanent`, rather than a refused account (RA-PLG-01, see
    /// [`HttpError::ensure_without_account`]).
    ///
    /// # Errors
    ///
    /// The classified refusal, for every status that is not a `2xx`.
    pub fn ensure_without_account(
        &self,
        status: u16,
        retry_after: Option<u64>,
    ) -> Result<(), ApiFailure> {
        match http_status(status, retry_after) {
            Err(HttpRefusal::Unauthorized) => {
                Err(self.other.failure(FailureKind::Permanent, status))
            }
            result => result.map_err(|refusal| self.refusal(refusal)),
        }
    }
}

/// Sends one request and turns every answer `refused` names a refusal into the failure.
///
/// `refused` reads the status, the stated `Retry-After` (seconds only, never `0`, at most a
/// day: [`retry_after`]) and the body, and returns the refusal they describe, if any. It is the
/// one place a plugin decides what an answer means, so a caller gets the answer or a failure
/// and never decides a second time what a status code means.
///
/// # Errors
///
/// The host's failure to make the request, or the refusal `refused` named.
pub async fn call<H: PluginHost>(
    host: &H,
    request: HttpRequest,
    refused: impl FnOnce(u16, Option<u64>, &[u8]) -> Option<ApiFailure>,
) -> Result<HttpResponse, Failure> {
    let response = host.http(request).await?;
    let retry_after = retry_after(&response.headers);
    match refused(response.status, retry_after, &response.body) {
        Some(failure) => Err(failure.into()),
        None => Ok(response),
    }
}

#[cfg(test)]
mod tests;
