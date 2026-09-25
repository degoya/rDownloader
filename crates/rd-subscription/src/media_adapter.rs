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
            .probe(&subscription.url)
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
    use super::parse_upload_date;

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
