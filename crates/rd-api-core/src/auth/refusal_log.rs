//! The log line of a refused API token, once per client, path and reason in ten minutes
//! (RD-1240-38).
//!
//! An MCP client configured with the wrong kind of `Authorization` header asks again and again,
//! and every attempt wrote the same warning: 132 identical lines on the owner's instance, from
//! one client, burying everything else. The refusal stays visible — the first one is logged at
//! once, and the next line after the interval says how many identical ones it held back — but a
//! client that keeps knocking writes one line per interval, not one per knock.
//!
//! In memory on purpose, like the token-use throttles beside it: after a restart the first
//! refusal is logged again, which is exactly what an administrator looking for it wants.

use std::{
    collections::{HashMap, hash_map},
    net::IpAddr,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

/// How long identical refusals are counted instead of logged.
const INTERVAL: Duration = Duration::from_secs(10 * 60);

/// Entries kept before the expired ones are swept. A swept entry loses the count it held back,
/// which is the price of a bound: only a flood of distinct clients or reasons reaches it.
const SWEEP_ABOVE: usize = 1024;

/// What makes two refusals the same one.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Key {
    path: String,
    reason: String,
    client: IpAddr,
}

struct Entry {
    logged: Instant,
    /// Identical refusals since the last line that was written.
    repeated: u64,
}

#[derive(Default)]
struct Refusals(HashMap<Key, Entry>);

impl Refusals {
    /// Notes one refusal at `now`; `Some(repeated)` when it is due a line, with the number of
    /// identical refusals held back since the previous one.
    fn note(&mut self, key: Key, now: Instant) -> Option<u64> {
        if self.0.len() >= SWEEP_ABOVE {
            self.0
                .retain(|_, entry| now.duration_since(entry.logged) < INTERVAL);
        }
        match self.0.entry(key) {
            hash_map::Entry::Occupied(mut held) => {
                let entry = held.get_mut();
                if now.duration_since(entry.logged) < INTERVAL {
                    entry.repeated = entry.repeated.saturating_add(1);
                    return None;
                }
                entry.logged = now;
                Some(std::mem::take(&mut entry.repeated))
            }
            hash_map::Entry::Vacant(first) => {
                first.insert(Entry {
                    logged: now,
                    repeated: 0,
                });
                Some(0)
            }
        }
    }
}

static REFUSALS: LazyLock<Mutex<Refusals>> = LazyLock::new(|| Mutex::new(Refusals::default()));

/// Logs that a token was refused on `path` for `reason`, unless the same client was refused
/// there for the same reason within the interval. `reason` never carries the token itself, only
/// its shape or its digest's fingerprint ([`super::tokens::refusal_reason`]). A poisoned lock
/// logs: a lost count is better than a lost refusal.
pub(super) fn log_refusal(path: &str, reason: &str, client: IpAddr) {
    let key = Key {
        path: path.to_owned(),
        reason: reason.to_owned(),
        client,
    };
    let due = match REFUSALS.lock() {
        Ok(mut refusals) => refusals.note(key, Instant::now()),
        Err(_) => Some(0),
    };
    if let Some(repeated) = due {
        tracing::warn!(
            path,
            reason,
            client = %client,
            repeated,
            "an API token was refused"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::{
        net::{IpAddr, Ipv4Addr},
        time::{Duration, Instant},
    };

    use super::{INTERVAL, Key, Refusals};

    fn key(reason: &str, client: [u8; 4]) -> Key {
        Key {
            path: "/mcp".to_owned(),
            reason: reason.to_owned(),
            client: IpAddr::V4(Ipv4Addr::from(client)),
        }
    }

    const NOT_BEARER: &str = "the Authorization header is not a Bearer credential";

    #[test]
    fn a_repeated_refusal_writes_one_line_per_interval_with_the_count_it_held_back() {
        let mut refusals = Refusals::default();
        let start = Instant::now();
        assert_eq!(
            refusals.note(key(NOT_BEARER, [10, 0, 0, 7]), start),
            Some(0)
        );
        for second in 1..=131 {
            assert_eq!(
                refusals.note(
                    key(NOT_BEARER, [10, 0, 0, 7]),
                    start + Duration::from_secs(second)
                ),
                None
            );
        }
        assert_eq!(
            refusals.note(key(NOT_BEARER, [10, 0, 0, 7]), start + INTERVAL),
            Some(131),
            "the line after the interval says how many it held back"
        );
        assert_eq!(
            refusals.note(
                key(NOT_BEARER, [10, 0, 0, 7]),
                start + INTERVAL + Duration::from_secs(1)
            ),
            None,
            "and the count starts again from there"
        );
    }

    #[test]
    fn another_client_or_reason_is_its_own_refusal() {
        let mut refusals = Refusals::default();
        let now = Instant::now();
        assert_eq!(refusals.note(key(NOT_BEARER, [10, 0, 0, 7]), now), Some(0));
        assert_eq!(refusals.note(key(NOT_BEARER, [10, 0, 0, 8]), now), Some(0));
        assert_eq!(
            refusals.note(key("the Bearer credential is empty", [10, 0, 0, 7]), now),
            Some(0)
        );
        let mut elsewhere = key(NOT_BEARER, [10, 0, 0, 7]);
        elsewhere.path = "/mcp/other".to_owned();
        assert_eq!(refusals.note(elsewhere, now), Some(0));
        assert_eq!(refusals.note(key(NOT_BEARER, [10, 0, 0, 7]), now), None);
    }
}
