//! Script subscriptions (RD-130-19): a script the administrator placed in the scripts
//! directory, run on the subscription's schedule, whose output lines are links.
//!
//! The running is a trait rather than a process call, for the same reason the feed adapter
//! takes a fetcher: where a script may live, how long it may run, how much it may print and
//! what an exit code means are the post-processing sandbox's rules (`rd-extract`), and this
//! crate only turns what came out into items. Everything after that -- the archive that
//! takes an address once, the review list or the queue, the category -- is the poller's, as
//! for every other kind.

use std::{collections::HashSet, sync::Arc};

use async_trait::async_trait;
use url::Url;

use rd_core::{Subscription, SubscriptionKind};

use crate::adapter::{DiscoveredItem, PollOutcome, SourceAdapter};

/// Runs a subscription's script.
#[async_trait]
pub trait ScriptRunner: Send + Sync {
    /// Runs the script called `name` and returns everything it wrote to standard output.
    ///
    /// Fails for a script that cannot be found, exits with anything but 0, runs past its
    /// time limit or prints more than the output limit. A failed run must not return what the
    /// script printed before it failed: half a list is indistinguishable from a whole one.
    async fn run(&self, name: &str, subscription: &Subscription) -> anyhow::Result<String>;
}

/// Polls script subscriptions.
pub struct ScriptAdapter {
    runner: Arc<dyn ScriptRunner>,
}

impl ScriptAdapter {
    #[must_use]
    pub fn new(runner: Arc<dyn ScriptRunner>) -> Self {
        Self { runner }
    }
}

#[async_trait]
impl SourceAdapter for ScriptAdapter {
    fn kind(&self) -> SubscriptionKind {
        SubscriptionKind::Script
    }

    async fn poll(&self, subscription: &Subscription) -> anyhow::Result<PollOutcome> {
        let Some(name) = subscription.script_name() else {
            anyhow::bail!("subscription names no script");
        };
        let output = self.runner.run(name, subscription).await?;
        Ok(PollOutcome {
            items: links_of(&output),
            ..PollOutcome::default()
        })
    }
}

/// Every line of `output` that is one address, in order and once each.
///
/// A line counts when, trimmed, it is a single link the LinkGrabber would take from pasted
/// text -- the same pattern, so a script sees exactly the rules a person pasting its output
/// would. Everything else is the script talking to itself (a progress line, a comment, a
/// line with an address inside a sentence) and is left out rather than guessed at.
#[must_use]
pub fn links_of(output: &str) -> Vec<DiscoveredItem> {
    let mut seen = HashSet::new();
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && line.split_whitespace().count() == 1)
        .filter_map(|line| {
            let mut urls = rd_collector::extract_urls(line);
            (urls.len() == 1).then(|| urls.remove(0))
        })
        .filter(|url| seen.insert(url.as_str().to_owned()))
        .map(|url| DiscoveredItem::new(title_of(&url), url))
        .collect()
}

