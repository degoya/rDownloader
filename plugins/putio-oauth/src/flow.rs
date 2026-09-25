//! Reading what Put.io's token endpoint answered.
//!
//! Kept apart from the component so it can be unit-tested on the host target: `cargo test`
//! runs these without a WebAssembly toolchain.
//!
//! Put.io's exchange is the plainest one this tree has met. It answers
//! `{"access_token": "..."}` on success and an ordinary Put.io error document on refusal —
//! `{"error_type": "...", "error_message": "..."}` — rather than the RFC 6749 `error` field an
//! OAuth endpoint usually uses. Both spellings are read, because a provider that starts
//! answering the standard shape should not break a sign-in that already works.
//!
//! The four answers below are the four the host reacts to differently:
//!
//! - `Granted` — store the token, report `authorized`.
//! - `Refused` — Put.io said no and will keep saying no. It becomes `failed`, and the person is
//!   told to sign in again.
//! - `Busy` — a rate limit. It becomes `pending`, and the host waits. Never a failure: nothing
//!   is wrong with the credential.
//! - `Unreadable` — an answer this plugin does not understand. Treated as a refusal rather
//!   than as success, because reporting `authorized` without a stored token would leave an
//!   account that looks signed in and cannot download anything.
//!
//! A provider that could not be reached at all never gets here: `http-request` fails, the guest
//! returns that failure, and the host keeps the stored token and tries again later.

use putio_common::reason;

use crate::pkce;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenAnswer {
    Granted {
        access_token: String,
        /// Put.io issues none today. Read anyway, so a day when it does needs no edit here.
        refresh_token: Option<String>,
        /// Likewise: Put.io states no expiry, and `None` is what keeps the renewal sweep from
        /// scheduling a renewal there is nothing to renew from.
        expires_in_seconds: Option<u64>,
    },
    /// The word Put.io refused with, already sanitised.
    Refused(String),
    /// Too many requests; wait this many seconds if Put.io said how long.
    Busy(Option<u64>),
    Unreadable(u16),
}

/// The translation code a refusal is reported under, so the interface can say it in the
/// language the person reads. `putio_oauth` is this plugin's slug; the catalogue in `locales/`
/// carries each of these.
///
/// Put.io's own words and the RFC 6749 ones are both mapped, because the token endpoint uses
/// the first and a future one might use the second. Everything unrecognised lands on
/// `refresh_refused`, which is the terminal one: it tells the person to sign in again, which
/// is the only thing that helps for a grant that is no longer good.
#[must_use]
pub fn refusal_code(error: &str) -> &'static str {
    match error {
        // Put.io's own vocabulary.
        "ACCESS_DENIED" | "UNAUTHORIZED" => "consent_denied",
        "INVALID_CODE" | "INVALID_REQUEST" | "BAD_REQUEST" => "code_expired",
        "INVALID_GRANT" | "INVALID_CLIENT" | "INVALID_TOKEN" => "refresh_refused",
        // RFC 6749's, in case the endpoint ever answers in it.
        "access_denied" | "consent_required" => "consent_denied",
        "expired_token" | "invalid_request" => "code_expired",
        _ => "refresh_refused",
    }
}

/// Reads a token or refresh answer.
///
/// `retry_after` is the `Retry-After` response header, which is where a provider says how long
/// to wait; the body is consulted only when the header is missing.
#[must_use]
pub fn read_token_answer(status: u16, retry_after: Option<&str>, body: &str) -> TokenAnswer {
    let error = refusal(body);
    // Waiting is not refusal even when it arrives with an error word beside it, so it is read
    // first. Reading a rate limit as a failure would end a sign-in that was going perfectly
    // well.
    if status == 429 || matches!(error.as_deref(), Some("slow_down" | "TOO_MANY_REQUESTS")) {
        let seconds = retry_after
            .and_then(|value| value.trim().parse::<u64>().ok())
            .or_else(|| pkce::number_field(body, "retry_after"))
            .or_else(|| pkce::number_field(body, "interval"));
        return TokenAnswer::Busy(seconds);
    }
    if let Some(error) = error {
        return TokenAnswer::Refused(error);
    }
    match access_token(body) {
        Some(access_token) => TokenAnswer::Granted {
            access_token,
            refresh_token: pkce::string_field(body, "refresh_token").filter(|t| !t.is_empty()),
            expires_in_seconds: pkce::number_field(body, "expires_in"),
        },
        None => TokenAnswer::Unreadable(status),
    }
}

/// The token an answer carries, under either of the two names Put.io uses for it.
///
/// `access_token` is what the redirect exchange answers. `oauth_token` is what the account's
/// own settings page calls the same value, and it is read too so a person pasting one into a
/// future entrance is not met with "this answer makes no sense".
fn access_token(body: &str) -> Option<String> {
    pkce::string_field(body, "access_token")
        .or_else(|| pkce::string_field(body, "oauth_token"))
        .filter(|token| !token.is_empty())
}

