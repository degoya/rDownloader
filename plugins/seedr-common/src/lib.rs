//! What every Seedr plugin has to answer identically (RD-120-04).
//!
//! Seedr is two plugins — the resolver that carries the `seedr` provider row, and the remote
//! job that runs magnets and torrent files on the account — because a manifest carries exactly
//! one `plugin_type` and only a resolver may declare a `[provider]` section. Three questions
//! have to get the same answer in both, and a copy in each would be two places for them to
//! drift:
//!
//! - **What a Seedr file's address is.** The remote job hands a finished transfer's files to
//!   the LinkGrabber as `https://www.seedr.cc/rest/file/<id>`, and the resolver claims exactly
//!   that address again. One function writes it and one reads it ([`address`]).
//! - **How a request is authenticated.** Seedr's REST v1 is HTTP Basic and nothing else, and
//!   neither plugin ever holds a credential: both write the same header template and the host
//!   builds the blob ([`address::AUTHORIZATION_TEMPLATE`]).
//! - **What is safe to repeat out of a refusal** ([`reason`]).
//!
//! [`torrent`] is the fourth thing they share by being the thing only one of them uses today:
//! the content key a magnet and the matching `.torrent` file both reduce to. It lives here
//! rather than in the remote job because the key is a promise the *provider* makes — one job
//! for one piece of content — and a second derivation of it anywhere would break that promise
//! quietly.
//!
//! Nothing here makes a request, and nothing here depends on a host: this is the part of the
//! provider that is pure, so `cargo test` covers it without a WebAssembly toolchain.

#![forbid(unsafe_code)]

pub mod address;
pub mod folder;
pub mod reason;
pub mod torrent;
