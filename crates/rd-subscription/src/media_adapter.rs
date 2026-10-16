//! Channel, user and playlist subscriptions through yt-dlp (RD-080-07).
//!
//! Reuses the existing flat-playlist probe rather than adding a second way to talk to
//! yt-dlp: the probe already bounds the entry count, maps tool errors onto the retry policy,
//! and knows where the binary lives. What this adds is only the translation from a probed
//! entry into a [`DiscoveredItem`].

use async_trait::async_trait;
use std::sync::Arc;

use rd_core::{Subscription, SubscriptionKind};

use crate::adapter::{DiscoveredItem, PollOutcome, SourceAdapter};

/// Polls yt-dlp channel, user and playlist pages.
pub struct MediaAdapter {
    probe: Arc<dyn rd_media::MediaProbe>,
}

impl MediaAdapter {
    #[must_use]
    pub fn new(probe: Arc<dyn rd_media::MediaProbe>) -> Self {
        Self { probe }
    }
}

#[async_trait]
impl SourceAdapter for MediaAdapter {
    fn kind(&self) -> SubscriptionKind {
        SubscriptionKind::Media
    }

    async fn poll(&self, subscription: &Subscription) -> anyhow::Result<PollOutcome> {
        let entries = self
            .probe
            // A subscription carries no profile of its own: the one whose scope matches the
            // channel, as for a download left on automatic (RD-1320-02).
            .probe(&subscription.url, rd_core::AuthProfileSelection::Auto)
            .await
            .map_err(|failure| anyhow::anyhow!(failure.message))?;
        let items = entries
            .into_iter()
            .map(|entry| {
                let info = entry.info;
                DiscoveredItem {
                    media_type: None,
                    // The extractor's own id is what the site means by "the same video",
                    // and unlike the page URL it survives the site rewriting its links.
                    source_id: info.video_id.clone(),
                    title: info.title.clone(),
                    url: info.page_url.clone(),
                    // `upload_date` is `YYYYMMDD`, exactly as the extractor reports it.
                    published_at: info.upload_date.as_deref().and_then(parse_upload_date),
                    duration_seconds: info.duration_seconds,
                    language: None,
                    height: info
                        .selected_variant()
                        .and_then(|variant| variant.height)
                        .or_else(|| info.variants.iter().filter_map(|v| v.height).max()),
                    published_raw: info.upload_date,
                    source_category: None,
                    // The extractor's metadata reaches the queue through `MediaInfo`, not
                    // through the indexer attribute map.
                    attributes: std::collections::BTreeMap::new(),
                    // The extractor's video id already says "the same video"; a release name
                    // is what a page without one has (RD-110-21).
                    release_key: None,
                    password: None,
                    refused: None,
                }
            })
            .collect();
        Ok(PollOutcome {
            items,
            ..PollOutcome::default()
        })
    }
}

/// Parses yt-dlp's `YYYYMMDD` upload date as midnight UTC.
///
/// The date carries no time of day, so any instant within it would be a guess; midnight is
/// the one that keeps ordering and comparisons honest.
#[must_use]
pub fn parse_upload_date(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let value = value.trim();
    if value.len() != 8 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let year = value[0..4].parse().ok()?;
    let month = value[4..6].parse().ok()?;
    let day = value[6..8].parse().ok()?;
    chrono::NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|naive| naive.and_utc())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use rd_core::{AuthProfileSelection, Failure, MediaCandidate, SubscriptionKind};

    use super::{MediaAdapter, parse_upload_date};
    use crate::adapter::SourceAdapter;

    /// Remembers the profile choice each probe was asked with.
    #[derive(Default)]
    struct RecordingProbe {
        asked: Mutex<Vec<AuthProfileSelection>>,
    }

    #[async_trait::async_trait]
    impl rd_media::MediaProbe for RecordingProbe {
        async fn probe(
            &self,
            _url: &url::Url,
            auth: AuthProfileSelection,
        ) -> Result<Vec<MediaCandidate>, Failure> {
            self.asked.lock().expect("lock").push(auth);
            Ok(Vec::new())
        }
    }

    /// RD-1320-02: a channel is checked with the profile whose scope matches it, as a download
    /// left on automatic is.
    #[tokio::test]
    async fn a_media_subscription_probes_with_the_matching_profile() {
        let probe = Arc::new(RecordingProbe::default());
        let adapter = MediaAdapter::new(probe.clone());
        let subscription = crate::test_support::subscription(
            "Channel",
            SubscriptionKind::Media,
            "https://www.youtube.com/@channel",
        );
        adapter.poll(&subscription).await.expect("poll");
        assert_eq!(
            *probe.asked.lock().expect("lock"),
            [AuthProfileSelection::Auto]
        );
    }

    #[test]
    fn a_yyyymmdd_date_becomes_midnight_utc() {
        let parsed = parse_upload_date("20260204").expect("date");
        assert_eq!(parsed.to_rfc3339(), "2026-02-04T00:00:00+00:00");
    }

    #[test]
    fn anything_that_is_not_that_shape_is_refused() {
        // A wrong guess here silently shifts an item across a backlog cutoff, so the parser
        // says "no" rather than improvising.
        for value in ["", "2026-02-04", "202602", "2026020x", "20261352"] {
            assert!(parse_upload_date(value).is_none(), "{value}");
        }
    }
}
