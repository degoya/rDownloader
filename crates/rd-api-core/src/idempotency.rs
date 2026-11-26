//! `Idempotency-Key`: one create for however often a client repeats the request (RD-1480-10,
//! API-11).
//!
//! A client whose `POST /api/v1/downloads` timed out cannot know whether the download was
//! created, and repeating the request used to create it twice. With the header it repeats the
//! request under the same key and is answered the download the first one created.
//!
//! * **Per credential.** A key is one token's, or one signed-in session's: two clients that happen
//!   to choose the same key never see each other's download.
//! * **Bound to the request.** The same key with another body is refused as
//!   `request.idempotency_key_reused`; answering it the first download would lose the second
//!   request silently. A key whose request failed created nothing and takes any body.
//! * **Bounded.** A key lives for [`LIFETIME`], a caller holds at most [`MAX_PER_CALLER`] and the
//!   service [`MAX_ENTRIES`]; above that the oldest goes first, the caller's own before anybody
//!   else's. An entry is the key, a digest and an id, so the whole store stays below a megabyte.
//!
//! In memory on purpose, like the call limit's windows (`auth::token_rate`): the key covers the
//! retry after a lost answer, which comes within seconds. After a restart the key is unknown and a
//! repeat creates the download again, which is what every request without the header does; a
//! table, its writer and its sweep would be there for that one case.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use axum::http::{HeaderMap, HeaderName};
use rd_core::DownloadId;

use crate::{ApiError, audit::Actor};

/// The request header that names the key.
pub const IDEMPOTENCY_KEY_HEADER: HeaderName = HeaderName::from_static("idempotency-key");

/// The longest key, in characters; a UUID has 36.
pub(crate) const MAX_KEY_LENGTH: usize = 128;

/// How long a key is remembered after the request that brought it.
const LIFETIME: Duration = Duration::from_secs(60 * 60);

/// The most keys one credential holds at once.
const MAX_PER_CALLER: usize = 256;

/// The most keys the service holds at once.
const MAX_ENTRIES: usize = 2_048;

/// One caller's key, checked.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct IdempotencyKey {
    caller: String,
    key: String,
}

impl IdempotencyKey {
    /// The key `value` of `actor`.
    ///
    /// # Errors
    ///
    /// `request.idempotency_key_invalid` unless it is 1 to 128 visible ASCII characters.
    pub fn new(actor: &Actor, value: &str) -> Result<Self, ApiError> {
        let key = value.trim();
        if key.is_empty()
            || key.len() > MAX_KEY_LENGTH
            || !key.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(invalid());
        }
        Ok(Self {
            caller: format!(
                "{}:{}",
                actor.kind.as_str(),
                actor.id.as_deref().unwrap_or_default()
            ),
            key: key.to_owned(),
        })
    }

    /// The key the request's header names, or `None` without the header.
    ///
    /// # Errors
    ///
    /// `request.idempotency_key_invalid` for a malformed header, and for one sent twice.
    pub fn from_headers(actor: &Actor, headers: &HeaderMap) -> Result<Option<Self>, ApiError> {
        let mut values = headers.get_all(IDEMPOTENCY_KEY_HEADER).iter();
        let Some(value) = values.next() else {
            return Ok(None);
        };
        if values.next().is_some() {
            return Err(invalid());
        }
        let value = value.to_str().map_err(|_| invalid())?;
        Self::new(actor, value).map(Some)
    }

    /// The key of one part of a request that creates several: `#` and its number behind it.
    #[must_use]
    pub fn part(&self, number: usize) -> Self {
        Self {
            caller: self.caller.clone(),
            key: format!("{}#{number}", self.key),
        }
    }
}

fn invalid() -> ApiError {
    ApiError::bad_request(
        "request.idempotency_key_invalid",
        format!("The Idempotency-Key must be 1 to {MAX_KEY_LENGTH} visible ASCII characters"),
    )
    .with_param("max", MAX_KEY_LENGTH)
}

fn reused() -> ApiError {
    ApiError::unprocessable(
        "request.idempotency_key_reused",
        "This Idempotency-Key was already used for a different request",
    )
}

/// What a key's request created: `None` while nothing is created yet.
///
/// The lock is held while the request runs, so a repeat that arrives meanwhile waits for its
/// outcome instead of creating a second download beside it.
pub type Outcome = Arc<tokio::sync::Mutex<Option<DownloadId>>>;

struct Entry {
    fingerprint: String,
    since: Instant,
    outcome: Outcome,
}

/// The keys the service remembers.
#[derive(Clone, Default)]
pub struct IdempotencyStore(Arc<Mutex<HashMap<IdempotencyKey, Entry>>>);

impl IdempotencyStore {
    /// The outcome slot of `key`, for a request whose content has this `fingerprint`.
    ///
    /// # Errors
    ///
    /// `request.idempotency_key_reused` when the key stands for another request that created
    /// something or is still running.
    pub fn claim(&self, key: IdempotencyKey, fingerprint: &str) -> Result<Outcome, ApiError> {
        self.claim_at(key, fingerprint, Instant::now())
    }

    fn claim_at(
        &self,
        key: IdempotencyKey,
        fingerprint: &str,
        now: Instant,
    ) -> Result<Outcome, ApiError> {
        let mut entries = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        entries.retain(|_, entry| now.duration_since(entry.since) < LIFETIME);
        if let Some(entry) = entries.get(&key) {
            if entry.fingerprint == fingerprint {
                return Ok(Arc::clone(&entry.outcome));
            }
            // A request that failed created nothing, so its key is free again.
            if !entry
                .outcome
                .try_lock()
                .is_ok_and(|outcome| outcome.is_none())
            {
                return Err(reused());
            }
            entries.remove(&key);
        }
        make_room(&mut entries, &key.caller);
        let outcome = Outcome::default();
        entries.insert(
            key,
            Entry {
                fingerprint: fingerprint.to_owned(),
                since: now,
                outcome: Arc::clone(&outcome),
            },
        );
        Ok(outcome)
    }
}

/// Drops the oldest key until one more fits: the caller's own above its share, else anybody's.
fn make_room(entries: &mut HashMap<IdempotencyKey, Entry>, caller: &str) {
    let own = entries.keys().filter(|key| key.caller == caller).count();
    let scope = if own >= MAX_PER_CALLER {
        Some(caller)
    } else if entries.len() >= MAX_ENTRIES {
        None
    } else {
        return;
    };
    let oldest = entries
        .iter()
        .filter(|(key, _)| scope.is_none_or(|caller| key.caller == caller))
        .min_by_key(|(_, entry)| entry.since)
        .map(|(key, _)| key.clone());
    if let Some(oldest) = oldest {
        entries.remove(&oldest);
    }
}

#[cfg(test)]
#[path = "idempotency_tests.rs"]
mod tests;
