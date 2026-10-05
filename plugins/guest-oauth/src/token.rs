//! Reading what a token endpoint answered, once for the OAuth plugins (RD-1110-04, audit R3).
//!
//! Box, Dropbox, Google Drive, OneDrive and the example plugin read the same four answers out of
//! the same RFC 6749 document; what differs is which `error` codes mean "wait" and where a body
//! states how long, which a plugin says in its [`Waiting`]. Its refusal codes and their mapping
//! stay in the plugin's own `flow.rs`, next to its catalogue. Plain Rust over `plugin-common`'s
//! JSON readers, so it is unit-tested on the host target.
//!
//! The four answers are the four the host reacts to differently, which is why they are four and
//! not one:
//!
//! - `Granted` — store the token, report `authorized`.
//! - `Refused` — the provider said no and will keep saying no. It becomes `failed`, and the
//!   person is told to sign in again.
//! - `Busy` — a rate limit, or a device flow the person has not confirmed yet. It becomes
//!   `pending`, and the host waits the given number of seconds. Never a failure: nothing is
//!   wrong with the credential.
//! - `Unreadable` — an answer the plugin does not understand. Treated as a refusal rather than
//!   as success, because reporting `authorized` without a stored token would leave an account
//!   that looks signed in and cannot download anything.
//!
//! A provider that could not be reached at all never gets here: `http-request` fails, the guest
//! returns that failure, and the host keeps the stored token and tries again later.

use plugin_common::json::{number_field, string_field};

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

/// What a provider's token answer says "wait" with, besides HTTP 429.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Waiting {
    /// `error` codes that are a wait, not a refusal: `slow_down`, a rate limit spelled in the
    /// body, and for a device flow `authorization_pending`, the person not finished at the
    /// other screen yet.
    pub errors: &'static [&'static str],
    /// Body fields a wait in seconds is read from, in order, when no `Retry-After` came.
    pub fields: &'static [&'static str],
}

/// Reads a token or refresh answer.
///
/// `retry_after` is the `Retry-After` response header, which is where a provider says how long
/// to wait; the body's [`Waiting::fields`] are consulted only when the header is missing.
#[must_use]
pub fn read_token_answer(
    waiting: &Waiting,
    status: u16,
    retry_after: Option<&str>,
    body: &str,
) -> TokenAnswer {
    let error = string_field(body, "error");
    // Waiting is not refusal even when it arrives with an `error` field, so it is read first.
    // Reading a rate limit as a failure would end a sign-in that was going perfectly well.
    if status == 429
        || error
            .as_deref()
            .is_some_and(|error| waiting.errors.contains(&error))
    {
        let seconds = plugin_common::retry_after_seconds(retry_after).or_else(|| {
            waiting
                .fields
                .iter()
                .find_map(|field| number_field(body, field))
        });
        return TokenAnswer::Busy(seconds);
    }
    if let Some(error) = error {
        return TokenAnswer::Refused(error);
    }
    match string_field(body, "access_token") {
        Some(access_token) if !access_token.is_empty() => TokenAnswer::Granted {
            access_token,
            refresh_token: string_field(body, "refresh_token").filter(|t| !t.is_empty()),
            expires_in_seconds: number_field(body, "expires_in"),
        },
        _ => TokenAnswer::Unreadable(status),
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
    let device_code = string_field(body, "device_code")?;
    let user_code = string_field(body, "user_code")?;
    let verification_url = string_field(body, "verification_uri")
        .or_else(|| string_field(body, "verification_url"))?;
    (!device_code.is_empty() && !user_code.is_empty() && !verification_url.is_empty()).then(|| {
        DeviceCode {
            device_code,
            user_code,
            verification_url,
            expires_in: number_field(body, "expires_in"),
            interval: number_field(body, "interval"),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{TokenAnswer, Waiting, read_device_code, read_token_answer};

    const DEVICE: Waiting = Waiting {
        errors: &["slow_down", "authorization_pending"],
        fields: &["retry_after", "interval"],
    };

    const REDIRECT_ONLY: Waiting = Waiting {
        errors: &["slow_down"],
        fields: &["retry_after"],
    };

    #[test]
    fn only_the_named_errors_and_fields_are_a_wait() {
        let pending = r#"{"error":"authorization_pending","interval":7}"#;
        assert_eq!(
            read_token_answer(&DEVICE, 400, None, pending),
            TokenAnswer::Busy(Some(7))
        );
        assert_eq!(
            read_token_answer(&REDIRECT_ONLY, 400, None, pending),
            TokenAnswer::Refused("authorization_pending".to_owned())
        );
        assert_eq!(
            read_token_answer(&REDIRECT_ONLY, 429, None, r#"{"interval":7}"#),
            TokenAnswer::Busy(None)
        );
    }

    #[test]
    fn the_header_wins_over_the_body() {
        assert_eq!(
            read_token_answer(&DEVICE, 429, Some("30"), r#"{"retry_after":5}"#),
            TokenAnswer::Busy(Some(30))
        );
    }

    #[test]
    fn a_device_code_needs_its_three_values() {
        let code = read_device_code(
            r#"{"device_code":"D","user_code":"U","verification_url":"https://x.test/d"}"#,
        )
        .expect("device code");
        assert_eq!(code.verification_url, "https://x.test/d");
        assert_eq!(
            read_device_code(r#"{"device_code":"D","user_code":"U"}"#),
            None
        );
    }
}
