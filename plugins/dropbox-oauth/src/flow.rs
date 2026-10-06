//! Reading what Dropbox's token endpoint answered.
//!
//! Kept apart from the component so it can be unit-tested on the host target: `cargo test` runs
//! these without a WebAssembly toolchain.
//!
//! What the four answers mean to the host, and why an unreachable provider is none of them,
//! is `plugin_guest_oauth::token`'s to say; the flow that acts on them is
//! `plugin_guest_oauth::redirect`'s. What is the provider's own stays here: its refusal codes
//! and what it says "wait" with, which `guest` hands to the flow.

pub use plugin_guest_oauth::token::TokenAnswer;
use plugin_guest_oauth::token::{self, Waiting};

/// The translation code a refusal is reported under, so the interface can say it in the
/// language the person reads. `dropbox_oauth` is this plugin's slug; the catalogue in
/// `locales/` carries each of these.
///
/// `invalid_grant` is the one Dropbox produces most: a code used twice or too late, and a
/// refresh token revoked when the person unlinked the app or the app key changed. It is the
/// difference between "sign in again" and "something went wrong".
#[must_use]
pub fn refusal_code(error: &str) -> &'static str {
    match error {
        "access_denied" | "consent_required" => "consent_denied",
        "expired_token" | "invalid_request" => "code_expired",
        "invalid_grant" | "unauthorized_client" | "invalid_client" | "invalid_scope" => {
            "refresh_refused"
        }
        _ => "refresh_refused",
    }
}

/// What this provider's token answer says "wait" with, besides HTTP 429.
pub const WAITING: Waiting = Waiting {
    errors: &["slow_down"],
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
///
/// Dropbox's `error_description` is deliberately never read at all. It is a full English
/// sentence written for a developer, and there is no shape check that makes a sentence safe.
pub use plugin_common::device_flow::sanitize_error;

#[cfg(test)]
mod tests {
    use super::{TokenAnswer, read_token_answer, refusal_code, sanitize_error};

    #[test]
    fn a_granted_exchange_yields_the_three_values_the_host_stores() {
        let answer = read_token_answer(
            200,
            None,
            r#"{"access_token":"AT","token_type":"bearer","expires_in":14400,
                "refresh_token":"RT","scope":"files.content.read files.metadata.read",
                "uid":"12345","account_id":"dbid:redacted"}"#,
        );
        assert_eq!(
            answer,
            TokenAnswer::Granted {
                access_token: "AT".to_owned(),
                refresh_token: Some("RT".to_owned()),
                expires_in_seconds: Some(14_400),
            }
        );
    }

    /// Dropbox hands refresh material over on the first exchange and never on a renewal, so a
    /// renewal's answer legitimately carries none. That is not a failure and must not read as
    /// one.
    #[test]
    fn a_renewal_answer_without_refresh_material_is_still_a_grant() {
        assert_eq!(
            read_token_answer(
                200,
                None,
                r#"{"access_token":"AT2","token_type":"bearer","expires_in":14400}"#
            ),
            TokenAnswer::Granted {
                access_token: "AT2".to_owned(),
                refresh_token: None,
                expires_in_seconds: Some(14_400),
            }
        );
    }

    #[test]
    fn the_refusals_dropbox_actually_sends_are_told_apart() {
        assert_eq!(
            read_token_answer(400, None, r#"{"error":"access_denied"}"#),
            TokenAnswer::Refused("access_denied".to_owned())
        );
        assert_eq!(refusal_code("access_denied"), "consent_denied");
        assert_eq!(refusal_code("invalid_request"), "code_expired");
        // The one Dropbox produces most: a used code, or a refresh token revoked by unlinking.
        assert_eq!(refusal_code("invalid_grant"), "refresh_refused");
        assert_eq!(refusal_code("invalid_client"), "refresh_refused");
    }

    #[test]
    fn a_rate_limit_reads_retry_after_before_the_body() {
        assert_eq!(
            read_token_answer(429, Some("42"), r#"{"error":"slow_down","retry_after":5}"#),
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

    /// Dropbox's error documents carry an `error_description` written as an English sentence.
    /// None of it survives, and neither does anything that sentence happened to quote.
    #[test]
    fn a_providers_error_text_never_travels_verbatim() {
        assert_eq!(sanitize_error("invalid_grant"), "invalid_grant");
        assert_eq!(
            sanitize_error("code has expired (within the last hour)"),
            "refused"
        );
        assert_eq!(sanitize_error("token sl.u.AFx rejected"), "refused");
        assert_eq!(sanitize_error("<html>500</html>"), "refused");
        assert_eq!(sanitize_error(""), "refused");
        assert_eq!(sanitize_error(&"x".repeat(200)), "refused");
    }
}
