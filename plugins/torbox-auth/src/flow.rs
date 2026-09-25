//! Reading what `GET /user/me` said about a key.
//!
//! Target-independent, so the whole decision is tested without a WebAssembly target. What
//! travels back is one of three answers and never a sentence TorBox wrote: `detail` is prose
//! that changes and has echoed credentials back at other providers, and a sign-in message is
//! exactly the place a person would paste into a bug report.

use serde::Deserialize;

/// The envelope `GET /user/me` answers in.
#[derive(Default, Deserialize)]
pub struct Envelope {
    #[serde(default)]
    pub success: Option<bool>,
    #[serde(default)]
    pub error: Option<serde_json::Value>,
    #[serde(default)]
    pub data: Option<User>,
}

/// The half of the account this plugin reads.
#[derive(Default, Deserialize)]
pub struct User {
    /// `0` is the free tier; everything above it is a paid plan.
    #[serde(default)]
    pub plan: Option<i64>,
}

/// What a check of the key means.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The key is good and the account answered. The person is signed in, and nothing was
    /// stored: the key they pasted is already the credential.
    Valid,
    /// The key is not one TorBox accepts. Waiting does not change that, so the flow ends and
    /// the person is asked for a key rather than left watching a spinner.
    Invalid(&'static str),
    /// TorBox could not answer right now. The host waits the stated seconds and asks again,
    /// which is what makes a sign-in started during an outage finish afterwards instead of
    /// failing for a reason that had nothing to do with the key.
    Retry(u64),
}

/// How long a check waits after TorBox refused to answer. Long enough not to deepen a rate
/// limit, short enough that a person watching the accounts page sees it finish.
pub const RETRY_SECONDS: u64 = 15;

/// How long a check waits when TorBox stated no figure of its own with a 429.
pub const RATE_LIMIT_SECONDS: u64 = 60;

/// Decides what one answer means.
///
/// Three rules, in this order, and the order is the point:
///
/// - **A word TorBox named decides first.** `BAD_TOKEN` inside a `200` is still a bad key, and
///   a plugin that read the status first would call that a success.
/// - **A status that says "not you" is an invalid key**, and a status that says "not now" is a
///   wait. The difference is what the person is shown: one asks them to paste a key again, the
///   other asks them for nothing at all.
/// - **An answer that parses to nothing is a wait, not a refusal.** A proxy's error page is
///   not evidence about somebody's key.
#[must_use]
pub fn read(status: u16, retry_after: Option<u64>, body: &[u8]) -> Outcome {
    let envelope: Envelope = serde_json::from_slice(body).unwrap_or_default();
    if let Some(word) = envelope
        .error
        .as_ref()
        .and_then(|value| value.as_str())
        .map(|word| word.trim().to_ascii_uppercase())
        .filter(|word| !word.is_empty())
    {
        return match word.as_str() {
            "BAD_TOKEN" | "AUTH_ERROR" | "NO_AUTH" | "OAUTH_VERIFICATION_ERROR" => {
                Outcome::Invalid(crate::KEY_INVALID)
            }
            "TOO_MANY_REQUESTS" | "COOLDOWN_LIMIT" => {
                Outcome::Retry(retry_after.unwrap_or(RATE_LIMIT_SECONDS))
            }
            _ => Outcome::Retry(retry_after.unwrap_or(RETRY_SECONDS)),
        };
    }
    match status {
        401 | 403 => Outcome::Invalid(crate::KEY_INVALID),
        429 => Outcome::Retry(retry_after.unwrap_or(RATE_LIMIT_SECONDS)),
        200..=299 if envelope.success != Some(false) => {
            if envelope.data.is_some() {
                Outcome::Valid
            } else {
                // A success with no account in it is not an answer about the key.
                Outcome::Retry(retry_after.unwrap_or(RETRY_SECONDS))
            }
        }
        _ => Outcome::Retry(retry_after.unwrap_or(RETRY_SECONDS)),
    }
}

/// Reads a `Retry-After` header stated in seconds. A date-shaped one is ignored rather than
/// guessed at: a wrong wait is worse than the default.
#[must_use]
pub fn retry_after_seconds(value: Option<&str>) -> Option<u64> {
    value.and_then(|value| value.trim().parse::<u64>().ok())
}

#[cfg(test)]
#[path = "flow/tests.rs"]
mod tests;
