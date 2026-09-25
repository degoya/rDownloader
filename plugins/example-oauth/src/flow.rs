//! Reading what a token endpoint answered.
//!
//! Kept apart from the component so it can be unit-tested on the host target: `cargo test`
//! in a fresh scaffold runs these without a WebAssembly toolchain.
//!
//! The same four answers serve both ways in (RD-106-01): a device poll reads the very same
//! token endpoint, and `authorization_pending` is a `Busy` for the same reason `slow_down` is
//! -- the person simply has not finished yet, and nothing is wrong with anything.
//!
//! The four answers below are the four the host reacts to differently, which is why they are
//! four and not one:
//!
//! - `Granted` — store the token, report `authorized`.
//! - `Refused` — the provider said no and will keep saying no. It becomes `failed`, and the
//!   person is told to sign in again.
//! - `Busy` — a rate limit. It becomes `pending`, and the host waits the given number of
//!   seconds. Never a failure: nothing is wrong with the credential.
//! - `Unreadable` — an answer this plugin does not understand. Treated as a refusal rather
//!   than as success, because reporting `authorized` without a stored token would leave an
//!   account that looks signed in and cannot download anything.
//!
//! A provider that could not be reached at all never gets here: `http-request` fails, the
//! guest returns that failure, and the host keeps the stored token and tries again later.

use crate::pkce;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenAnswer {
    Granted {
        access_token: String,
        refresh_token: Option<String>,
        expires_in_seconds: Option<u64>,
    },
    /// The provider's own `error` code, e.g. `access_denied` or `invalid_grant`.
    Refused(String),
    /// Too many requests; wait this many seconds if the provider said how long.
    Busy(Option<u64>),
    Unreadable(u16),
}

/// The translation code a refusal is reported under, so the interface can say it in the
/// language the person reads. `example_oauth` is this plugin's slug; the catalogue in
/// `locales/` has to carry each of these.
#[must_use]
pub fn refusal_code(error: &str) -> &'static str {
    match error {
        "access_denied" | "consent_required" | "interaction_required" => "consent_denied",
        // `authorization_pending` is deliberately absent: since RD-106-01 it never reaches
        // here, because a device flow that has not been confirmed yet is waiting, not
        // refused. `read_token_answer` turns it into `Busy` before this is consulted.
        "expired_token" | "invalid_request" => "code_expired",
        _ => "refresh_refused",
    }
}

/// What a device-authorization endpoint answered (RD-106-01).
///
/// The device flow's counterpart of the authorization URL `begin` builds: here the provider,
/// not the plugin, decides what the person sees.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceCode {
    /// The value the next poll is made with. Never shown to anybody, so it travels in
    /// `flow-state` rather than in anything the interface renders.
    pub device_code: String,
    /// The short code the person types at `verification_url`.
    pub user_code: String,
    /// Where they type it. Goes back as the provider gave it and is refused by the host
    /// unless it is on a domain this plugin's manifest declares.
    pub verification_url: String,
    pub expires_in: Option<u64>,
    /// How long the provider asked to be left alone between polls.
    pub interval: Option<u64>,
}

/// Reads a device-authorization answer, or `None` when it is not one.
///
/// RFC 8628 spells the address `verification_uri`; enough providers write `verification_url`
/// that both are read. An answer missing any of the three required values is `None` rather
/// than a half-filled prompt: a sign-in without a code is a screen nobody can act on.
#[must_use]
pub fn read_device_code(body: &str) -> Option<DeviceCode> {
    let device_code = pkce::string_field(body, "device_code")?;
    let user_code = pkce::string_field(body, "user_code")?;
    let verification_url = pkce::string_field(body, "verification_uri")
        .or_else(|| pkce::string_field(body, "verification_url"))?;
    (!device_code.is_empty() && !user_code.is_empty() && !verification_url.is_empty()).then(|| {
        DeviceCode {
            device_code,
            user_code,
            verification_url,
            expires_in: pkce::number_field(body, "expires_in"),
            interval: pkce::number_field(body, "interval"),
        }
    })
}

/// Reads a token or refresh answer.
///
/// `retry_after` is the `Retry-After` response header, which is where a provider says how long
/// to wait; the body is consulted only when the header is missing.
#[must_use]
pub fn read_token_answer(status: u16, retry_after: Option<&str>, body: &str) -> TokenAnswer {
    let error = pkce::string_field(body, "error");
    // Waiting is not refusal even when it arrives with an `error` field, so it is read first.
    // `slow_down` is a rate limit spelled in the body; `authorization_pending` is a device
    // flow saying the person has not finished at the other screen yet. Reading either as a
    // failure would end a sign-in that was going perfectly well.
    if status == 429
        || matches!(
            error.as_deref(),
            Some("slow_down" | "authorization_pending")
        )
    {
        let seconds = retry_after
            .and_then(|value| value.trim().parse::<u64>().ok())
            .or_else(|| pkce::number_field(body, "retry_after"))
            .or_else(|| pkce::number_field(body, "interval"));
        return TokenAnswer::Busy(seconds);
    }
    if let Some(error) = error {
        return TokenAnswer::Refused(error);
    }
    match pkce::string_field(body, "access_token") {
        Some(access_token) if !access_token.is_empty() => TokenAnswer::Granted {
            access_token,
            refresh_token: pkce::string_field(body, "refresh_token").filter(|t| !t.is_empty()),
            expires_in_seconds: pkce::number_field(body, "expires_in"),
        },
        _ => TokenAnswer::Unreadable(status),
    }
}

