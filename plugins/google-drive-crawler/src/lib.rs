//! Google Drive folders and shared drives in the LinkGrabber (RD-106-04).
//!
//! The half of Google Drive the resolver cannot reach. `resolve` answers with exactly one file,
//! so a folder address has no shape to come back in; `crawl` walks `files.list` under its own
//! limits and hands back the files that were behind it all along, each with the folder it sat
//! in as its package suggestion.
//!
//! A sibling rather than a second world for the resolver, because a manifest carries exactly
//! one `plugin_type`. The account, its token and the `google_drive` provider row stay with
//! `plugins/google-drive/`, which is the only one of the three that may declare a provider;
//! what crosses between them is nothing but an address — the canonical
//! `https://drive.google.com/file/d/<id>/view` that `google_drive_common::address::file_address`
//! builds and the resolver claims.
//!
//! Everything that can be tested without a WebAssembly toolchain lives outside the component:
//! [`target`] decides what is claimed, [`listing`] reads an answer, [`walk`] bounds the tree.
//! `guest` is the thin wrapper around the three and exists only on `wasm32`.

pub mod listing;
pub mod messages;
pub mod target;
pub mod walk;

#[cfg(target_arch = "wasm32")]
mod guest;
