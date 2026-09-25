//! Put.io magnets and torrents as remote jobs (RD-120-03).
//!
//! The half of Put.io the resolver could not reach. `resolve` answers with one file, and a
//! magnet answers with an identifier and hours of work, so this is the eleventh world rather
//! than a resolver with a loop in it: `docs/adr/0003-a-job-that-runs-at-the-provider.md`
//! argues where the row, the clock, the person's answer and the restart belong, and every one
//! of them is in the host.
//!
//! A sibling rather than part of `plugins/putio/`, because a manifest carries exactly one
//! `plugin_type`. The three of them share one account and one provider row:
//!
//! - `plugins/putio-oauth/` signs in and keeps the token in the vault.
//! - `plugins/putio/` carries the `putio` provider row and turns a Put.io file address into a
//!   download — including the addresses this plugin hands back.
//! - this one turns a magnet or a `.torrent` into a transfer at the provider.
//!
//! Two things about Put.io shape this plugin and are argued where they are implemented:
//!
//! - **A `.torrent` is submitted as the magnet it is equivalent to.** Put.io's `transfers/add`
//!   takes one address, and its only way to accept container bytes is a resumable upload
//!   protocol on a second host. [`source::container_magnet`] has the trade-off in full.
//! - **There is no file selection before the download.** Put.io fetches a torrent whole and
//!   its files exist only afterwards, so `poll` never answers `awaiting-choice`; it answers
//!   `ready` with the complete tree and the choice is made in the LinkGrabber. [`api`] has the
//!   reasoning.
//!
//! Everything that can be tested without a WebAssembly toolchain lives outside the component:
//! [`source`] derives the content key and the address Put.io is handed, [`api`] holds the
//! response shapes, the state mapping and the failure classification. `guest` is the thin
//! wrapper around the two and exists only on `wasm32`.

pub mod api;
pub mod messages;
pub mod source;

#[cfg(target_arch = "wasm32")]
mod guest;