/// A provider's `error` code, reduced to something that is safe to put in a message.
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
mod tests {
    use super::*;

    #[test]
    fn a_granted_exchange_yields_the_three_values_the_host_stores() {
        let answer = read_token_answer(
            200,
            None,
            r#"{"access_token":"AT","refresh_token":"RT","expires_in":3600,"token_type":"Bearer"}"#,
        );
        assert_eq!(
            answer,
            TokenAnswer::Granted {
                access_token: "AT".to_owned(),
                refresh_token: Some("RT".to_owned()),
                expires_in_seconds: Some(3600),
            }
        );
    }

    #[test]
    fn a_refused_consent_is_a_refusal_and_not_a_rate_limit() {
        assert_eq!(
            read_token_answer(400, None, r#"{"error":"access_denied"}"#),
            TokenAnswer::Refused("access_denied".to_owned())
        );
        assert_eq!(refusal_code("access_denied"), "consent_denied");
    }

    #[test]
    fn an_expired_code_and_a_refused_refresh_are_told_apart() {
        assert_eq!(refusal_code("expired_token"), "code_expired");
        assert_eq!(refusal_code("invalid_grant"), "refresh_refused");
    }

    /// The device flow's "not yet" is a wait, never a refusal — the difference between a
    /// sign-in that finishes and one that dies while the person is still reading the code.
    #[test]
    fn an_unconfirmed_device_code_keeps_the_flow_open() {
        assert_eq!(
            read_token_answer(400, None, r#"{"error":"authorization_pending"}"#),
            TokenAnswer::Busy(None)
        );
        assert_eq!(
            read_token_answer(
                400,
                None,
                r#"{"error":"authorization_pending","interval":7}"#
            ),
            TokenAnswer::Busy(Some(7))
        );
        // And what a device flow gives up on still gives up.
        assert_eq!(
            read_token_answer(400, None, r#"{"error":"expired_token"}"#),
            TokenAnswer::Refused("expired_token".to_owned())
        );
    }

    #[test]
    fn a_device_authorization_answer_is_read_whole() {
        let body = r#"{"device_code":"DC-1","user_code":"WXYZ-1234",
          "verification_uri":"https:\/\/oauth.example.invalid\/device",
          "expires_in":900,"interval":5}"#;
        assert_eq!(
            read_device_code(body),
            Some(DeviceCode {
                device_code: "DC-1".to_owned(),
                user_code: "WXYZ-1234".to_owned(),
                verification_url: "https://oauth.example.invalid/device".to_owned(),
                expires_in: Some(900),
                interval: Some(5),
            })
        );
        // The spelling half the providers use, and the only other one accepted.
        let legacy = r#"{"device_code":"DC-1","user_code":"WX",
          "verification_url":"https:\/\/oauth.example.invalid\/device"}"#;
        assert_eq!(
            read_device_code(legacy).map(|code| code.verification_url),
            Some("https://oauth.example.invalid/device".to_owned())
        );
        // A prompt nobody could act on is not a prompt.
        assert_eq!(read_device_code(r#"{"device_code":"DC-1"}"#), None);
        assert_eq!(
            read_device_code(
                r#"{"device_code":"","user_code":"WX","verification_uri":"https://x"}"#
            ),
            None
        );
    }

    #[test]
    fn a_rate_limit_reads_retry_after_before_the_body() {
        assert_eq!(
            read_token_answer(429, Some("42"), r#"{"error":"slow_down","interval":5}"#),
            TokenAnswer::Busy(Some(42))
        );
        assert_eq!(
            read_token_answer(200, None, r#"{"error":"slow_down","interval":5}"#),
            TokenAnswer::Busy(Some(5))
        );
        assert_eq!(read_token_answer(429, None, "{}"), TokenAnswer::Busy(None));
    }

    #[test]
    fn an_answer_without_a_token_is_never_read_as_success() {
        assert_eq!(
            read_token_answer(200, None, r#"{"token_type":"Bearer"}"#),
            TokenAnswer::Unreadable(200)
        );
        assert_eq!(
            read_token_answer(200, None, r#"{"access_token":""}"#),
            TokenAnswer::Unreadable(200)
        );
    }

    #[test]
    fn a_providers_error_text_never_travels_verbatim() {
        assert_eq!(sanitize_error("invalid_grant"), "invalid_grant");
        // An endpoint that echoes a token into its error document publishes nothing here:
        // the value is not an error code, so none of it survives.
        assert_eq!(sanitize_error("token AT-7f3c9 rejected"), "refused");
        assert_eq!(sanitize_error("<html>500</html>"), "refused");
        assert_eq!(sanitize_error(""), "refused");
        assert_eq!(sanitize_error(&"x".repeat(200)), "refused");
    }
}
