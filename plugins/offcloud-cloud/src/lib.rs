//! Offcloud cloud downloads: a `remote-job` plugin (RD-120-02).
//!
//! The half of Offcloud the resolver cannot reach. `resolve` answers with one address in one
//! call, and a magnet handed to Offcloud's cloud answers with a request identifier and then
//! works for minutes or hours before there is anything to fetch — the shape
//! `docs/adr/0003-a-job-that-runs-at-the-provider.md` opened the eleventh world for. Every
//! call here is short and returns on the provider's next answer; the row, the clock, the
//! restart and the person's decisions belong to the host.
//!
//! A sibling rather than part of `plugins/offcloud/`, because a manifest carries exactly one
//! `plugin_type`. The two share one account and one provider row:
//!
//! - `plugins/offcloud/` resolves hoster links, carries the `[provider]` row and renews a
//!   short-lived address by asking for a new one.
//! - this one turns a magnet or an address into a job in the account's Offcloud cloud.
//!
//! Three things about Offcloud shape what this plugin can and cannot do, and each is argued
//! where it is implemented:
//!
//! - **A cloud job has no file-selection step.** Offcloud fetches the whole of what it was
//!   given and says afterwards what is in it, so `poll` never answers `awaiting-choice` and
//!   `choose` refuses under a stable code rather than inventing a question ([`api::stage_of`]).
//! - **The duplicate guard has to work without an info hash for every source.** A magnet
//!   carries one; an ordinary address does not, so the key for an address is derived from the
//!   address itself, and the two key spaces are kept apart by a prefix ([`source`]).
//! - **Removing at the provider is one documented call with a list parameter**, which is the
//!   one place this plugin sends JSON rather than the form body the published document asks
//!   for ([`api::REMOVE_PATH`]).
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
