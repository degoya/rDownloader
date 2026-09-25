//! Seedr transfers: a `remote-job` plugin (RD-120-04).
//!
//! The half of Seedr the resolver cannot reach. `resolve` answers with one address in one call,
//! and a magnet handed to a Seedr account answers with a transfer identifier and then works for
//! minutes or hours before there is anything to fetch — the shape
//! `docs/adr/0003-a-job-that-runs-at-the-provider.md` opened the eleventh world for. Every call
//! here is short and returns on the provider's next answer; the row, the clock, the restart and
//! the person's decisions belong to the host.
//!
//! A sibling rather than part of `plugins/seedr/`, because a manifest carries exactly one
//! `plugin_type`. The two share one account, one provider row and the crate `seedr-common`:
//!
//! - `plugins/seedr/` resolves the per-file addresses and carries the `[provider]` row.
//! - this one turns a magnet or a `.torrent` into a transfer in the account.
//!
//! Four things about Seedr shape what this plugin can and cannot do, and each is argued where
//! it is implemented:
//!
//! - **A transfer has no file-selection step.** Seedr fetches the whole torrent, so `poll`
//!   never answers `awaiting-choice` and `choose` refuses under a stable code rather than
//!   inventing a question. That is the same answer TorBox, Put.io, Offcloud and Premiumize
//!   reached independently, and it is a limit of the contract rather than of this plugin: see
//!   `guest::Component::choose` and RD-120-35.
//! - **The poll is the folder listing, not the transfer endpoint.** A finished transfer stops
//!   being a transfer and becomes a folder, so the endpoint named after polling can only stop
//!   answering — which is what a deleted transfer looks like too ([`api::stage_of`]).
//! - **A container is submitted as a magnet.** `POST /rest/transfer/file` is a multipart
//!   upload; the container is read locally and re-expressed as the magnet it is equivalent to,
//!   which leaves the content key unchanged (`seedr_common::torrent`).
//! - **The finished addresses carry no credential.** They are stable per-file API addresses,
//!   and what authenticates them is the account's own Basic pair, which the download engine
//!   attaches because the `seedr` row declares `transfer_auth = "basic"` (RD-120-38) -- not
//!   anything this plugin can state (`seedr_common::address`).
//!
//! Everything that can be tested without a WebAssembly toolchain lives outside the component:
//! [`api`] holds the response shapes, the state machine and the failure classification, and
//! `seedr-common` holds what both Seedr plugins have to answer identically. `guest` is the thin
//! wrapper around them and exists only on `wasm32`.

pub mod api;
pub mod messages;

#[cfg(target_arch = "wasm32")]
mod guest;
