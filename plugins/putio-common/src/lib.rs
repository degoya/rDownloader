//! What every Put.io plugin has to answer identically (RD-120-03).
//!
//! Put.io is three plugins — the resolver that carries the `putio` provider row, the OAuth
//! sign-in that fills its token slot, and the remote job that runs magnets and torrent files
//! on the account — because a manifest carries exactly one `plugin_type`. Two questions have
//! to get the same answer in all three, and a copy in each would be two places for them to
//! drift:
//!
//! - **What a Put.io file's address is.** The remote job hands finished files to the
//!   LinkGrabber as `https://api.put.io/v2/files/<id>/download`, and the resolver has to
//!   recognise exactly that address again. One function writes it and one reads it.
//! - **What is safe to repeat out of an error document.** Put.io answers a refusal with
//!   `{"error_type": "<word>", "error_message": "<sentence>"}`. The word is stable and
//!   documented; the sentence is written for a developer and is never repeated.
//!
//! Nothing here makes a request, and nothing here depends on a host: this is the part of the
//! provider that is pure, so `cargo test` covers it without a WebAssembly toolchain.

#![forbid(unsafe_code)]

pub mod address;
pub mod reason;
