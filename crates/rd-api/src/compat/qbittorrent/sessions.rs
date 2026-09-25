//! Opaque `SID` values for the qBittorrent adapter.
//!
//! The cookie used to carry the API bearer itself. That was defensible on paper — no session
//! table, a credential that stays revocable, an automation client that survives a restart —
//! and it is not defensible on the wire: the cookie is set at `Path=/`, so every path on the
//! origin can read a full `api:*` credential, and a plain-HTTP hop in front of the service
//! carries it in clear text. A handle that means nothing outside this process costs far less
//! than that.
//!
//! In memory rather than in the database, deliberately. A qBittorrent session is not a record
//! worth keeping: the client re-logs in when it is refused, which is the same thing real
//! qBittorrent makes it do, and a handle that cannot outlive the process it was minted in
//! cannot be replayed into the next one. The token behind the handle is still checked against
//! the token store on every request, so revoking it takes effect at once, exactly as before.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;

/// How long a session survives without being used. qBittorrent's own default is one hour.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// Ceiling on concurrent sessions.
///
/// Login is metered but not free, and the map is reachable by anyone holding a valid token, so
/// it needs a bound that does not depend on clients logging out. The oldest entry gives way,
/// which costs the least active client one re-login.
const MAX_SESSIONS: usize = 256;

/// Live `SID` handles and the bearer each one stands for.
#[derive(Default)]
pub(crate) struct SessionStore {
    entries: Mutex<HashMap<String, Entry>>,
}

struct Entry {
    token: String,
    last_used: Instant,
}

impl SessionStore {
    /// Mints a handle for a token that has just been accepted.
    pub(crate) fn issue(&self, token: &str) -> String {
        let mut bytes = [0_u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let id = URL_SAFE_NO_PAD.encode(bytes);
        let mut entries = self.lock();
        entries.retain(|_, entry| entry.last_used.elapsed() < IDLE_TIMEOUT);
        while entries.len() >= MAX_SESSIONS {
            let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            entries.remove(&oldest);
        }
        entries.insert(
            id.clone(),
            Entry {
                token: token.to_owned(),
                last_used: Instant::now(),
            },
        );
        id
    }

    /// The bearer a handle stands for, if it is one of ours and still current.
    pub(crate) fn resolve(&self, sid: &str) -> Option<String> {
        let mut entries = self.lock();
        let entry = entries.get_mut(sid)?;
        if entry.last_used.elapsed() >= IDLE_TIMEOUT {
            entries.remove(sid);
            return None;
        }
        entry.last_used = Instant::now();
        Some(entry.token.clone())
    }

    /// Drops a handle on logout, so the cookie a client keeps is worth nothing.
    pub(crate) fn forget(&self, sid: &str) {
        self.lock().remove(sid);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_SESSIONS, SessionStore};

    #[test]
    fn a_handle_stands_for_the_token_it_was_minted_for() {
        let store = SessionStore::default();
        let id = store.issue("secret-bearer");
        assert_eq!(store.resolve(&id).as_deref(), Some("secret-bearer"));
    }

    /// The whole point: the value handed to the client is not the credential.
    #[test]
    fn the_handle_is_not_the_token() {
        let store = SessionStore::default();
        let id = store.issue("secret-bearer");
        assert!(!id.contains("secret-bearer"));
        assert!(store.resolve("secret-bearer").is_none());
    }

    #[test]
    fn logging_out_makes_the_handle_worthless() {
        let store = SessionStore::default();
        let id = store.issue("secret-bearer");
        store.forget(&id);
        assert!(store.resolve(&id).is_none());
    }

    /// Reachable by any token holder, so it cannot grow without a bound.
    #[test]
    fn the_store_stays_bounded() {
        let store = SessionStore::default();
        for _ in 0..MAX_SESSIONS * 4 {
            store.issue("secret-bearer");
        }
        assert!(store.entries.lock().expect("entries").len() <= MAX_SESSIONS);
    }
}
