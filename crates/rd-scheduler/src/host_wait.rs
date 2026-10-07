//! Files that wait for a connection to their host before they start (RD-1130-02).
//!
//! The per-host connection limit (`rd_http::HostLimits`, six by default) used to be met only
//! inside the engine: the dispatcher started a file, the file took one of the
//! `max_active_files` places, and then all of its chunks queued at the limiter until another
//! file of the same host let a connection go. The row said *Downloading* with 0 B, the log said
//! nothing, and a file of another host that could have run sat queued behind it.
//!
//! The dispatch pass now asks first. A queued HTTP file starts only while its host has a free
//! connection that no file started before it is about to take; otherwise it stays `Queued`,
//! the pass goes on to the next file, and the file is listed as waiting for that host. A file
//! that runs already and whose further chunks wait holds connections and moves bytes; it is
//! left alone. The limit itself is unchanged: this only reads it.
//!
//! A starting file promises its host one connection, not its planned chunks (RD-1140-07). The
//! promise is made to the host of the link as it was added, and a hoster's or a debrid
//! service's link is downloaded from somewhere else: in 1.13.0 the first file of a hoster
//! promised as many chunks as the hoster page had connections, nothing ever took them back,
//! and the hoster's files ran one at a time. A promise therefore also ends when the worker
//! turns to another host, and after [`START_GRACE`] whatever the worker does.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use rd_core::{DownloadFile, DownloadId, DownloadKind};
use url::Url;

use crate::SchedulerHandle;

/// How long a file handed to the engine still counts as starting: until its chunks have
/// queued at the limiter, which the next dispatch pass (two a second) then reads directly.
const HANDOVER_GRACE: Duration = Duration::from_secs(1);

/// How long a started file counts as starting at most. A resolver that waits out a hoster's
/// countdown, or an address that does not answer, would otherwise hold its promise for as
/// long as the attempt lasts.
const START_GRACE: Duration = Duration::from_secs(30);

/// What a queued HTTP file asks of its host's connections: one, for as long as it starts.
pub(crate) struct HostClaim {
    /// The host as the limiter counts it (`rd_core::host_key`).
    pub(crate) host: String,
    /// Connections the host could open right now.
    pub(crate) free: usize,
}

/// A started file whose chunks have not reached the limiter yet.
struct Starting {
    host: String,
    /// When the dispatcher started it.
    started: Instant,
    /// When the worker handed the transfer to the engine; `None` while it still resolves,
    /// connects or probes.
    handed_over: Option<Instant>,
}

/// The dispatcher's view of the host limit, kept under the active set's lock.
#[derive(Default)]
pub(crate) struct HostAdmission {
    starting: HashMap<DownloadId, Starting>,
    /// Queued files the last dispatch pass held back, with the host each waits for.
    waiting: HashMap<DownloadId, String>,
}

impl HostAdmission {
    /// Whether `claim`'s host has a connection left once the files started before it on the
    /// same host take one each. The limiter cannot know those yet: they resolve, connect and
    /// probe before their chunks ask, so three files of one host started in one pass used to
    /// see the same six free connections.
    pub(crate) fn has_room(&self, claim: &HostClaim, now: Instant) -> bool {
        let promised = self
            .starting
            .values()
            .filter(|start| start.host == claim.host && !start.expired(now))
            .count();
        claim.free > promised
    }

    pub(crate) fn start(&mut self, id: DownloadId, claim: HostClaim, now: Instant) {
        self.starting.insert(
            id,
            Starting {
                host: claim.host,
                started: now,
                handed_over: None,
            },
        );
    }

    /// The file's chunks are about to ask the limiter themselves.
    fn handed_over(&mut self, id: DownloadId, now: Instant) {
        if let Some(start) = self.starting.get_mut(&id) {
            start.handed_over = Some(now);
        }
    }

    /// The worker turns to `host`: a hoster's download server, where a redirect led. The
    /// connections it opens there are not the ones it promised to the host it started on.
    fn turned_to(&mut self, id: DownloadId, host: &str) {
        if self
            .starting
            .get(&id)
            .is_some_and(|start| start.host != host)
        {
            self.starting.remove(&id);
        }
    }

    /// The attempt ended, however it ended.
    pub(crate) fn finished(&mut self, id: &DownloadId) {
        self.starting.remove(id);
    }

    /// Takes the waits one dispatch pass found, saying in the log which began and which ended.
    pub(crate) fn settle(&mut self, waiting: HashMap<DownloadId, String>, now: Instant) {
        self.starting.retain(|_, start| !start.expired(now));
        for (id, host) in &waiting {
            if !self.waiting.contains_key(id) {
                tracing::debug!(download_id = %id, %host, "download waits for a connection to its host");
            }
        }
        for (id, host) in &self.waiting {
            if !waiting.contains_key(id) {
                tracing::debug!(download_id = %id, %host, "download no longer waits for a connection to its host");
            }
        }
        self.waiting = waiting;
    }
}

impl Starting {
    fn expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) >= START_GRACE
            || self
                .handed_over
                .is_some_and(|since| now.saturating_duration_since(since) >= HANDOVER_GRACE)
    }
}

