//! Reading what the Seedr API answered, and telling its refusals apart.
//!
//! Kept apart from the component so it runs on the host target: `cargo test` covers it without
//! a WebAssembly toolchain, which is the only place the shapes that actually occur — a refusal
//! served as an HTML page, a 200 whose body says `result: false`, a plan that does not include
//! the API — can be pinned down.
//!
//! Written against Seedr's published REST v1, <https://www.seedr.cc/docs/api/rest/v1/>, read
//! again on 2026-09-23 because the job file asked for exactly that second look:
//!
//! - **Auth flavour**: HTTP Basic, and nothing else. The page says so itself — "Rest API v1 is
//!   only available to use with HTTP basic auth, making it only secure for personal use" — and
//!   the v2 and OAuth API it has announced since its 2021 copyright still have no reference
//!   anywhere. So the token variant the feasibility left open does not exist, and this plugin
//!   takes v1 as measured.
//! - `GET /rest/user` answers the account record. Read for one thing: whether the credential
//!   works at all, plus whatever it says about space.
//! - **The API is a paid feature.** The same page states it is "accessible only to relevant
//!   premium account types", which is why a refusal naming the plan has a code of its own
//!   rather than being filed under "some 4xx".
//! - **Refusals**: `seedr_common::reason` is what decides which half of an answer may be
//!   repeated.
//!
//! **Nothing here has been run against a live Seedr account.** The status mapping comes from
//! the provider's documentation and from the method matrix measured on 2026-09-22, which is
//! what `docs/roadmap/jobs/120-04-seedr-feasibility.md` records; the account record's own field
//! names are read permissively for that reason, and a field this build does not recognise costs
//! nothing.

use serde::Deserialize;

pub use plugin_common::FailureKind;
use seedr_common::reason::ErrorEnvelope;

use crate::messages;

/// `GET /rest/user`, read for the two things a person is shown.
///
/// Everything is optional. The account record is the one document here whose field names were
/// not demonstrated by the provider's own example, so a missing one is a label with less in it
/// rather than a refusal.
#[derive(Debug, Default, Deserialize)]
pub struct UserRecord {
    #[serde(default, alias = "email")]
    pub username: Option<String>,
    #[serde(default, alias = "space_used")]
    pub used: Option<u64>,
    #[serde(default, alias = "space_max")]
    pub max: Option<u64>,
}

impl UserRecord {
    /// Reads the record out of a response body, refusing anything that is not a JSON object.
    ///
    /// The object check is not pedantry: serde builds a struct from a sequence in field order,
    /// so an array would become a record whose `username` was its first element.
    #[must_use]
    pub fn of(body: &[u8]) -> Option<Self> {
        let value = serde_json::from_slice::<serde_json::Value>(body).ok()?;
        if !value.is_object() {
            return None;
        }
        serde_json::from_value(value).ok()
    }

    /// The storage left, when Seedr stated both figures and they make sense together.
    ///
    /// A used figure larger than the maximum is not "minus three gigabytes": it is two fields
    /// that do not belong to each other, and reporting a number out of them would be believed
    /// on sight.
    #[must_use]
    pub fn space_free(&self) -> Option<u64> {
        self.max?.checked_sub(self.used?)
    }
}

/// A classified refusal: what it is, what the interface calls it, and what may be repeated.
#[derive(Debug, Eq, PartialEq)]
pub struct ApiFailure {
    pub kind: FailureKind,
    pub code: &'static str,
    pub message: String,
    /// The sanitised reason, when Seedr stated a code-shaped one.
    pub reason: Option<String>,
}

/// How long a provider-side outage is waited out. Five minutes, the figure the other
/// API-shaped providers in this tree settled on.
const BUSY_SECONDS: u64 = 300;

/// The wait a 429 gets when Seedr states no usable `Retry-After`. A minute: refused requests
/// count towards the very cap that refused them, so asking again at once only extends it.
const RATE_LIMIT_SECONDS: u64 = 60;

/// The longest `Retry-After` that is believed. A header from a proxy could otherwise park a
/// job for days.
const MAX_RETRY_AFTER_SECONDS: u64 = 3600;

fn failure(kind: FailureKind, (code, message): (&'static str, &str)) -> ApiFailure {
    ApiFailure {
        kind,
        code,
        message: message.to_owned(),
        reason: None,
    }
}

/// The refusal an answer describes, or `None` when it describes none.
///
/// An answer is a refusal when its status says so **or** when its body says so, whichever comes
/// first: Seedr answers a refused call with a 200 and `{"result": false}` as readily as with a
/// status, so a status-first rule would read a refusal as a success.
#[must_use]
pub fn failure_from(
    status: u16,
    retry_after_seconds: Option<u64>,
    envelope: &ErrorEnvelope,
) -> Option<ApiFailure> {
    if (200..=299).contains(&status) && !envelope.is_refusal() {
        return None;
    }
    let mut refusal = classify(status, retry_after_seconds, envelope);
    refusal.reason = envelope.reason();
    Some(refusal)
}

/// Maps one refusal onto the category the scheduler acts on.
///
/// The status decides, because Seedr's statuses are the part the feasibility actually measured:
/// every documented path answered 401 without credentials and 405 for the wrong method, which
/// is a provider that means what its status line says.
///
/// Two of the arms are worth stating rather than reading out of the table:
///
/// - **402 is its own answer.** Seedr's API is a premium feature by its own documentation, and
///   "your plan does not include this" is something a person can act on in a way that "Seedr
///   said no" is not.
/// - **A refusal on a 2xx is permanent and carries its word.** It is the shape a refused
///   transfer arrives in, and the word is the only part of it that may be repeated.
fn classify(status: u16, retry_after_seconds: Option<u64>, envelope: &ErrorEnvelope) -> ApiFailure {
    let plan_refused = matches!(
        envelope.reason().as_deref(),
        Some("premium_required" | "upgrade_required" | "not_premium")
    );
    match status {
        401 | 403 => failure(FailureKind::AccountInvalid, messages::AUTH_INVALID),
        402 => failure(FailureKind::Unsupported, messages::PLAN_REQUIRED),
        404 | 410 => failure(FailureKind::Permanent, messages::FILE_NOT_FOUND),
        429 => failure(
            FailureKind::RateLimited(Some(retry_after_seconds.unwrap_or(RATE_LIMIT_SECONDS))),
            messages::RATE_LIMITED,
        ),
        500..=599 => failure(
            FailureKind::Transient(Some(BUSY_SECONDS)),
            messages::SERVER_ERROR,
        ),
        status if (200..=299).contains(&status) && plan_refused => {
            failure(FailureKind::Unsupported, messages::PLAN_REQUIRED)
        }
        status if (200..=299).contains(&status) => {
            failure(FailureKind::Permanent, messages::API_ERROR)
        }
        other => ApiFailure {
            kind: FailureKind::Permanent,
            code: messages::HTTP_ERROR.0,
            message: messages::http_error(other),
            reason: None,
        },
    }
}

/// Reads a `Retry-After` stated in seconds.
///
/// A date-shaped one is ignored rather than guessed at, and so is one further away than an
/// hour: a wrong wait is worse than the bucket's own default, which is at least a wait somebody
/// can reason about.
#[must_use]
pub fn retry_after_seconds(header: Option<&str>) -> Option<u64> {
    let seconds: u64 = header?.trim().parse().ok()?;
    (seconds > 0 && seconds <= MAX_RETRY_AFTER_SECONDS).then_some(seconds)
}

#[cfg(test)]
#[path = "api/tests.rs"]
mod tests;
