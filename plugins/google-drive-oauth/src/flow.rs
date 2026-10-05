//! Reading what Google's token endpoint answered.
//!
//! Kept apart from the component so it can be unit-tested on the host target: `cargo test` runs
//! these without a WebAssembly toolchain.
//!
//! The three answers below are the three the host reacts to differently, which is why they are
//! three and not one:
//!
//! - `Granted` — store the token, report `authorized`.
//! - `Refused` — Google said no and will keep saying no. It becomes `failed`, and the person is
//!   told to sign in again.
//! - `Busy` — a rate limit. It becomes `pending`, and the host waits the given number of
//!   seconds. Never a failure: nothing is wrong with the credential.
//! - `Unreadable` — an answer this plugin does not understand. Treated as a refusal rather than
//!   as success, because reporting `authorized` without a stored token would leave an account
//!   that looks signed in and cannot download anything.
//!
//! A provider that could not be reached at all never gets here: `http-request` fails, the guest
//! returns that failure, and the host keeps the stored token and tries again later.

pub use plugin_guest_oauth::token::TokenAnswer;
use plugin_guest_oauth::token::{self, Waiting};

/// The translation code a refusal is reported under, so the interface can say it in the
/// language the person reads. `google_drive_oauth` is this plugin's slug; the catalogue in
/// `locales/` carries each of these.
///
/// One code beyond what the SDK template maps, and it is the one Google produces most: a
/// refresh token that has been revoked, expired after six months of disuse, or invalidated by a
/// password change all come back as `invalid_grant`. It is the difference between "sign in
/// again" and "something went wrong".
#[must_use]
pub fn refusal_code(error: &str) -> &'static str {
    match error {
        "access_denied" | "consent_required" | "interaction_required" | "admin_policy_enforced" => {
            "consent_denied"
        }
        "expired_token" | "invalid_request" => "code_expired",
        // Google's catch-all for "this grant is no longer good", and the terminal one.
        "invalid_grant" | "unauthorized_client" | "invalid_client" => "refresh_refused",
        _ => "refresh_refused",
    }
}

/// What this provider's token answer says "wait" with, besides HTTP 429.
const WAITING: Waiting = Waiting {
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
/// Google's `error_description` is deliberately never read at all. It is a full English
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
            r#"{"access_token":"AT","expires_in":3599,"refresh_token":"RT",
                "scope":"https://www.googleapis.com/auth/drive.readonly","token_type":"Bearer"}"#,
        );
        assert_eq!(
            answer,
            TokenAnswer::Granted {
                access_token: "AT".to_owned(),
                refresh_token: Some("RT".to_owned()),
                expires_in_seconds: Some(3599),
            }
        );
    }

    /// Google hands refresh material over on the first exchange and never again, so a renewal's
    /// answer legitimately carries none. That is not a failure and must not read as one.
    #[test]
    fn a_renewal_answer_without_refresh_material_is_still_a_grant() {
        assert_eq!(
            read_token_answer(200, None, r#"{"access_token":"AT2","expires_in":3599}"#),
            TokenAnswer::Granted {
                access_token: "AT2".to_owned(),
                refresh_token: None,
                expires_in_seconds: Some(3599),
            }
        );
    }

    #[test]
    fn the_refusals_google_actually_sends_are_told_apart() {
        assert_eq!(
            read_token_answer(400, None, r#"{"error":"access_denied"}"#),
            TokenAnswer::Refused("access_denied".to_owned())
        );
        assert_eq!(refusal_code("access_denied"), "consent_denied");
        // What a Workspace administrator's policy comes back as.
        assert_eq!(refusal_code("admin_policy_enforced"), "consent_denied");
        assert_eq!(refusal_code("invalid_request"), "code_expired");
        // The one Google produces most: revoked, unused for six months, or a changed password.
        assert_eq!(refusal_code("invalid_grant"), "refresh_refused");
    }

    #[test]
    fn a_rate_limit_reads_retry_after_before_the_body() {
        assert_eq!(
            read_token_answer(429, Some("42"), r#"{"error":"slow_down","interval":5}"#),
            TokenAnswer::Busy(Some(42))
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

    /// Google's error documents carry an `error_description` written as an English sentence.
    /// None of it survives, and neither does anything that sentence happened to quote.
    #[test]
    fn a_providers_error_text_never_travels_verbatim() {
        assert_eq!(sanitize_error("invalid_grant"), "invalid_grant");
        assert_eq!(
            sanitize_error("Token has been expired or revoked."),
            "refused"
        );
        assert_eq!(sanitize_error("token ya29.a0AfB_xyz rejected"), "refused");
        assert_eq!(sanitize_error("<html>500</html>"), "refused");
        assert_eq!(sanitize_error(""), "refused");
        assert_eq!(sanitize_error(&"x".repeat(200)), "refused");
    }
}
