//! Film and series metadata for a link the online check just resolved (RD-107-01).
//!
//! Three modules, none of which knows about the plugin contract, so all three are unit-tested
//! on the host without a WebAssembly target:
//!
//! - [`release`] reads a title, a year and a season/episode out of a release name — and, far
//!   more often, decides that there is none. That decision is the whole negative path: this
//!   plugin is asked about every link, so "this is not a film" has to cost nothing.
//! - [`json`] is the small reader for the source's answer, which is nested enough that
//!   scanning for substrings would attribute one entry's fields to another.
//! - [`lookup`] builds the addresses and turns an answer into fields.
//!
//! `guest` is the component wrapper and exists only on `wasm32`.

pub mod json;
pub mod lookup;
pub mod release;

#[cfg(target_arch = "wasm32")]
mod guest;
