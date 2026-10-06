//! Reading what a token endpoint answered.
//!
//! Kept apart from the component so it can be unit-tested on the host target: `cargo test`
//! in a fresh scaffold runs these without a WebAssembly toolchain.
//!
//! The same four answers serve both ways in (RD-106-01): a device poll reads the very same
//! token endpoint, and `authorization_pending` is a `Busy` for the same reason `slow_down` is
//! -- the person simply has not finished yet, and nothing is wrong with anything.
//!
//! What the four answers mean to the host, and why an unreachable provider is none of them,
//! is `plugin_guest_oauth::token`'s to say; the flow that acts on them is
//! `plugin_guest_oauth::redirect`'s. What is the provider's own stays here: its refusal codes
//! and what it says "wait" with, which `guest` hands to the flow.

use plugin_guest_oauth::token::{self, Waiting};
pub use plugin_guest_oauth::token::{DeviceCode, TokenAnswer, read_device_code};

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

/// What this provider's token answer says "wait" with, besides HTTP 429.
pub const WAITING: Waiting = Waiting {
    errors: &["slow_down", "authorization_pending"],
    fields: &["retry_after", "interval"],
};

/// Reads a token or refresh answer; the reading is `plugin-guest-oauth`'s, the [`WAITING`]
/// this provider's.
///
/// `retry_after` is the `Retry-After` response header, which is where a provider says how long
/// to wait; the body is consulted only when the header is missing.
#[must_use]
pub fn read_token_answer(status: u16, retry_after: Option<&str>, body: &str) -> TokenAnswer {
    token::read_token_answer(&WAITING, status, retry_after, body)
}

/// A provider's `error` code, reduced to something that is safe to put in a message: the one
/// rule every OAuth plugin applies, in `plugin-common` (RD-1110-04).
pub use plugin_common::device_flow::sanitize_error;

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
        // The header is read by the reader every plugin shares (RD-191-07): `0` is no wait, so
        // the body's figure answers, and a year is the host's one-day ceiling.
        assert_eq!(
            read_token_answer(429, Some("0"), r#"{"interval":5}"#),
            TokenAnswer::Busy(Some(5))
        );
        assert_eq!(
            read_token_answer(429, Some("31536000"), "{}"),
            TokenAnswer::Busy(Some(86_400))
        );
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