/// The last path segment, which on a filehoster is the file name; the address otherwise.
fn title_of(url: &Url) -> String {
    url.path_segments()
        .and_then(|mut segments| segments.rfind(|segment| !segment.is_empty()))
        .map_or_else(|| url.to_string(), str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::{ScriptAdapter, ScriptRunner, links_of};
    use crate::adapter::SourceAdapter;

    struct FakeRunner {
        output: anyhow::Result<String>,
        asked: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ScriptRunner for FakeRunner {
        async fn run(
            &self,
            name: &str,
            _subscription: &rd_core::Subscription,
        ) -> anyhow::Result<String> {
            self.asked.lock().expect("lock").push(name.to_owned());
            match &self.output {
                Ok(output) => Ok(output.clone()),
                Err(error) => Err(anyhow::anyhow!("{error}")),
            }
        }
    }

    fn subscription(url: &str) -> rd_core::Subscription {
        rd_core::Subscription {
            id: rd_core::SubscriptionId::new(),
            name: "Daily links".to_owned(),
            source_categories: Vec::new(),
            url: url.parse().expect("url"),
            kind: rd_core::SubscriptionKind::Script,
            enabled: true,
            mode: rd_core::SubscriptionMode::AutoQueue,
            category_id: None,
            priority: rd_core::DownloadPriority::default(),
            interval_seconds: 3_600,
            filters: rd_core::SubscriptionFilters::default(),
            backlog: rd_core::BacklogPolicy::default(),
            category_map: Vec::new(),
            primed: false,
            last_run_at: None,
            next_run_at: None,
            consecutive_failures: 0,
            last_error: None,
            etag: None,
            last_modified: None,
            secret_ref: None,
            has_secret: false,
            every_release: false,
            view: rd_core::SubscriptionView::List,
            autoplay: false,
            card_ratio: rd_core::SubscriptionCardRatio::TwoOne,
            schedule: Some("0 6 * * *".to_owned()),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn runner(output: anyhow::Result<String>) -> Arc<FakeRunner> {
        Arc::new(FakeRunner {
            output,
            asked: Mutex::new(Vec::new()),
        })
    }

    #[test]
    fn only_lines_that_are_one_address_become_links() {
        let output = "\
# collected 2026-09-25
fetching page 1 of 2 ...
https://ddownload.com/abc123/Show.S01E01.1080p.rar

\t https://ddownload.com/def456/Show.S01E02.1080p.rar \t
see https://example.test/not-a-link-line for details
https://ddownload.com/abc123/Show.S01E01.1080p.rar
magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567
ftp://files.example.test/pub/archive.zip
not-a-url
javascript:alert(1)
";
        let links = links_of(output);
        let urls: Vec<&str> = links.iter().map(|item| item.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://ddownload.com/abc123/Show.S01E01.1080p.rar",
                "https://ddownload.com/def456/Show.S01E02.1080p.rar",
                "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567",
                "ftp://files.example.test/pub/archive.zip",
            ]
        );
        // The file name is the title a person sees in the review list.
        assert_eq!(links[0].title, "Show.S01E01.1080p.rar");
        assert_eq!(links[3].title, "archive.zip");
        assert!(links_of("").is_empty());
        assert!(links_of("nothing to see\n\n").is_empty());
    }

    #[test]
    fn windows_line_endings_are_read_like_any_others() {
        // A `.bat` writes CRLF; the trailing `\r` must not become part of the address.
        let links = links_of("https://example.test/a.rar\r\nhttps://example.test/b.rar\r\n");
        let urls: Vec<&str> = links.iter().map(|item| item.url.as_str()).collect();
        assert_eq!(
            urls,
            ["https://example.test/a.rar", "https://example.test/b.rar"]
        );
    }

    #[tokio::test]
    async fn the_script_named_by_the_address_is_run_and_its_links_returned() {
        let fake = runner(Ok("https://example.test/one.rar\n".to_owned()));
        let adapter = ScriptAdapter::new(fake.clone());
        assert_eq!(adapter.kind(), rd_core::SubscriptionKind::Script);
        let outcome = adapter
            .poll(&subscription("script:daily-links.sh"))
            .await
            .expect("poll");
        assert_eq!(outcome.items.len(), 1);
        assert!(!outcome.not_modified);
        assert_eq!(
            fake.asked.lock().expect("lock").as_slice(),
            ["daily-links.sh"]
        );
    }

    #[tokio::test]
    async fn a_failed_run_fails_the_poll_with_its_reason() {
        // The reason is what the run history shows, so it must survive the adapter.
        let fake = runner(Err(anyhow::anyhow!("script exited with status 3")));
        let adapter = ScriptAdapter::new(fake);
        let error = adapter
            .poll(&subscription("script:daily-links.sh"))
            .await
            .expect_err("failed run");
        assert!(error.to_string().contains("status 3"), "{error}");
    }

    #[tokio::test]
    async fn an_address_that_names_no_script_runs_nothing() {
        let fake = runner(Ok(String::new()));
        let adapter = ScriptAdapter::new(fake.clone());
        assert!(
            adapter
                .poll(&subscription("https://example.test/links.sh"))
                .await
                .is_err()
        );
        assert!(fake.asked.lock().expect("lock").is_empty());
    }
}
