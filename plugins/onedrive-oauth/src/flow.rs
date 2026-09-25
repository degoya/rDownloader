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

use crate::pkce;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenAnswer {
    Granted {
        access_token: String,
        refresh_token: Option<String>,
        expires_in_seconds: Option<u64>,
    },
    /// Microsoft's own `error` code, e.g. `access_denied` or `invalid_grant`.
    Refused(String),
    /// Too many requests, or the person has not confirmed yet; wait this many seconds if
    /// Microsoft said how long.
    Busy(Option<u64>),
    Unreadable(u16),
}

/// What a device-code answer carries: the code to poll with, the code to show, and where to
/// send the person.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub expires_in: Option<u64>,
    pub interval: Option<u64>,
}

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

/// Reads a device-authorization answer, or `None` when it is not one.
///
/// RFC 8628 spells the address `verification_uri`, and so does Microsoft; enough providers
/// write `verification_url` that both are read. An answer missing any of the three required
/// values is `None` rather than a half-filled prompt: a sign-in without a code is a screen
/// nobody can act on. Microsoft's `verification_uri_complete` — the address with the code
/// already in it — is deliberately not preferred: the person is shown the code and types it,
/// which is what makes a device sign-in legible.
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

/// Microsoft's `error` code, reduced to something that is safe to put in a message.
///
/// The point is not tidiness. Whatever a provider sends back travels into a log line and into
/// the failure the interface shows, and an endpoint that echoed part of a token into its error
/// document would otherwise publish it. RFC 6749 error codes are lowercase words joined by
/// underscores, so anything that is not exactly that shape is dropped whole rather than
/// filtered character by character — filtering would keep the digits of a leaked token.
///
/// Microsoft's `error_description` is deliberately never read at all. It is a full sentence
/// with an `AADSTS` code, a timestamp, a trace id and a correlation id in it, written for a
/// developer, and there is no shape check that makes a sentence safe.
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
