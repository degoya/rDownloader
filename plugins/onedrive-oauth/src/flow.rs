//! Reading what Microsoft's token and device-code endpoints answered.
//!
//! Kept apart from the component so it can be unit-tested on the host target: `cargo test` runs
//! these without a WebAssembly toolchain.
//!
//! The answers below are the ones the host reacts to differently, which is why they are four
//! and not one:
//!
//! - `Granted` — store the token, report `authorized`.
//! - `Refused` — Microsoft said no and will keep saying no. It becomes `failed`, and the
//!   person is told to sign in again.
//! - `Busy` — a rate limit, or a device sign-in the person has not finished at the other
//!   screen. It becomes `pending`, and the host waits the given number of seconds. Never a
//!   failure: nothing is wrong with the credential.
//! - `Unreadable` — an answer this plugin does not understand. Treated as a refusal rather than
//!   as success, because reporting `authorized` without a stored token would leave an account
//!   that looks signed in and cannot download anything.
//!
//! A provider that could not be reached at all never gets here: `http-request` fails, the guest
//! returns that failure, and the host keeps the stored token and tries again later.

use plugin_guest_oauth::token::{self, Waiting};
// Microsoft spells the address `verification_uri`, which the shared reader prefers. Its
// `verification_uri_complete` -- the address with the code already in it -- is deliberately not
// read: the person is shown the code and types it, which is what makes a device sign-in legible.
pub use plugin_guest_oauth::token::{DeviceCode, TokenAnswer, read_device_code};

/// The translation code a refusal is reported under, so the interface can say it in the
/// language the person reads. `onedrive_oauth` is this plugin's slug; the catalogue in
/// `locales/` carries each of these.
///
/// Microsoft's vocabulary beyond RFC 6749: `authorization_declined` is the device-flow
/// spelling of "the person said no", `bad_verification_code` and `expired_token` are a device
/// code that is wrong or was left too long, `interaction_required` and `consent_required` are
/// a tenant policy or an administrator asking for something the flow cannot give, and
/// `invalid_grant` is the catch-all for refresh material that is no longer good — revoked, a
/// changed password, or a tenant that no longer allows the application.
#[must_use]
pub fn refusal_code(error: &str) -> &'static str {
    match error {
        "access_denied"
        | "authorization_declined"
        | "consent_required"
        | "interaction_required" => "consent_denied",
        "expired_token" | "bad_verification_code" | "invalid_request" => "code_expired",
        "invalid_grant" | "unauthorized_client" | "invalid_client" => "refresh_refused",
        _ => "refresh_refused",
    }
}

/// What this provider's token answer says "wait" with, besides HTTP 429.
const WAITING: Waiting = Waiting {
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
///
/// Microsoft's `error_description` is deliberately never read at all. It is a full sentence
/// with an `AADSTS` code, a timestamp, a trace id and a correlation id in it, written for a
/// developer, and there is no shape check that makes a sentence safe.
pub use plugin_common::device_flow::sanitize_error;

#[cfg(test)]
mod tests {
    use super::{
        DeviceCode, TokenAnswer, read_device_code, read_token_answer, refusal_code, sanitize_error,
    };

    #[test]
    fn a_granted_exchange_yields_the_three_values_the_host_stores() {
        let answer = read_token_answer(
            200,
            None,
            r#"{"token_type":"Bearer","scope":"Files.Read.All","expires_in":3599,"ext_expires_in":3599,
                "access_token":"AT","refresh_token":"RT"}"#,
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

    /// Microsoft rotates refresh material on every renewal, but a renewal without it is still
    /// a grant: the stored one keeps working, and reading its absence as a failure would end
    /// a sign-in that succeeded.
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

    /// The device-code answer Microsoft actually sends, with the message and the complete
    /// address it also carries left unread.
    #[test]
    fn a_device_code_answer_is_read_with_its_prompt_and_its_timing() {
        let answer = read_device_code(
            r#"{"user_code":"ABCD1234","device_code":"DC","verification_uri":"https://microsoft.com/devicelogin",
                "expires_in":900,"interval":5,
                "message":"To sign in, use a web browser to open the page https://microsoft.com/devicelogin and enter the code ABCD1234 to authenticate."}"#,
        );
        assert_eq!(
            answer,
            Some(DeviceCode {
                device_code: "DC".to_owned(),
                user_code: "ABCD1234".to_owned(),
                verification_url: "https://microsoft.com/devicelogin".to_owned(),
                expires_in: Some(900),
                interval: Some(5),
            })
        );
        // Without a code there is nothing to show, and nothing is shown.
        assert_eq!(
            read_device_code(
                r#"{"device_code":"DC","verification_uri":"https://microsoft.com/devicelogin"}"#
            ),
            None
        );
    }

    #[test]
    fn the_refusals_microsoft_actually_sends_are_told_apart() {
        assert_eq!(
            read_token_answer(400, None, r#"{"error":"access_denied"}"#),
            TokenAnswer::Refused("access_denied".to_owned())
        );
        assert_eq!(refusal_code("access_denied"), "consent_denied");
        // The device-flow spelling of "the person said no".
        assert_eq!(refusal_code("authorization_declined"), "consent_denied");
        // A tenant policy or an administrator's consent requirement.
        assert_eq!(refusal_code("interaction_required"), "consent_denied");
        assert_eq!(refusal_code("consent_required"), "consent_denied");
        // A device code that ran out, or was typed wrong.
        assert_eq!(refusal_code("expired_token"), "code_expired");
        assert_eq!(refusal_code("bad_verification_code"), "code_expired");
        // The one that ends a renewal: revoked, a changed password, a tenant that no longer
        // allows the application.
        assert_eq!(refusal_code("invalid_grant"), "refresh_refused");
    }

    /// A device sign-in the person has not finished is a wait, never a refusal, and it waits
    /// as long as Microsoft asked.
    #[test]
    fn not_yet_is_a_wait_and_not_a_refusal() {
        assert_eq!(
            read_token_answer(
                400,
                None,
                r#"{"error":"authorization_pending","error_description":"AADSTS70016: ...","interval":5}"#
            ),
            TokenAnswer::Busy(Some(5))
        );
        assert_eq!(
            read_token_answer(400, None, r#"{"error":"slow_down","interval":10}"#),
            TokenAnswer::Busy(Some(10))
        );
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

    /// Microsoft's error documents carry an `error_description` with an AADSTS code, a trace
    /// id and a correlation id in one sentence. None of it survives, and neither does anything
    /// that sentence happened to quote.
    #[test]
    fn a_providers_error_text_never_travels_verbatim() {
        assert_eq!(sanitize_error("invalid_grant"), "invalid_grant");
        assert_eq!(
            sanitize_error(
                "AADSTS70000: The provided value for the input parameter is not valid. Trace ID: 1234"
            ),
            "refused"
        );
        assert_eq!(sanitize_error("token EwBwA8l6BAAU rejected"), "refused");
        assert_eq!(sanitize_error("<html>500</html>"), "refused");
        assert_eq!(sanitize_error(""), "refused");
        assert_eq!(sanitize_error(&"x".repeat(200)), "refused");
    }
}
