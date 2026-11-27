//! NZBs the online check already read, kept for the enqueue (RD-1490-02).
//!
//! An indexer proxy such as Prowlarr refuses the probe's `HEAD`, ignores its one-byte range and
//! sends the whole NZB — and counts that answer as a grab. The check reads it to the end and
//! keeps it here, so queueing the link takes this copy instead of grabbing the NZB again.
//!
//! In memory and bounded: what does not fit, outlives [`rd_torrent::PREFETCH_TTL`] or is lost
//! with a restart is fetched again by the enqueue, as before. The copy belongs to one candidate
//! at one address and is handed out once.

use std::{
    collections::VecDeque,
    sync::{Mutex, MutexGuard, PoisonError},
    time::Instant,
};

use rd_core::CandidateId;

/// What all kept NZBs may hold together: one NZB at the size an indexer may hand out.
const KEPT_BYTES: usize = rd_collector::MAX_NZB_BYTES;

struct Kept {
    candidate: CandidateId,
    url: url::Url,
    document: rd_http::FetchedDocument,
    at: Instant,
}

#[derive(Default)]
pub(super) struct KeptNzbs {
    /// Oldest first, so the bound drops the copy least likely to be queued still.
    entries: Mutex<VecDeque<Kept>>,
}

impl KeptNzbs {
    /// Keeps the NZB `candidate` answered at `url`, in place of one kept for it before.
    pub(super) fn keep(
        &self,
        candidate: CandidateId,
        url: &url::Url,
        document: rd_http::FetchedDocument,
    ) {
        if document.bytes.len() > KEPT_BYTES {
            return;
        }
        let mut entries = self.lock();
        entries.retain(|kept| kept.candidate != candidate && fresh(kept));
        entries.push_back(Kept {
            candidate,
            url: url.clone(),
            document,
            at: Instant::now(),
        });
        while entries
            .iter()
            .map(|kept| kept.document.bytes.len())
            .sum::<usize>()
            > KEPT_BYTES
        {
            entries.pop_front();
        }
    }

    /// Hands out the NZB kept for `candidate`, once, while it is fresh and the link still points
    /// where it was read from.
    pub(super) fn take(
        &self,
        candidate: CandidateId,
        url: &url::Url,
    ) -> Option<rd_http::FetchedDocument> {
        let mut entries = self.lock();
        entries.retain(fresh);
        let position = entries
            .iter()
            .position(|kept| kept.candidate == candidate)?;
        let kept = entries.remove(position)?;
        (kept.url == *url).then_some(kept.document)
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<Kept>> {
        // Every write is a push, a pop or a removal; none can panic halfway.
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn fresh(kept: &Kept) -> bool {
    kept.at.elapsed() < rd_torrent::PREFETCH_TTL
}

#[cfg(test)]
mod tests {
    use super::{KEPT_BYTES, KeptNzbs};

    fn document(bytes: usize) -> rd_http::FetchedDocument {
        rd_http::FetchedDocument {
            bytes: vec![b'x'; bytes],
            headers: reqwest::header::HeaderMap::new(),
        }
    }

    fn url(path: &str) -> url::Url {
        format!("http://indexer.test/{path}").parse().expect("URL")
    }

    #[test]
    fn a_kept_nzb_is_handed_out_once_and_only_for_its_address() {
        let kept = KeptNzbs::default();
        let candidate = rd_core::CandidateId::new();
        kept.keep(candidate, &url("a"), document(3));

        assert!(kept.take(rd_core::CandidateId::new(), &url("a")).is_none());
        assert_eq!(
            kept.take(candidate, &url("a")).map(|doc| doc.bytes.len()),
            Some(3)
        );
        assert!(kept.take(candidate, &url("a")).is_none(), "handed out once");

        kept.keep(candidate, &url("a"), document(3));
        assert!(
            kept.take(candidate, &url("b")).is_none(),
            "the link was edited"
        );
        assert!(kept.take(candidate, &url("a")).is_none());
    }

    #[test]
    fn the_bound_drops_the_oldest_copy() {
        let kept = KeptNzbs::default();
        let (old, new) = (rd_core::CandidateId::new(), rd_core::CandidateId::new());
        kept.keep(old, &url("old"), document(KEPT_BYTES / 2 + 1));
        kept.keep(new, &url("new"), document(KEPT_BYTES / 2 + 1));

        assert!(kept.take(old, &url("old")).is_none());
        assert!(kept.take(new, &url("new")).is_some());
    }
}
