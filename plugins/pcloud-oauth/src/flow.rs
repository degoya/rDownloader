//! Reading what pCloud's token endpoint answered.
//!
//! Kept apart from the component so it can be unit-tested on the host target: `cargo test`
//! runs these without a WebAssembly toolchain.
//!
//! pCloud answers **HTTP 200 to its refusals too**, and puts what went wrong in the `result`
//! number of the document. So the status is almost never the answer here, and `error` beside
//! the number is an English sentence written for a developer — never read, because there is no
//! shape check that makes a sentence safe. What travels instead is the decimal number, which
//! cannot carry a token.
//!
//! The four answers below are the ones the host reacts to differently, which is why they are
//! four and not one:
//!
//! - `Granted` — store the token, report `authorized`.
//! - `Refused` — pCloud said no and will keep saying no. It becomes `failed`, and the person is
//!   told to sign in again.
//! - `Busy` — a rate limit, pCloud's 4xxx family. It becomes `pending`, and the host waits.
//!   Never a failure: nothing is wrong with the credential.
//! - `Unreadable` — an answer this plugin does not understand. Treated as a refusal rather than
//!   as success, because reporting `authorized` without a stored token would leave an account
//!   that looks signed in and cannot download anything.
//!
//! A provider that could not be reached at all never gets here: `http-request` fails, the guest
//! returns that failure, and the host keeps the stored token and tries again later.

use pcloud_common::api::{Category, OK};

use crate::pkce;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenAnswer {
    Granted {
        access_token: String,
    },
    /// pCloud's own `result` number, e.g. 2094 for a code it will not take.
    Refused(u64),
    /// Too many requests; wait this many seconds if pCloud said how long.
    Busy(Option<u64>),
    Unreadable(u16),
}

/// The translation code a refusal is reported under, so the interface can say it in the
/// language the person reads. `pcloud_oauth` is this plugin's slug; the catalogue in
/// `locales/` carries each of these.
///
/// Classified by pCloud's documented *family* rather than by individual numbers: pCloud
/// publishes and guarantees the families, while the numbers inside one are a longer and less
/// stable list.
#[must_use]
pub fn refusal_code(result: u64) -> &'static str {
    match Category::of(result) {
        // The person said no, or the application may not ask.
        Category::Denied => "consent_denied",
        // A code that was used, expired, or issued by the other installation — and a call this
        // plugin got wrong, which reaches the person the same way: start again.
        Category::Credential | Category::BadRequest | Category::Missing => "code_expired",
        _ => "sign_in_refused",
    }
}

/// Reads a token answer.
///
/// `retry_after` is the `Retry-After` response header, which is where a provider says how long
/// to wait; pCloud's rate-limit document carries a sentence and no interval.
#[must_use]
pub fn read_token_answer(status: u16, retry_after: Option<&str>, body: &str) -> TokenAnswer {
    let Some(result) = pkce::number_field(body, "result") else {
        return TokenAnswer::Unreadable(status);
    };
    // Waiting is not refusal, so it is read before the refusal is. Reading a rate limit as a
    // failure would end a sign-in that was going perfectly well.
    if Category::of(result) == Category::RateLimited {
        return TokenAnswer::Busy(retry_after.and_then(|value| value.trim().parse::<u64>().ok()));
    }
    if result != OK {
        return TokenAnswer::Refused(result);
    }
    match pkce::string_field(body, "access_token") {
        Some(access_token) if !access_token.is_empty() => TokenAnswer::Granted { access_token },
        _ => TokenAnswer::Unreadable(status),
    }
}

#[cfg(test)]
mod tests {
    use super::{TokenAnswer, read_token_answer, refusal_code};

    #[test]
    fn a_granted_exchange_yields_the_one_value_the_host_stores() {
        assert_eq!(
            read_token_answer(
                200,
                None,
                r#"{"result":0,"access_token":"AT","token_type":"bearer","uid":12345,
                    "locationid":2}"#
            ),
            TokenAnswer::Granted {
                access_token: "AT".to_owned(),
            }
        );
    }

    /// pCloud issues no renewal material at all, so an answer that carries none is still a
    /// grant. Demanding one would fail every sign-in pCloud has ever made.
    #[test]
    fn an_answer_without_renewal_material_is_still_a_grant() {
        assert_eq!(
            read_token_answer(200, None, r#"{"result":0,"access_token":"AT","uid":1}"#),
            TokenAnswer::Granted {
                access_token: "AT".to_owned(),
            }
        );
    }

    /// pCloud refuses with HTTP 200 and a number, which is the trap this whole module exists
    /// for: a plugin that trusted the status would read every refusal as a success.
    #[test]
    fn a_refusal_arrives_as_http_200_and_is_still_a_refusal() {
        assert_eq!(
            read_token_answer(
                200,
                None,
                r#"{"result":2094,"error":"Invalid 'code' provided."}"#
            ),
            TokenAnswer::Refused(2094)
        );
        assert_eq!(refusal_code(2094), "code_expired");
        assert_eq!(refusal_code(2003), "consent_denied");
        assert_eq!(refusal_code(1004), "code_expired");
        assert_eq!(refusal_code(9999), "sign_in_refused");
    }

    #[test]
    fn a_rate_limit_is_a_wait_and_reads_its_interval_from_the_header() {
        assert_eq!(
            read_token_answer(200, Some("42"), r#"{"result":4000,"error":"Too many."}"#),
            TokenAnswer::Busy(Some(42))
        );
        assert_eq!(
            read_token_answer(200, None, r#"{"result":4000}"#),
            TokenAnswer::Busy(None)
        );
    }

    #[test]
    fn an_answer_without_a_token_is_never_read_as_success() {
        assert_eq!(
            read_token_answer(200, None, r#"{"result":0,"token_type":"bearer"}"#),
            TokenAnswer::Unreadable(200)
        );
        assert_eq!(
            read_token_answer(200, None, r#"{"result":0,"access_token":""}"#),
            TokenAnswer::Unreadable(200)
        );
        // No `result` at all is not pCloud's answer: a gateway page, or an outage.
        assert_eq!(
            read_token_answer(502, None, "<html>502 Bad Gateway</html>"),
            TokenAnswer::Unreadable(502)
        );
    }
}
