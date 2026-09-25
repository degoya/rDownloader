//! The transfer statistics REST surface (RD-110-01): `GET /api/v1/stats/transfers`, and the
//! retention settings the sweep reads.

use axum::{
    Json,
    extract::{Query, State},
};
use chrono::{DateTime, Duration, Utc};
use rd_db::{StatsResolution, StatsRetention, TransferBucket};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{AppState, dto::SettingsResponse, error::ApiError};

/// Bounds of the two retention settings, in days.
pub(crate) const MIN_HOURLY_DAYS: u32 = 1;
pub(crate) const MIN_RETENTION_DAYS: u32 = 7;
pub(crate) const MAX_RETENTION_DAYS: u32 = 3650;
pub(crate) const DEFAULT_HOURLY_DAYS: u32 = 30;
pub(crate) const DEFAULT_RETENTION_DAYS: u32 = 365;

/// How far back the statistics reach.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StatsRange {
    #[default]
    Day,
    Week,
    Month,
    Year,
}

impl StatsRange {
    fn since(self, now: DateTime<Utc>) -> DateTime<Utc> {
        match self {
            Self::Day => now - Duration::hours(24),
            Self::Week => now - Duration::days(7),
            Self::Month => now - Duration::days(30),
            Self::Year => now - Duration::days(365),
        }
    }

    /// One bar per hour for a day, per day for anything longer.
    fn resolution(self) -> StatsBucketWidth {
        match self {
            Self::Day => StatsBucketWidth::Hour,
            Self::Week | Self::Month | Self::Year => StatsBucketWidth::Day,
        }
    }
}

/// The width of the buckets a response is folded to; `rd_db::StatsResolution` as the
/// contract names it, kept apart because the store crate carries no OpenAPI schema.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StatsBucketWidth {
    Hour,
    Day,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct TransferStatsParams {
    /// `day` (hourly buckets), `week`, `month` or `year` (daily buckets); `day` by default.
    #[serde(default)]
    pub range: StatsRange,
}

/// The five figures every row of the statistics carries.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, ToSchema)]
pub struct TransferStatsFigures {
    pub completed: u64,
    pub failed: u64,
    pub retries: u64,
    pub bytes: u64,
    /// Seconds from creation to completion, summed over the completed transfers.
    pub seconds: u64,
}

impl TransferStatsFigures {
    fn add(&mut self, bucket: &TransferBucket) {
        self.completed += bucket.completed.max(0) as u64;
        self.failed += bucket.failed.max(0) as u64;
        self.retries += bucket.retries.max(0) as u64;
        self.bytes += bucket.bytes.max(0) as u64;
        self.seconds += bucket.seconds.max(0) as u64;
    }
}

/// One bucket of the range, summed over every kind and provider.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct TransferStatsBucket {
    /// Start of the bucket, RFC 3339 in UTC.
    pub start: String,
    #[serde(flatten)]
    pub figures: TransferStatsFigures,
}

/// The range's figures for one kind or one provider.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct TransferStatsGroup {
    pub key: String,
    #[serde(flatten)]
    pub figures: TransferStatsFigures,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct TransferStatsResponse {
    pub range: StatsRange,
    pub resolution: StatsBucketWidth,
    pub since: DateTime<Utc>,
    pub buckets: Vec<TransferStatsBucket>,
    pub by_kind: Vec<TransferStatsGroup>,
    pub by_provider: Vec<TransferStatsGroup>,
    /// The whole range.
    pub totals: TransferStatsFigures,
    /// Everything ever recorded, untouched by the retention.
    pub all_time: TransferStatsFigures,
}

/// Transfer statistics over a range, from the persistent buckets.
#[utoipa::path(
    get,
    path = "/api/v1/stats/transfers",
    tag = "system",
    params(TransferStatsParams),
    responses((status = 200, body = TransferStatsResponse))
)]
pub async fn transfer_stats(
    State(state): State<AppState>,
    Query(params): Query<TransferStatsParams>,
) -> Result<Json<TransferStatsResponse>, ApiError> {
    let now = Utc::now();
    let since = params.range.since(now);
    // Both widths are read: an hour older than the hourly window has already been folded
    // into its day, and a day view must not lose the hours that have not been folded yet.
    let mut rows = state
        .database
        .list_transfer_stats(StatsResolution::Hour, since)
        .await?;
    rows.extend(
        state
            .database
            .list_transfer_stats(StatsResolution::Day, since)
            .await?,
    );
    let all_time = state.database.list_transfer_totals().await?.iter().fold(
        TransferStatsFigures::default(),
        |mut figures, total| {
            figures.completed += total.completed.max(0) as u64;
            figures.failed += total.failed.max(0) as u64;
            figures.retries += total.retries.max(0) as u64;
            figures.bytes += total.bytes.max(0) as u64;
            figures.seconds += total.seconds.max(0) as u64;
            figures
        },
    );
    Ok(Json(summarise(params.range, since, &rows, all_time)))
}