/// The refusal word an answer carries, sanitised, under either spelling.
///
/// Nothing else of the document travels. Put.io's `error_message` and RFC 6749's
/// `error_description` are both English sentences written for a developer, and there is no
/// shape check that makes a sentence safe: an endpoint that echoed part of a token into one
/// would otherwise publish it into a log line and into the failure the interface shows.
fn refusal(body: &str) -> Option<String> {
    if let Some(word) = pkce::string_field(body, "error_type") {
        return Some(reason::sanitize(&word).unwrap_or_else(|| "refused".to_owned()));
    }
    let word = pkce::string_field(body, "error")?;
    Some(sanitize_rfc6749(&word))
}

/// An RFC 6749 `error` code, reduced to something that is safe to put in a message.
///
/// Those codes are lower-case words joined by underscores, so anything that is not exactly
/// that shape is dropped whole rather than filtered character by character — filtering would
/// keep the digits of a leaked token.
#[must_use]
pub fn sanitize_rfc6749(error: &str) -> String {
    let trimmed = error.trim();
    let is_error_code = !trimmed.is_empty()
        && trimmed.len() <= 40
        && trimmed
            .chars()
            .all(|character| character.is_ascii_lowercase() || character == '_');
    if is_error_code {
        trimmed.to_owned()
    } else {
        "refused".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{TokenAnswer, read_token_answer, refusal_code, sanitize_rfc6749};

    /// What Put.io actually answers: one field, no expiry, no refresh material. All three
    /// matter — the two `None`s are what keep the renewal sweep from scheduling a renewal
    /// there is nothing to renew from.
    #[test]
    fn a_granted_exchange_is_a_token_and_nothing_else() {
        assert_eq!(
            read_token_answer(200, None, r#"{"access_token":"AT"}"#),
            TokenAnswer::Granted {
                access_token: "AT".to_owned(),
                refresh_token: None,
                expires_in_seconds: None,
            }
        );
    }

    /// And if Put.io ever does state an expiry and refresh material, both are read rather than
    /// dropped, so the renewal that already exists starts working without an edit here.
    #[test]
    fn refresh_material_is_read_if_it_ever_arrives() {
        assert_eq!(
            read_token_answer(
                200,
                None,
                r#"{"access_token":"AT","refresh_token":"RT","expires_in":3600}"#
            ),
            TokenAnswer::Granted {
                access_token: "AT".to_owned(),
                refresh_token: Some("RT".to_owned()),
                expires_in_seconds: Some(3600),
            }
        );
    }

    #[test]
    fn the_refusals_put_io_actually_sends_are_told_apart() {
        assert_eq!(
            read_token_answer(400, None, r#"{"error_type":"INVALID_CODE"}"#),
            TokenAnswer::Refused("INVALID_CODE".to_owned())
        );
        assert_eq!(refusal_code("INVALID_CODE"), "code_expired");
        assert_eq!(refusal_code("ACCESS_DENIED"), "consent_denied");
        assert_eq!(refusal_code("INVALID_GRANT"), "refresh_refused");
        // The standard spelling, read too.
        assert_eq!(
            read_token_answer(400, None, r#"{"error":"access_denied"}"#),
            TokenAnswer::Refused("access_denied".to_owned())
        );
        assert_eq!(refusal_code("access_denied"), "consent_denied");
        // Anything this build does not know ends the sign-in rather than looping.
        assert_eq!(refusal_code("SOMETHING_NEW"), "refresh_refused");
    }

    #[test]
    fn a_rate_limit_reads_retry_after_before_the_body() {
        assert_eq!(
            read_token_answer(429, Some("42"), r#"{"error_type":"TOO_MANY_REQUESTS"}"#),
            TokenAnswer::Busy(Some(42))
        );
        assert_eq!(read_token_answer(429, None, "{}"), TokenAnswer::Busy(None));
        // A 200 that still says "slow down" is a wait and not a failed sign-in.
        assert_eq!(
            read_token_answer(
                200,
                None,
                r#"{"error_type":"TOO_MANY_REQUESTS","interval":5}"#
            ),
            TokenAnswer::Busy(Some(5))
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
        assert_eq!(
            read_token_answer(502, None, "<html>bad gateway</html>"),
            TokenAnswer::Unreadable(502)
        );
    }

    /// Put.io's error documents carry an `error_message` written as an English sentence. None
    /// of it survives, and neither does anything that sentence happened to quote.
    #[test]
    fn a_providers_error_text_never_travels_verbatim() {
        let answer = read_token_answer(
            400,
            None,
            r#"{"error_type":"INVALID_CODE","error_message":"code AB12CD34 is not valid"}"#,
        );
        assert_eq!(answer, TokenAnswer::Refused("INVALID_CODE".to_owned()));
        // A word that is not one of Put.io's keeps nothing at all.
        assert_eq!(
            read_token_answer(400, None, r#"{"error_type":"token ya29.a0 rejected"}"#),
            TokenAnswer::Refused("refused".to_owned())
        );
        assert_eq!(sanitize_rfc6749("invalid_grant"), "invalid_grant");
        assert_eq!(sanitize_rfc6749("Token has been revoked."), "refused");
        assert_eq!(sanitize_rfc6749(""), "refused");
        assert_eq!(sanitize_rfc6749(&"x".repeat(200)), "refused");
    }
}
