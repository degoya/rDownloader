//! Reading what Real-Debrid's OAuth2 endpoints answered.
//!
//! Kept apart from the component so it can be unit-tested on the host target: `cargo test` here
//! runs without a WebAssembly toolchain.
//!
//! Real-Debrid speaks two error vocabularies at once, and both have to be read:
//!
//! - the RFC 6749/8628 words in `error` — `authorization_pending`, `slow_down`,
//!   `access_denied`, `expired_token`, `invalid_client`;
//! - its own documented `error_code` numbers, of which `5` ("slow down") and `34` ("too many
//!   requests") are the ones an OAuth exchange can meet.
//!
//! The four answers below are the four a host reacts to differently:
//!
//! - `Granted` — store the token, report `authorized`.
//! - `Refused` — the provider said no and will keep saying no. It becomes `failed`.
//! - `Busy` — nobody has confirmed yet, or the request budget is spent. It becomes `pending`,
//!   and the host waits. Never a failure: nothing is wrong with anything.
//! - `Unreadable` — an answer this plugin does not understand. Treated as a refusal rather than
//!   as success, because reporting `authorized` without a stored token would leave an account
//!   that looks signed in and cannot download anything.
//!
//! A provider that could not be reached at all never gets here: `http-request` fails, the guest
//! returns that failure, and the host keeps the stored token and tries again later.

use crate::json;

/// The `error_code` numbers Real-Debrid answers an OAuth request with that mean *wait*, not
/// *no*: 5 is "slow down" and 34 is "too many requests". Both count towards the very cap that
/// produced them, so asking faster is the one thing that must not happen.
const WAITING_CODES: [u64; 2] = [5, 34];

/// The floor a wait falls back to when the provider named none.
const DEFAULT_WAIT: u64 = 5;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenAnswer {
    Granted {
        access_token: String,
        refresh_token: Option<String>,
        expires_in_seconds: Option<u64>,
    },
    /// The provider's own refusal: its `error` word, and its `error_code` number when it sent
    /// one. Neither is ever put in front of a person as it stands; see [`sanitize_error`].
    Refused {
        error: String,
        api_code: Option<u64>,
    },
    /// Not finished, or asked too often. Wait this many seconds.
    Busy(u64),
    Unreadable(u16),
}

/// The translation code a refusal is reported under, so the interface can say it in the
/// language the person reads. `realdebrid_auth` is this plugin's slug; the catalogue in
/// `locales/` carries each of these.
///
/// `invalid_client` gets its own code on purpose. It is the one refusal that is not about the
/// person at all — it says this build's application registration is not accepted — and telling
/// somebody to sign in again would send them round a loop that cannot end.
#[must_use]
pub fn refusal_code(error: &str, api_code: Option<u64>) -> &'static str {
    match error {
        "access_denied" | "consent_required" | "interaction_required" => "consent_denied",
        // `authorization_pending` and `slow_down` are deliberately absent: they never reach
        // here, because `read_token_answer` turns both into `Busy` before this is consulted.
        "expired_token" | "invalid_request" | "invalid_grant" => "code_expired",
        "invalid_client" | "unauthorized_client" => "client_rejected",
        // Real-Debrid also refuses by number alone, so the documented ones are read too.
        // 10 and 11 are the second factor, which is the one refusal a person can actually do
        // something about right now -- and telling them to sign in again instead would send
        // them back to the same wall.
        _ => match api_code {
            Some(10 | 11) => "two_factor",
            _ => "sign_in_refused",
        },
    }
}

/// What a device-authorization endpoint answered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceCode {
    /// The value the next poll is made with. Never shown to anybody, so it travels in
    /// `flow-state` rather than in anything the interface renders.
    pub device_code: String,
    /// The short code the person types at `verification_url`.
    pub user_code: String,
    /// Where they type it. Goes back as the provider gave it and is refused by the host unless
    /// it is on a domain this plugin's manifest declares.
    pub verification_url: String,
    pub expires_in: Option<u64>,
    /// How long the provider asked to be left alone between polls.
    pub interval: Option<u64>,
}

/// Reads a device-authorization answer, or `None` when it is not one.
///
/// RFC 8628 spells the address `verification_uri` and Real-Debrid spells it `verification_url`;
/// both are read. An answer missing any of the three required values is `None` rather than a
/// half-filled prompt: a sign-in without a code is a screen nobody can act on.
#[must_use]
pub fn read_device_code(body: &str) -> Option<DeviceCode> {
    let device_code = json::string_field(body, "device_code")?;
    let user_code = json::string_field(body, "user_code")?;
    let verification_url = json::string_field(body, "verification_uri")
        .or_else(|| json::string_field(body, "verification_url"))?;
    (!device_code.is_empty() && !user_code.is_empty() && !verification_url.is_empty()).then(|| {
        DeviceCode {
            device_code,
            user_code,
            verification_url,
            expires_in: json::number_field(body, "expires_in"),
            interval: json::number_field(body, "interval"),
        }
    })
}

/// Reads a token or refresh answer.
///
/// `retry_after` is the `Retry-After` response header, which is where a provider says how long
/// to wait; the body is consulted only when the header is missing.
#[must_use]
pub fn read_token_answer(status: u16, retry_after: Option<&str>, body: &str) -> TokenAnswer {
    let error = json::string_field(body, "error");
    let api_code = json::number_field(body, "error_code");
    // Waiting is read first, even when it arrives with an `error` field. A device flow that
    // read "not yet" as failure would end sign-ins while the person was still walking to the
    // other screen -- and a rate limit says nothing about the credential either.
    let waiting = status == 429
        || matches!(
            error.as_deref(),
            Some("slow_down" | "authorization_pending")
        )
        || api_code.is_some_and(|code| WAITING_CODES.contains(&code));
    if waiting {
        let seconds = retry_after
            .and_then(|value| value.trim().parse::<u64>().ok())
            .or_else(|| json::number_field(body, "interval"))
            .unwrap_or(DEFAULT_WAIT);
        return TokenAnswer::Busy(seconds);
    }
    if let Some(error) = error {
        return TokenAnswer::Refused { error, api_code };
    }
    // A number with no word is still a refusal. Real-Debrid answers some of them that way, and
    // reading only `error` would take one for a success with no token in it.
    if let Some(api_code) = api_code {
        return TokenAnswer::Refused {
            error: String::new(),
            api_code: Some(api_code),
        };
    }
    match json::string_field(body, "access_token") {
        Some(access_token) if !access_token.is_empty() => TokenAnswer::Granted {
            access_token,
            refresh_token: json::string_field(body, "refresh_token").filter(|t| !t.is_empty()),
            expires_in_seconds: json::number_field(body, "expires_in"),
        },
        _ => TokenAnswer::Unreadable(status),
    }
}

/// A provider's `error` word, reduced to something that is safe to put in a message.
///
/// The point is not tidiness. Whatever a provider sends back travels into a log line and into
/// the failure the interface shows, and an endpoint that echoed part of a token into its error
/// document would otherwise publish it. RFC 6749 error codes are lowercase words joined by
/// underscores, so anything that is not exactly that shape is dropped whole rather than
/// filtered character by character -- filtering would keep the digits of a leaked token.
#[must_use]
pub fn sanitize_error(error: &str) -> String {
    let trimmed = error.trim();
    let is_error_code = !trimmed.is_empty()
        && trimmed.len() <= 40
        && trimmed.chars().all(|c| c.is_ascii_lowercase() || c == '_');
    if is_error_code {
        trimmed.to_owned()
    } else {
        "refused".to_owned()
    }
}

#[cfg(test)]
#[path = "flow/tests.rs"]
mod tests;