/// Folds the rows into the response shape; pure, so it is tested without a database.
pub(crate) fn summarise(
    range: StatsRange,
    since: DateTime<Utc>,
    rows: &[TransferBucket],
    all_time: TransferStatsFigures,
) -> TransferStatsResponse {
    use std::collections::BTreeMap;
    let resolution = range.resolution();
    let mut buckets: BTreeMap<String, TransferStatsFigures> = BTreeMap::new();
    let mut by_kind: BTreeMap<String, TransferStatsFigures> = BTreeMap::new();
    let mut by_provider: BTreeMap<String, TransferStatsFigures> = BTreeMap::new();
    let mut totals = TransferStatsFigures::default();
    for row in rows {
        let start = match resolution {
            StatsBucketWidth::Hour => row.bucket_start.clone(),
            StatsBucketWidth::Day => format!(
                "{}T00:00:00Z",
                &row.bucket_start[..10.min(row.bucket_start.len())]
            ),
        };
        buckets.entry(start).or_default().add(row);
        by_kind.entry(row.kind.clone()).or_default().add(row);
        by_provider
            .entry(row.provider.clone())
            .or_default()
            .add(row);
        totals.add(row);
    }
    let group = |map: BTreeMap<String, TransferStatsFigures>| {
        map.into_iter()
            .map(|(key, figures)| TransferStatsGroup { key, figures })
            .collect()
    };
    TransferStatsResponse {
        range,
        resolution,
        since,
        buckets: buckets
            .into_iter()
            .map(|(start, figures)| TransferStatsBucket { start, figures })
            .collect(),
        by_kind: group(by_kind),
        by_provider: group(by_provider),
        totals,
        all_time,
    }
}

/// The retention the settings blob asks for, checked at save time.
pub(crate) fn validate_stats_settings(settings: &SettingsResponse) -> Result<(), ApiError> {
    let hourly = settings.stats_hourly_days;
    let retention = settings.stats_retention_days;
    if !(MIN_HOURLY_DAYS..=MAX_RETENTION_DAYS).contains(&hourly)
        || !(MIN_RETENTION_DAYS..=MAX_RETENTION_DAYS).contains(&retention)
        || hourly > retention
    {
        return Err(ApiError::bad_request(
            "settings.stats_retention_invalid",
            "Hourly statistics must be kept between 1 day and the retention, and the retention \
             between 7 and 3650 days",
        )
        .with_param("hourly_days", hourly)
        .with_param("retention_days", retention));
    }
    Ok(())
}

/// The retention as the sweep needs it.
pub(crate) fn retention_of(settings: &SettingsResponse) -> StatsRetention {
    StatsRetention {
        hourly_days: settings.stats_hourly_days,
        retention_days: settings.stats_retention_days,
    }
}

#[cfg(test)]
mod tests {
    use super::{StatsRange, TransferStatsFigures, summarise, validate_stats_settings};
    use crate::dto::SettingsResponse;
    use rd_db::TransferBucket;

    fn row(start: &str, kind: &str, provider: &str, bytes: i64) -> TransferBucket {
        TransferBucket {
            bucket_start: start.to_owned(),
            kind: kind.to_owned(),
            provider: provider.to_owned(),
            completed: 1,
            failed: 0,
            retries: 2,
            bytes,
            seconds: 30,
        }
    }

    /// Hour rows fold into their day for a week view, and every group sums the same rows.
    #[test]
    fn a_week_folds_hours_into_days_and_groups_by_kind_and_provider() {
        let rows = [
            row("2026-09-19T10:00:00Z", "http", "direct", 100),
            row("2026-09-19T11:00:00Z", "http", "rapidgator", 200),
            row("2026-09-20T00:00:00Z", "usenet", "direct", 300),
        ];
        let since = "2026-09-14T00:00:00Z".parse().expect("moment");
        let response = summarise(
            StatsRange::Week,
            since,
            &rows,
            TransferStatsFigures::default(),
        );
        let starts: Vec<_> = response
            .buckets
            .iter()
            .map(|bucket| bucket.start.as_str())
            .collect();
        assert_eq!(starts, ["2026-09-19T00:00:00Z", "2026-09-20T00:00:00Z"]);
        assert_eq!(response.buckets[0].figures.bytes, 300);
        assert_eq!(response.totals.bytes, 600);
        assert_eq!(response.totals.retries, 6);
        let kinds: Vec<_> = response
            .by_kind
            .iter()
            .map(|group| (group.key.as_str(), group.figures.bytes))
            .collect();
        assert_eq!(kinds, [("http", 300), ("usenet", 300)]);
        let providers: Vec<_> = response
            .by_provider
            .iter()
            .map(|group| (group.key.as_str(), group.figures.bytes))
            .collect();
        assert_eq!(providers, [("direct", 400), ("rapidgator", 200)]);
    }

    /// A day view keeps the hours apart.
    #[test]
    fn a_day_keeps_hourly_buckets() {
        let rows = [
            row("2026-09-20T10:00:00Z", "http", "direct", 1),
            row("2026-09-20T11:00:00Z", "http", "direct", 2),
        ];
        let since = "2026-09-19T12:00:00Z".parse().expect("moment");
        let response = summarise(
            StatsRange::Day,
            since,
            &rows,
            TransferStatsFigures::default(),
        );
        assert_eq!(response.buckets.len(), 2);
        assert_eq!(response.resolution, super::StatsBucketWidth::Hour);
    }

    /// The bounds, and the one relation between the two settings.
    #[test]
    fn retention_settings_are_bounded() {
        let mut settings = SettingsResponse::default();
        assert!(validate_stats_settings(&settings).is_ok());
        settings.stats_hourly_days = 0;
        assert_eq!(
            validate_stats_settings(&settings).expect_err("zero").code(),
            "settings.stats_retention_invalid"
        );
        settings.stats_hourly_days = 400;
        settings.stats_retention_days = 365;
        assert!(
            validate_stats_settings(&settings).is_err(),
            "hourly beyond the retention"
        );
        settings.stats_hourly_days = 30;
        settings.stats_retention_days = 6;
        assert!(
            validate_stats_settings(&settings).is_err(),
            "retention below a week"
        );
        settings.stats_retention_days = 3651;
        assert!(
            validate_stats_settings(&settings).is_err(),
            "retention beyond ten years"
        );
    }
}
