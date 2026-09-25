//! The Prometheus/OpenMetrics exposition (RD-110-01): `GET /api/v1/metrics`.
//!
//! Every figure is derived at scrape time from the queue, the scheduler and the statistics
//! tables; nothing is counted in memory, so a restart loses nothing and two instances of the
//! collector cannot disagree. The counters read `transfer_totals`, which the retention sweep
//! never touches, and are therefore monotonic for the life of the database.
//!
//! ## The label budget
//!
//! Label values come only from closed sets: the download kind and state enums, the outcome,
//! the provider id an account carries, and the id of a storage root. Never a URL, a file or
//! package name, an account label, a user name or a host. `tests/metrics.rs` grows a queue by
//! hundreds of rows and checks that the number of series does not move, and that none of the
//! names it used appear in the text.

use std::{collections::BTreeMap, sync::OnceLock, time::Instant};

use axum::{
    extract::State,
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};
use rd_core::{DownloadFile, DownloadState};
use rd_files::StorageTarget;

use crate::{
    AppState,
    error::ApiError,
    metrics_format::{Family, MetricKind, Sample, histogram, render},
};

/// The OpenMetrics media type, answered when the scraper asks for it.
pub(crate) const OPENMETRICS_CONTENT_TYPE: &str =
    "application/openmetrics-text; version=1.0.0; charset=utf-8";
/// The Prometheus text format, the default and what every scraper understands.
pub(crate) const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Upper bounds of the queue-wait histogram, in seconds: ten seconds to a day.
const WAIT_BUCKETS: [f64; 8] = [
    10.0, 60.0, 300.0, 900.0, 3_600.0, 14_400.0, 86_400.0, 604_800.0,
];

static STARTED: OnceLock<Instant> = OnceLock::new();

/// Records the moment the service came up, for `rdownloader_uptime_seconds`.
pub(crate) fn mark_started() {
    let _ = STARTED.set(Instant::now());
}

/// The metrics exposition. Costs `api:metrics` and nothing else reaches it.
#[utoipa::path(
    get,
    path = "/api/v1/metrics",
    tag = "system",
    responses((status = 200, description = "Prometheus text exposition", content_type = "text/plain", body = String))
)]
pub async fn scrape_metrics(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let body = render(&collect(&state).await?);
    let content_type = if headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("application/openmetrics-text"))
    {
        OPENMETRICS_CONTENT_TYPE
    } else {
        PROMETHEUS_CONTENT_TYPE
    };
    Ok(([(header::CONTENT_TYPE, content_type)], body).into_response())
}

/// A download whose runner holds a slot right now.
fn runner_active(state: DownloadState) -> bool {
    matches!(
        state,
        DownloadState::Resolving | DownloadState::Downloading | DownloadState::Seeding
    )
}

fn kind_label(kind: rd_core::DownloadKind) -> String {
    serde_json::to_string(&kind)
        .unwrap_or_default()
        .trim_matches('"')
        .to_owned()
}

fn gauge(name: &'static str, help: &'static str, samples: Vec<Sample>) -> Family {
    Family {
        name,
        help,
        kind: MetricKind::Gauge,
        samples,
    }
}

fn counter(name: &'static str, help: &'static str, samples: Vec<Sample>) -> Family {
    Family {
        name,
        help,
        kind: MetricKind::Counter,
        samples,
    }
}

/// Every family, in the order they are exposed.
pub(crate) async fn collect(state: &AppState) -> Result<Vec<Family>, ApiError> {
    let downloads = state.database.list_downloads().await?;
    let rates = state.scheduler.transfer_rates();
    let totals = state.database.list_transfer_totals().await?;
    let accounts = state.database.list_accounts().await?;
    let now = chrono::Utc::now();

    let mut families = vec![
        gauge(
            "rdownloader_build_info",
            "The running version; always 1.",
            vec![Sample::new(&[("version", env!("CARGO_PKG_VERSION"))], 1.0)],
        ),
        gauge(
            "rdownloader_uptime_seconds",
            "Seconds since the service started.",
            vec![Sample::bare(
                STARTED
                    .get()
                    .map_or(0.0, |started| started.elapsed().as_secs_f64()),
            )],
        ),
    ];
    families.extend(queue_families(&downloads, &rates, now));
    families.extend(transfer_families(&totals));
    families.push(gauge(
        "rdownloader_provider_accounts",
        "Configured hoster accounts by provider and whether they are enabled.",
        {
            let mut by_provider: BTreeMap<(String, bool), f64> = BTreeMap::new();
            for account in &accounts {
                *by_provider
                    .entry((account.provider.clone(), account.enabled))
                    .or_default() += 1.0;
            }
            by_provider
                .into_iter()
                .map(|((provider, enabled), count)| {
                    Sample::new(
                        &[
                            ("provider", provider.as_str()),
                            ("enabled", if enabled { "true" } else { "false" }),
                        ],
                        count,
                    )
                })
                .collect()
        },
    ));
    families.push(gauge(
        "rdownloader_provider_hosts_blocked",
        "Hosts the scheduler is holding back after a rate limit or an address block.",
        vec![Sample::bare(state.scheduler.blocked_hosts().len() as f64)],
    ));
    families.extend(storage_families(state).await?);
    Ok(families)
}

