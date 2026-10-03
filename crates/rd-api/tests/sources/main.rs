//! rd-api integration tests: other programs and polled sources: the qBittorrent, SABnzbd and *arr interfaces, feeds, indexers, subscriptions, git releases and stream schedules.
//!
//! One test binary per subject, each suite a module of it (RD-150-10). Every binary links the
//! whole service, and one binary per file meant 57 links of ~550 MB each. A new suite is a
//! module here and a row in `scripts/lib/rd-api-tests.map`, which selects suites by these
//! module names.

#[path = "../common/mod.rs"]
mod common;

mod compat_arr;
mod compat_qbittorrent;
mod compat_sabnzbd;
mod feeds;
mod git_releases;
mod indexer_search;
mod indexers;
mod stream_schedules;
mod subscriptions;
