//! Whether the browser extension announces an intake itself (RD-1270-05).
//!
//! The extension tells the person about every hand-over it makes -- links from the context menu
//! or the popup, an intercepted download, a Click'n'Load, a file. The same hand-over is a
//! `collector.intake` on the bus, and the web tab and the capture agent turn that into a notice
//! of their own: two notices for one hand-over. So the capture routes the extension uses run
//! their intake inside [`announced_by_extension`], and the intake event carries
//! [`PAYLOAD_KEY`] `= true`; the tab and the agent leave such an intake to the extension. Every
//! other intake -- the agent's own Click'n'Load and files, a paste, a hot folder, a feed -- is
//! announced as before.
//!
//! A task-local rather than a field on every intake type: the mark belongs to the request, not
//! to the batch, and the paths a capture route reaches the store by are many (links, a link
//! container, a torrent, an NZB). Each of them ends in `Database::add_collector_batch` or
//! `Database::add_nzb_import`, which run in the request's task, read the mark there and hand it
//! to the writer with the command.

use std::future::Future;

/// The key of the intake event's payload that says the extension announces it.
pub const PAYLOAD_KEY: &str = "extension_notice";

tokio::task_local! {
    static BY_EXTENSION: bool;
}

/// Runs `future` as a hand-over the browser extension announces itself.
pub async fn announced_by_extension<F: Future>(future: F) -> F::Output {
    BY_EXTENSION.scope(true, future).await
}

/// Whether the intake being stored right now is one the extension announces.
pub(crate) fn by_extension() -> bool {
    BY_EXTENSION.try_with(|flag| *flag).unwrap_or(false)
}

/// Marks an intake event's payload as the extension's to announce; leaves it alone otherwise.
pub(crate) fn mark(payload: &mut serde_json::Value, by_extension: bool) {
    if by_extension && let Some(payload) = payload.as_object_mut() {
        payload.insert(PAYLOAD_KEY.to_owned(), true.into());
    }
}

#[cfg(test)]
mod tests {
    use super::{PAYLOAD_KEY, announced_by_extension, by_extension, mark};

    #[tokio::test]
    async fn only_a_scoped_intake_is_the_extensions() {
        assert!(
            !by_extension(),
            "outside a scope nothing is the extension's"
        );
        assert!(announced_by_extension(async { by_extension() }).await);
        assert!(!by_extension(), "the mark ends with its scope");
    }

    #[test]
    fn the_payload_is_marked_only_for_the_extension() {
        let mut marked = serde_json::json!({ "candidate_count": 2 });
        mark(&mut marked, true);
        assert_eq!(marked[PAYLOAD_KEY], true);
        let mut plain = serde_json::json!({ "candidate_count": 2 });
        mark(&mut plain, false);
        assert!(plain.get(PAYLOAD_KEY).is_none());
    }
}