impl SchedulerHandle {
    /// What `file` asks of its host's connections, or `None` when the host limit does not
    /// apply: another kind of download, the limit switched off, an address without a host.
    pub(crate) fn host_claim(&self, file: &DownloadFile) -> Option<HostClaim> {
        if file.kind != DownloadKind::Http {
            return None;
        }
        let free = self.host_limits().free(&file.source)?;
        let host = rd_core::host_key(file.source.host_str()?);
        Some(HostClaim { host, free })
    }

    /// The worker hands the transfer of `id` to the engine.
    pub(crate) async fn host_handed_over(&self, id: DownloadId) {
        self.active
            .lock()
            .await
            .host
            .handed_over(id, Instant::now());
    }

    /// The worker of `id` goes on at `address`: a resolver's answer, the end of a redirect.
    /// On another host than the one it started on, its promise ends here (RD-1140-07).
    pub(crate) async fn host_turned_to(&self, id: DownloadId, address: &Url) {
        let Some(host) = address.host_str().map(rd_core::host_key) else {
            return;
        };
        self.active.lock().await.host.turned_to(id, &host);
    }

    /// Queued downloads held back because their host has no free connection, with the host
    /// each waits for (RD-1130-02). Runtime state only, as of the last dispatch pass.
    pub async fn host_waits(&self) -> HashMap<DownloadId, String> {
        self.active.lock().await.host.waiting.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use rd_core::DownloadId;

    use super::{HANDOVER_GRACE, HostAdmission, HostClaim, START_GRACE};

    fn claim(host: &str, free: usize) -> HostClaim {
        HostClaim {
            host: host.to_owned(),
            free,
        }
    }

    #[test]
    fn files_started_on_one_host_promise_one_connection_each_until_their_chunks_ask() {
        let now = Instant::now();
        let mut admission = HostAdmission::default();
        assert!(admission.has_room(&claim("a.test", 2), now));
        let first = DownloadId::new();
        admission.start(first, claim("a.test", 2), now);
        assert!(
            admission.has_room(&claim("a.test", 2), now),
            "one of two is left"
        );
        admission.start(DownloadId::new(), claim("a.test", 2), now);
        assert!(!admission.has_room(&claim("a.test", 2), now));
        assert!(
            admission.has_room(&claim("b.test", 2), now),
            "another host has its own connections"
        );

        // Handed to the engine, the first file's chunks are counted by the limiter instead.
        admission.handed_over(first, now);
        assert!(!admission.has_room(&claim("a.test", 2), now));
        let later = now + HANDOVER_GRACE + Duration::from_millis(1);
        admission.settle(std::collections::HashMap::new(), later);
        assert!(admission.has_room(&claim("a.test", 2), later));
    }

    /// RD-1140-07: a file planning six chunks on a host with six free connections let no
    /// other file of that host start; a hoster's files then ran one at a time.
    #[test]
    fn a_file_does_not_promise_its_chunks() {
        let now = Instant::now();
        let mut admission = HostAdmission::default();
        for _ in 0..5 {
            assert!(admission.has_room(&claim("hoster.test", 6), now));
            admission.start(DownloadId::new(), claim("hoster.test", 6), now);
        }
        assert!(admission.has_room(&claim("hoster.test", 6), now));
        assert!(
            !admission.has_room(&claim("hoster.test", 0), now),
            "a host whose connections are all taken has no room"
        );
    }

    #[test]
    fn a_promise_ends_when_the_worker_turns_to_another_host() {
        let now = Instant::now();
        let mut admission = HostAdmission::default();
        let id = DownloadId::new();
        admission.start(id, claim("hoster.test", 1), now);
        admission.turned_to(id, "hoster.test");
        assert!(
            !admission.has_room(&claim("hoster.test", 1), now),
            "still on the host it started on"
        );
        admission.turned_to(id, "cdn.test");
        assert!(admission.has_room(&claim("hoster.test", 1), now));
    }

    #[test]
    fn a_promise_ends_after_the_start_grace_without_a_handover() {
        let now = Instant::now();
        let mut admission = HostAdmission::default();
        admission.start(DownloadId::new(), claim("hoster.test", 1), now);
        let before = now + START_GRACE - Duration::from_millis(1);
        assert!(!admission.has_room(&claim("hoster.test", 1), before));
        let after = now + START_GRACE;
        assert!(admission.has_room(&claim("hoster.test", 1), after));
        admission.settle(std::collections::HashMap::new(), after);
        assert!(admission.starting.is_empty());
    }

    #[test]
    fn the_waits_of_the_last_pass_replace_the_earlier_ones() {
        let now = Instant::now();
        let mut admission = HostAdmission::default();
        let id = DownloadId::new();
        admission.settle([(id, "a.test".to_owned())].into(), now);
        assert_eq!(
            admission.waiting.get(&id).map(String::as_str),
            Some("a.test")
        );
        admission.settle(std::collections::HashMap::new(), now);
        assert!(admission.waiting.is_empty());
    }
}
