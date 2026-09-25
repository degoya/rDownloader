//! Real-Debrid magnets and torrents: the first `remote-job` plugin (RD-107-06).
//!
//! The half of Real-Debrid the resolver could not reach. `resolve` answers with one file and a
//! magnet answers with an identifier and hours of work, so RD-106-03 split this off rather
//! than half-deliver it — and the answer to *where* it lives is
//! `docs/adr/0003-a-job-that-runs-at-the-provider.md`: an eleventh world whose calls are all
//! short, with the row, the clock, the person's answer and the restart in the host.
//!
//! A sibling rather than part of `plugins/realdebrid/`, because a manifest carries exactly one
//! `plugin_type`. The three of them share one account and one provider row:
//!
//! - `plugins/realdebrid-auth/` signs in by device code and keeps the token renewed.
//! - `plugins/realdebrid/` unrestricts hoster links — including the links this plugin hands
//!   back, because what a finished torrent produces at Real-Debrid is still a restricted link.
//! - this one turns a magnet or a `.torrent` into a job at the provider.
//!
//! Everything that can be tested without a WebAssembly toolchain lives outside the component:
//! [`source`] derives the content key, [`api`] holds the response shapes, the state mapping
//! and the failure classification. `guest` is the thin wrapper around the two and exists only
//! on `wasm32`.

pub mod api;
pub mod messages;
pub mod source;

#[cfg(target_arch = "wasm32")]
mod guest;
