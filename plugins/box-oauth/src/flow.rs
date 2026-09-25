//! Reading what Box's token endpoint answered.
//!
//! Kept apart from the component so it can be unit-tested on the host target: `cargo test` runs
//! these without a WebAssembly toolchain.
//!
//! The answers below are the ones the host reacts to differently, which is why they are four
//! and not one:
//!
//! - `Granted` — store the token, report `authorized`.
//! - `Refused` — Box said no and will keep saying no. It becomes `failed`, and the person is
//!   told to sign in again.
//! - `Busy` — a rate limit. It becomes `pending`, and the host waits the given number of
//!   seconds. Never a failure: nothing is wrong with the credential.
//! - `Unreadable` — an answer this plugin does not understand. Treated as a refusal rather than
//!   as success, because reporting `authorized` without a stored token would leave an account
//!   that looks signed in and cannot download anything.
//!
//! A provider that could not be reached at all never gets here: `http-request` fails, the guest
//! returns that failure, and the host keeps the stored token and tries again later.

use crate::pkce;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenAnswer {
    Granted {
        access_token: String,
        refresh_token: Option<String>,
        expires_in_seconds: Option<u64>,
    },
    /// Box's own `error` code, e.g. `invalid_grant` or `unauthorized_client`.
    Refused(String),
    /// Too many requests; wait this many seconds if Box said how long.
    Busy(Option<u64>),
    Unreadable(u16),
}

/// The translation code a refusal is reported under, so the interface can say it in the
/// language the person reads. `box_oauth` is this plugin's slug; the catalogue in `locales/`
/// carries each of these.
///
/// Box answers in RFC 6749's vocabulary. `invalid_grant` is the one that ends a renewal — a
/// refresh token Box rotated away, one left unused past its sixty days, or an application the
/// person removed — and `invalid_client` is what a mistyped client secret comes back as, which
/// is the failure this provider has and the other three do not.
#[must_use]
pub fn refusal_code(error: &str) -> &'static str {
    match error {
        "access_denied" => "consent_denied",
        "invalid_request" | "unsupported_grant_type" => "code_expired",
        "invalid_client" | "unauthorized_client" => "client_refused",
        "invalid_grant" => "refresh_refused",
        _ => "refresh_refused",
    }
}

/// Reads a token or refresh answer.
///
/// `retry_after` is the `Retry-After` response header, which is where a provider says how long
/// to wait; the body is consulted only when the header is missing.
#[must_use]
pub fn read_token_answer(status: u16, retry_after: Option<&str>, body: &str) -> TokenAnswer {
    let error = pkce::string_field(body, "error");
    // Waiting is not refusal even when it arrives with an `error` field, so it is read first.
    // Reading a rate limit as a failure would end a sign-in that was going perfectly well.
    if status == 429 || error.as_deref() == Some("slow_down") {
        let seconds = retry_after
            .and_then(|value| value.trim().parse::<u64>().ok())
            .or_else(|| pkce::number_field(body, "retry_after"));
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

/// Box's `error` code, reduced to something that is safe to put in a message.
///
/// The point is not tidiness. Whatever a provider sends back travels into a log line and into
/// the failure the interface shows, and an endpoint that echoed part of a token into its error
/// document would otherwise publish it. RFC 6749 error codes are lowercase words joined by
/// underscores, so anything that is not exactly that shape is dropped whole rather than
/// filtered character by character — filtering would keep the digits of a leaked token.
///
/// Box's `error_description` is deliberately never read at all. It is a full sentence written
/// for a developer, and there is no shape check that makes a sentence safe.
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
    use super::{TokenAnswer, read_token_answer, refusal_code, sanitize_error};

    /// The answer Box's token endpoint actually sends, down to the `restricted_to` field it
    /// carries beside the tokens.
    #[test]
    fn a_granted_exchange_yields_the_three_values_the_host_stores() {
        let answer = read_token_answer(
            200,
            None,
            r#"{"access_token":"AT","expires_in":4245,"restricted_to":[],
                "refresh_token":"RT","token_type":"bearer"}"#,
        );
        assert_eq!(
            answer,
            TokenAnswer::Granted {
                access_token: "AT".to_owned(),
                refresh_token: Some("RT".to_owned()),
                expires_in_seconds: Some(4245),
            }
        );
    }

    /// Box rotates refresh material on every renewal and the old one stops working, so a
    /// renewal that answered without one is still a grant — the account keeps the token it just
    /// got, and reading the absence as a failure would end a sign-in that succeeded.
    #[test]
    fn a_renewal_answer_without_refresh_material_is_still_a_grant() {
        assert_eq!(
            read_token_answer(200, None, r#"{"access_token":"AT2","expires_in":4245}"#),
            TokenAnswer::Granted {
                access_token: "AT2".to_owned(),
                refresh_token: None,
                expires_in_seconds: Some(4245),
            }
        );
    }

    #[test]
    fn the_refusals_box_actually_sends_are_told_apart() {
        assert_eq!(
            read_token_answer(400, None, r#"{"error":"invalid_grant"}"#),
            TokenAnswer::Refused("invalid_grant".to_owned())
        );
        assert_eq!(refusal_code("access_denied"), "consent_denied");
        // The one this provider has and the other three do not: a mistyped client secret.
        assert_eq!(refusal_code("invalid_client"), "client_refused");
        assert_eq!(refusal_code("unauthorized_client"), "client_refused");
        // A code that was already used, or was left the thirty seconds Box allows.
        assert_eq!(refusal_code("invalid_request"), "code_expired");
        // The one that ends a renewal: rotated away, unused past its sixty days, or revoked.
        assert_eq!(refusal_code("invalid_grant"), "refresh_refused");
    }

    #[test]
    fn a_rate_limit_is_a_wait_and_reads_retry_after_before_the_body() {
        assert_eq!(
            read_token_answer(429, Some("42"), r#"{"error":"rate_limit_exceeded"}"#),
            TokenAnswer::Busy(Some(42))
        );
        assert_eq!(read_token_answer(429, None, "{}"), TokenAnswer::Busy(None));
    }

    #[test]
    fn an_answer_without_a_token_is_never_read_as_success() {
        assert_eq!(
            read_token_answer(200, None, r#"{"token_type":"bearer"}"#),
            TokenAnswer::Unreadable(200)
        );
        assert_eq!(
            read_token_answer(200, None, r#"{"access_token":""}"#),
            TokenAnswer::Unreadable(200)
        );
    }

    /// Box's error documents carry an `error_description` written for a developer. None of it
    /// survives, and neither does anything that sentence happened to quote.
    #[test]
    fn a_providers_error_text_never_travels_verbatim() {
        assert_eq!(sanitize_error("invalid_grant"), "invalid_grant");
        assert_eq!(
            sanitize_error("Refresh token has expired: 1!vcS2tGHMnv8 was issued 2026-07-01"),
            "refused"
        );
        assert_eq!(sanitize_error("<html>500</html>"), "refused");
        assert_eq!(sanitize_error(""), "refused");
        assert_eq!(sanitize_error(&"x".repeat(200)), "refused");
    }
}
