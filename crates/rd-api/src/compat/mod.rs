//! Compatibility adapters for automation clients that speak someone else's API.
//!
//! Kept apart from `/api/v1` on purpose. These surfaces exist to satisfy clients written
//! against SABnzbd and qBittorrent, so their shapes, their status vocabulary and their error
//! conventions are theirs, not ours — and the native contract must never be bent to make one
//! of them fit. They translate at the edge and reuse the same handlers, validation and error
//! codes underneath.

pub(crate) mod qbittorrent;
pub(crate) mod sabnzbd;

use axum::Router;

use crate::AppState;

/// Every compatibility route, merged into the application router.
///
/// Takes the state because the qBittorrent half authenticates with a layer rather than with a
/// call inside each handler, and a layer needs the state at build time. SABnzbd needs no such
/// thing: it has a single entry point, so a mode added to its `match` is behind the check by
/// construction.
pub(crate) fn routes(state: &AppState) -> Router<AppState> {
    sabnzbd::routes().merge(qbittorrent::routes(state))
}
