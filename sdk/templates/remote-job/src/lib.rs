//! A scaffold remote job. It compiles, packages and passes conformance as it is.
//!
//! A remote job is work that happens at somebody else's provider and outlives the call that
//! started it. A magnet handed to a debrid account answers with an identifier, not a file: it
//! runs for minutes or hours, it stops half-way and refuses to continue until a person has
//! said which files they want, and the account keeps it afterwards whether anybody wanted
//! that or not. None of the other ten worlds can hold that — `resolve` answers with one file,
//! `crawl` with a list in one call, `parse` with what was already in hand, and `run` carries
//! bytes for exactly as long as one download lasts. The reasoning, and the four designs that
//! were rejected, are in `docs/adr/0003-a-job-that-runs-at-the-provider.md`.
//!
//! Four things the host guarantees, which shape how this is written:
//!
//! - **Nothing here waits.** Every exported function returns on the provider's next answer.
//!   The waiting lives on a row in the host's database, which costs no fuel and survives a
//!   restart — which is what lets this plugin keep a fuel and timeout budget a crawler
//!   waiting for a torrent could never have kept.
//! - **You remember nothing.** A guest is instantiated fresh for every call. Whatever you
//!   need on the next one goes into `remote-handle.job-state`, which the host stores verbatim
//!   and hands back. It is not a credential and is never shown.
//! - **The duplicate is prevented on the host's side, not here.** `identify` gives the host a
//!   content key derived locally, without a request; a unique index on (account, key) then
//!   makes a second submit of the same content impossible before any request goes out; and
//!   `adopt` closes the one window that index cannot — a crash between the request leaving
//!   and the identifier coming back. So `submit` neither retries nor pretends to be
//!   idempotent, because at every provider this world was designed for it is not.
//! - **You never see a credential.** The account's token reaches a request only as the
//!   template `{{secret:<reference>}}`, which the host expands on the way out and only
//!   towards the domains the manifest declares.
//!
//! And one rule that is yours to keep: **`discard` is called from one explicit, confirmed
//! request and from nowhere else.** Nothing in the host's sweep reaches it. What rDownloader
//! did not put in somebody's account on its own it does not take out on its own.
//!
//! The layout follows one practical concern: everything that can be tested without a
//! WebAssembly toolchain lives outside the component. `cargo test` in a fresh scaffold runs
//! [`digest`], [`source`] and [`reply`] on the host target; `guest` exists only on `wasm32`.

pub mod digest;
pub mod reply;
pub mod source;

#[cfg(target_arch = "wasm32")]
mod guest;
