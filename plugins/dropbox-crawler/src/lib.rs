//! Dropbox folders and shared folder links in the LinkGrabber (RD-106-06).
//!
//! The half of Dropbox the resolver cannot reach. `resolve` answers with exactly one file, so
//! a folder address has no shape to come back in; `crawl` walks `files/list_folder` under its
//! own limits and hands back the files that were behind it all along, each with the folder it
//! sat in as its package suggestion.
//!
//! A sibling rather than a second world for the resolver, because a manifest carries exactly
//! one `plugin_type`. The account, its token and the `dropbox` provider row stay with
//! `plugins/dropbox/`, which is the only one of the three that may declare a provider; what
//! crosses between them is nothing but an address — the ones `dropbox_common::address` builds
//! and the resolver claims.
//!
//! Everything that can be tested without a WebAssembly toolchain lives outside the component:
//! [`target`] decides what is claimed, [`listing`] reads an entry, [`walk`] bounds the tree and
//! carries the cursor. `guest` is the thin wrapper around the three and exists only on `wasm32`.

pub mod listing;
pub mod messages;
pub mod target;
pub mod walk;

#[cfg(target_arch = "wasm32")]
mod guest;
