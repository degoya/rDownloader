//! Fixtures the adapters' tests share.

use rd_core::{Subscription, SubscriptionKind};

/// An enabled, primed subscription of `kind` polling `url` hourly, every other field at its
/// default. A test sets what it is about with struct update syntax, instead of spelling out
/// all thirty fields once more (audit 1.9.1, INTAKE-15).
pub(crate) fn subscription(name: &str, kind: SubscriptionKind, url: &str) -> Subscription {
    Subscription {
        id: rd_core::SubscriptionId::new(),
        name: name.to_owned(),
        url: url.parse().expect("url"),
        kind,
        enabled: true,
        mode: rd_core::SubscriptionMode::Review,
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        interval_seconds: 3_600,
        filters: rd_core::SubscriptionFilters::default(),
        backlog: rd_core::BacklogPolicy::default(),
        category_map: Vec::new(),
        source_categories: Vec::new(),
        primed: true,
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
        schedule: None,
        script_arguments: Vec::new(),
        indexer_search: rd_core::IndexerSearch::default(),
        git_release: rd_core::GitReleaseOptions::default(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}
