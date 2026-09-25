//! Per-hoster hold-off after a free-download IP limit.
//!
//! When a hoster answers "one free download at a time" or "wait 30 minutes", the limit
//! applies to the whole IP, not to the one link that hit it. Retrying the next link of that
//! hoster would spend another wait and another (possibly paid) captcha just to be refused
//! again, so the hoster is held back as a whole while other hosters keep downloading.
//!
//! Premium downloads are unaffected: the limit is a property of anonymous access.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use chrono::{DateTime, Utc};
use url::Url;

/// Hosts currently under an IP block, with the instant each one frees up again.
#[derive(Clone, Default)]
pub(crate) struct HostBlocks {
    blocked: Arc<Mutex<HashMap<String, DateTime<Utc>>>>,
}

impl HostBlocks {
    /// Holds `source`'s hoster back until `until`, keeping the longest block already set.
    pub(crate) fn block(&self, source: &Url, until: DateTime<Utc>) {
        let Some(host) = block_key(source) else {
            return;
        };
        let Ok(mut blocked) = self.blocked.lock() else {
            return;
        };
        blocked
            .entry(host)
            .and_modify(|current| *current = (*current).max(until))
            .or_insert(until);
    }

    /// Whether an account-less download from `source` must wait. Expired entries are
    /// dropped as they are encountered, so the map stays as small as the active queue.
    pub(crate) fn blocked_until(&self, source: &Url, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let host = block_key(source)?;
        let mut blocked = self.blocked.lock().ok()?;
        match blocked.get(&host).copied() {
            Some(until) if until > now => Some(until),
            Some(_) => {
                blocked.remove(&host);
                None
            }
            None => None,
        }
    }

    /// The hosts still held back at `now`, soonest to free up first.
    ///
    /// Reported so a reconnect can say what it is waiting on, and so the interface can explain
    /// a queue that looks stalled but is deliberately waiting out a limit.
    pub(crate) fn active(&self, now: DateTime<Utc>) -> Vec<(String, DateTime<Utc>)> {
        let Ok(mut blocked) = self.blocked.lock() else {
            return Vec::new();
        };
        blocked.retain(|_, until| *until > now);
        let mut active: Vec<(String, DateTime<Utc>)> = blocked
            .iter()
            .map(|(host, until)| (host.clone(), *until))
            .collect();
        active.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));
        active
    }

    /// Forgets every block. Used after a reconnect: the limits were tied to the old address.
    pub(crate) fn clear(&self) {
        if let Ok(mut blocked) = self.blocked.lock() {
            blocked.clear();
        }
    }
}

/// Groups a hoster's aliases onto one key: `www.` is noise, and the limit applies per host.
fn block_key(source: &Url) -> Option<String> {
    let host = source.host_str()?;
    Some(
        host.strip_prefix("www.")
            .unwrap_or(host)
            .to_ascii_lowercase(),
    )
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use url::Url;

    use super::{HostBlocks, block_key};

    fn url(value: &str) -> Url {
        value.parse().expect("URL")
    }

    #[test]
    fn a_block_covers_every_link_of_the_same_hoster_but_no_other() {
        let blocks = HostBlocks::default();
        let now = Utc::now();
        blocks.block(
            &url("https://rapidgator.net/file/one"),
            now + Duration::minutes(15),
        );

        assert!(
            blocks
                .blocked_until(&url("https://rapidgator.net/file/two"), now)
                .is_some(),
            "another link of the blocked hoster must wait"
        );
        assert!(
            blocks
                .blocked_until(&url("https://www.rapidgator.net/file/three"), now)
                .is_some(),
            "www. is the same hoster"
        );
        assert!(
            blocks
                .blocked_until(&url("https://katfile.biz/file/four"), now)
                .is_none(),
            "a different hoster keeps downloading"
        );
    }

    #[test]
    fn an_expired_block_is_forgotten() {
        let blocks = HostBlocks::default();
        let now = Utc::now();
        blocks.block(&url("https://katfile.biz/a"), now - Duration::seconds(1));

        assert!(
            blocks
                .blocked_until(&url("https://katfile.biz/a"), now)
                .is_none()
        );
    }

    /// A hoster that reports a long limit must not have it shortened by a later, smaller one.
    #[test]
    fn the_longest_block_wins() {
        let blocks = HostBlocks::default();
        let now = Utc::now();
        let long = now + Duration::minutes(60);
        blocks.block(&url("https://ddownload.com/a"), long);
        blocks.block(&url("https://ddownload.com/b"), now + Duration::minutes(5));

        assert_eq!(
            blocks.blocked_until(&url("https://ddownload.com/c"), now),
            Some(long)
        );
    }

    #[test]
    fn hosts_are_normalised_and_hostless_urls_are_ignored() {
        assert_eq!(
            block_key(&url("https://WWW.Rapidgator.NET/x")).as_deref(),
            Some("rapidgator.net")
        );
        assert_eq!(block_key(&url("file:///tmp/x")), None);
    }
}

#[cfg(test)]
mod reporting_tests {
    use chrono::{Duration, Utc};
    use url::Url;

    use super::HostBlocks;

    fn url(value: &str) -> Url {
        Url::parse(value).expect("url")
    }

    #[test]
    fn active_blocks_are_reported_soonest_first_and_expired_ones_are_gone() {
        let now = Utc::now();
        let blocks = HostBlocks::default();
        blocks.block(&url("https://late.example/a"), now + Duration::minutes(30));
        blocks.block(&url("https://soon.example/a"), now + Duration::minutes(5));
        blocks.block(&url("https://gone.example/a"), now - Duration::minutes(1));

        let active = blocks.active(now);

        let hosts: Vec<&str> = active.iter().map(|(host, _)| host.as_str()).collect();
        assert_eq!(hosts, ["soon.example", "late.example"]);
    }

    #[test]
    fn clearing_lets_every_host_run_again() {
        let now = Utc::now();
        let blocks = HostBlocks::default();
        blocks.block(&url("https://one.example/a"), now + Duration::hours(1));

        blocks.clear();

        assert!(blocks.active(now).is_empty());
        assert_eq!(
            blocks.blocked_until(&url("https://one.example/a"), now),
            None
        );
    }
}
