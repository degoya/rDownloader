//! Route groups. Each area owns its paths and its OpenAPI operations.

pub(crate) mod audit;
pub(crate) mod automations;
pub(crate) mod collector;
pub(crate) mod config;
pub(crate) mod diagnostics;
pub(crate) mod media;
pub(crate) mod notify;
pub(crate) mod plugins;
pub(crate) mod queue;
pub(crate) mod security;
pub(crate) mod site_rules;
pub(crate) mod stats;
pub(crate) mod system;
pub(crate) mod torrent;
pub(crate) mod usenet;

use axum::Router;

use crate::AppState;

/// Every session-authenticated route, merged from the area modules.
pub(crate) fn protected() -> Router<AppState> {
    Router::new()
        .merge(automations::routes())
        .merge(collector::routes())
        .merge(config::routes())
        .merge(media::routes())
        .merge(notify::routes())
        .merge(plugins::routes())
        .merge(queue::routes())
        .merge(security::routes())
        .merge(site_rules::routes())
        .merge(system::routes())
        .merge(torrent::routes())
        .merge(usenet::routes())
        .merge(stats::routes())
        .merge(diagnostics::routes())
        .merge(audit::routes())
}
