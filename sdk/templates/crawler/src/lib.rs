//! A scaffold folder crawler. It compiles, packages and passes conformance as it is.
//!
//! A crawler answers the question a resolver cannot: *what lies behind this address?* The
//! resolver world's `resolve` returns exactly one file, so a folder share, a cloud directory
//! or a release page has no shape to come back in. `crawl` returns a list, and everything in
//! that list then goes through the same review, validation and routing a pasted link does.
//!
//! Three things the host guarantees, which shape how this is written:
//!
//! - **You are asked before you are handed an address.** `claims-url` runs first and reaches
//!   nothing at all, so a plugin that has no business with an address never fetches it.
//! - **You never see a credential.** The account's stored secret reaches a request only as
//!   the template `{{secret:<reference>}}`, which the host expands on the way out — and only
//!   towards the hosts that reference is allowed to be sent to.
//! - **You propose, the application decides.** Nothing this plugin returns is queued by
//!   itself. The host also caps how many links it will take from one answer, so a crawler
//!   that answers with a hundred thousand links is trimmed rather than obeyed.
//!
//! The layout follows one practical concern: everything that can be tested without a
//! WebAssembly toolchain lives outside the component. `cargo test` in a fresh scaffold runs
//! [`target`], [`listing`] and [`walk`] on the host target; `guest` exists only on `wasm32`.

pub mod listing;
pub mod target;
pub mod walk;

#[cfg(target_arch = "wasm32")]
mod guest;