fn queue_families(
    downloads: &[DownloadFile],
    rates: &std::collections::HashMap<rd_core::DownloadId, u64>,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<Family> {
    let mut by_kind_state: BTreeMap<(String, String), f64> = BTreeMap::new();
    let mut runners: BTreeMap<String, f64> = BTreeMap::new();
    let mut rate_by_kind: BTreeMap<String, f64> = BTreeMap::new();
    let mut waits = Vec::new();
    for download in downloads {
        let kind = kind_label(download.kind);
        *by_kind_state
            .entry((kind.clone(), download.state.to_string()))
            .or_default() += 1.0;
        if runner_active(download.state) {
            *runners.entry(kind.clone()).or_default() += 1.0;
        }
        if let Some(rate) = rates.get(&download.id) {
            *rate_by_kind.entry(kind).or_default() += *rate as f64;
        }
        if download.state == DownloadState::Queued {
            waits.push((now - download.created_at).num_seconds().max(0) as f64);
        }
    }
    vec![
        gauge(
            "rdownloader_queue_downloads",
            "Downloads in the queue by kind and state.",
            by_kind_state
                .into_iter()
                .map(|((kind, state), count)| {
                    Sample::new(&[("kind", kind.as_str()), ("state", state.as_str())], count)
                })
                .collect(),
        ),
        histogram(
            "rdownloader_queue_wait_seconds",
            "How long the downloads that are still waiting have been queued.",
            &WAIT_BUCKETS,
            &waits,
        ),
        gauge(
            "rdownloader_runners_active",
            "Downloads a runner is working on right now, by kind.",
            runners
                .into_iter()
                .map(|(kind, count)| Sample::new(&[("kind", kind.as_str())], count))
                .collect(),
        ),
        gauge(
            "rdownloader_transfer_rate_bytes_per_second",
            "Current transfer rate by kind.",
            rate_by_kind
                .into_iter()
                .map(|(kind, rate)| Sample::new(&[("kind", kind.as_str())], rate))
                .collect(),
        ),
    ]
}

fn transfer_families(totals: &[rd_db::TransferTotal]) -> Vec<Family> {
    let labelled = |value: fn(&rd_db::TransferTotal) -> i64| -> Vec<Sample> {
        totals
            .iter()
            .map(|total| {
                Sample::new(
                    &[
                        ("kind", total.kind.as_str()),
                        ("provider", total.provider.as_str()),
                    ],
                    value(total) as f64,
                )
            })
            .collect()
    };
    let mut outcomes = Vec::with_capacity(totals.len() * 2);
    for total in totals {
        for (outcome, count) in [("completed", total.completed), ("failed", total.failed)] {
            outcomes.push(Sample::new(
                &[
                    ("kind", total.kind.as_str()),
                    ("provider", total.provider.as_str()),
                    ("outcome", outcome),
                ],
                count as f64,
            ));
        }
    }
    vec![
        counter(
            "rdownloader_transfers_total",
            "Transfers that ended, by kind, provider and outcome.",
            outcomes,
        ),
        counter(
            "rdownloader_transfer_bytes_total",
            "Bytes of completed transfers, by kind and provider.",
            labelled(|total| total.bytes),
        ),
        counter(
            "rdownloader_transfer_retries_total",
            "Attempts that failed and were scheduled again, by kind and provider.",
            labelled(|total| total.retries),
        ),
        counter(
            "rdownloader_transfer_seconds_total",
            "Seconds from creation to completion of completed transfers, by kind and provider.",
            labelled(|total| total.seconds),
        ),
    ]
}

async fn storage_families(state: &AppState) -> Result<Vec<Family>, ApiError> {
    let capacity = state.scheduler.capacity();
    let mut free = Vec::new();
    let mut total = Vec::new();
    let mut blocked = Vec::new();
    for (target, path) in capacity.targets().await {
        let label = match target {
            StorageTarget::Root(id) => id.to_string(),
            StorageTarget::Fallback => "fallback".to_owned(),
        };
        let probe = path.clone();
        let (free_bytes, total_bytes) = tokio::task::spawn_blocking(move || {
            (
                fs2::available_space(&probe).ok(),
                fs2::total_space(&probe).ok(),
            )
        })
        .await
        .map_err(|error| anyhow::anyhow!("storage probe task failed: {error}"))?;
        if let Some(bytes) = free_bytes {
            free.push(Sample::new(&[("target", label.as_str())], bytes as f64));
        }
        if let Some(bytes) = total_bytes {
            total.push(Sample::new(&[("target", label.as_str())], bytes as f64));
        }
        blocked.push(Sample::new(
            &[("target", label.as_str())],
            if capacity.shortfall(target).await.is_some() {
                1.0
            } else {
                0.0
            },
        ));
    }
    Ok(vec![
        gauge(
            "rdownloader_storage_free_bytes",
            "Free space of each storage destination, by storage root id or fallback.",
            free,
        ),
        gauge(
            "rdownloader_storage_total_bytes",
            "Size of the volume behind each storage destination.",
            total,
        ),
        gauge(
            "rdownloader_storage_blocked",
            "Whether intake to the destination is held back for lack of space (1) or not (0).",
            blocked,
        ),
    ])
}
