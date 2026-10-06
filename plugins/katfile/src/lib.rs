//! KatFile resolver: documented XFS metadata API plus cookie-backed premium transfer.
//!
//! KatFile is an XFileSharing Pro (XFS) installation, the same hosting engine `plugins/ddownload`
//! runs; this plugin is built on the shared `xfs-common` crate extracted from ddownload in Task
//! 11, cloning ddownload's dual API-key/cookie mechanism exactly: the API key proves the account
//! and answers `account/info`, `file/info` and `file/direct_link`, the cookie session carries
//! the premium `download2` form (`resolver.rs`).
//!
//! What KatFile does differently, verified against JD's `KatfileCom.java` (revision 53112) and
//! its `XFileSharingProBasic` base class:
//!
//! - **Domains**: `getPluginDomains()` (lines 64-68) registers seven, `katfile.biz` first as
//!   the main domain; `rewriteHost` (lines 136-142) moves every alias onto it, because the main
//!   domain moved five times in a year. `resolver/api.rs` holds the list (`MATCH_HOSTS`), the
//!   main domain (`PRIMARY_DOMAIN`) and the rewrite (`canonicalize_host`); `manifest.toml` and
//!   `HOSTERS` follow it.
//! - **API base**: `getAPIBase()` is the main page plus `/api`, with no separate API host —
//!   unlike ddownload's `api-v2.` subdomain (`API_BASE`).
//! - **Premium-form captcha**: `findFormDownload2Premium` (lines 236-243) hands the found
//!   `download2` form, not the page, to `handleCaptcha`. This plugin cannot solve one, so it
//!   scans that form only and reports `messages::CAPTCHA_REQUIRED` instead of posting it.
//! - **Premium-only markers**: `getPremiumOnlyErrorMessage` (lines 309-317) adds
//!   `">\s*This file is available for Premium"` and a `/?op=registration&redirect=` URL
//!   (`page::premium_only_reason`, `messages::PREMIUM_ONLY`).
//! - **Pre-download wait**: `regexWaittime` (lines 320-328) reads `var estimated_time = (\d+)`
//!   ahead of the base class's markers, in tenths of a second (`page::estimated_wait_seconds`,
//!   `messages::DOWNLOAD_WAIT`).
//!
//! Left out on purpose, as in ddownload: the `premim_expire` typo fallback of
//! `fetchAccountInfoAPI`, and `isOffline`'s page markers (lines 289-296) — a missing file is
//! known from `file/info`'s per-item status.

/// Domains served by this hoster, `katfile.biz` (the current live main domain) first — mirrors
/// JD's `KatfileCom.getPluginDomains()` (rev 53112); see this crate's module doc.
pub(crate) const HOSTERS: &[&str] = &[
    "katfile.biz",
    "katfile.space",
    "katfile.ws",
    "katfile.vip",
    "katfile.online",
    "katfile.cloud",
    "katfile.com",
];

mod account;
mod messages;

#[cfg(target_arch = "wasm32")]
mod guest;
mod page;
mod resolver;
#[cfg(test)]
mod session_trace_tests;

/// This plugin's own packaging manifest: the single authority for its identity, domains and
/// capability grants. The native build reads it from here, the component build gets the same
/// file out of the package the host installed, so neither can describe the plugin differently.
#[cfg(not(target_arch = "wasm32"))]
pub const MANIFEST: &str = include_str!("../manifest.toml");

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(not(target_arch = "wasm32"))]
pub use native::KatfileResolver;
